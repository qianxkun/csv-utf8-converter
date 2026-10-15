//! 流式编码转换
//!
//! - 使用 `encoding_rs` (含 SSSE3/AVX2 SIMD 加速) 做字节级转换
//! - 通过 BufReader/BufWriter 维持 ~256 KB 滑动窗口, 内存与文件大小无关
//! - 临时文件 + 原子 rename, 单文件失败不影响其他文件
//! - 源文件永不被修改 (始终输出到独立路径)

use crate::detector::{detect, DetectedEncoding};
use anyhow::{Context, Result};
use encoding_rs::{
    CoderResult, Encoding, BIG5, EUC_KR, GB18030, SHIFT_JIS, UTF_8, UTF_16BE, UTF_16LE,
    WINDOWS_1252,
};
use std::fs::File as FsFile;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 读取/写入缓冲 (256 KB)
const IO_BUF_SIZE: usize = 256 * 1024;
/// 输出缓冲上限 (UTF-8 最长 4 字节/字符, 留余量)
const OUTPUT_BUF_SIZE: usize = IO_BUF_SIZE * 4 + 16;

#[derive(Debug, Clone)]
pub struct ConvertOptions {
    pub probe_size: usize,
    pub write_bom: bool,
    pub retries: usize,
    /// 已是 UTF-8 的文件跳过不处理
    pub skip_utf8: bool,
}

#[derive(Debug, Clone)]
pub struct ConvertResult {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub encoding: DetectedEncoding,
    /// true = 源文件已是 UTF-8, 未做转换 (skip_utf8 选项命中, 原样复制)
    pub skipped: bool,
}

/// 转换 input → output (输出模式, 源文件不被修改)
pub fn convert(input: &Path, output: &Path, opts: &ConvertOptions) -> Result<ConvertResult> {
    run_with_retry(opts, || try_convert(input, output, opts))
}

fn run_with_retry<F>(opts: &ConvertOptions, mut f: F) -> Result<ConvertResult>
where
    F: FnMut() -> Result<ConvertResult>,
{
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..=opts.retries {
        match f() {
            Ok(r) => return Ok(r),
            Err(e) => {
                if attempt < opts.retries {
                    let delay = 100u64 * (1 << attempt.min(4));
                    std::thread::sleep(Duration::from_millis(delay));
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("未知错误")))
}

fn try_convert(input: &Path, output: &Path, opts: &ConvertOptions) -> Result<ConvertResult> {
    // 1. 确保输出目录存在
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建输出目录失败: {}", parent.display()))?;
        }
    }

    // 2. 检测编码 (顺带得到 seek 回 0 的句柄)
    let (detected, src_file) = detect(input, opts.probe_size)?;
    let bytes_in = src_file.metadata()?.len();

    // 3. UTF-8 跳过: 原样复制到输出目录 (保证输出目录文件完整), 计为 skipped
    if opts.skip_utf8
        && matches!(detected, DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom)
    {
        drop(src_file);
        if input != output {
            std::fs::copy(input, output).with_context(|| {
                format!(
                    "复制 UTF-8 文件失败: {} -> {}",
                    input.display(),
                    output.display()
                )
            })?;
        }
        return Ok(ConvertResult {
            bytes_in,
            bytes_out: bytes_in,
            encoding: detected,
            skipped: true,
        });
    }

    // 4. 选编码
    let src_enc: &'static Encoding = match detected {
        DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom => UTF_8,
        DetectedEncoding::Gb18030 => GB18030,
        DetectedEncoding::Big5 => BIG5,
        DetectedEncoding::ShiftJis => SHIFT_JIS,
        DetectedEncoding::EucKr => EUC_KR,
        DetectedEncoding::Windows1252 => WINDOWS_1252,
        DetectedEncoding::Utf16Le => UTF_16LE,
        DetectedEncoding::Utf16Be => UTF_16BE,
        DetectedEncoding::Unsupported => {
            return Err(anyhow::anyhow!(
                "不支持的编码 {}, 请扩展 converter.rs 映射",
                detected.name()
            ));
        }
    };

    // 5. 转换到临时文件
    //    tmp 与【最终目标】同目录: 输出模式在输出目录 (不写源目录), in-place 在原目录
    let tmp = tmp_path_for(output);
    let _ = std::fs::remove_file(&tmp); // 清理可能残留

    let bytes_out = {
        let dst = FsFile::create(&tmp)
            .with_context(|| format!("创建临时文件失败: {}", tmp.display()))?;
        let mut reader = BufReader::with_capacity(IO_BUF_SIZE, src_file);
        let mut writer = BufWriter::with_capacity(IO_BUF_SIZE, dst);
        let n = decode_stream(&mut reader, &mut writer, src_enc, opts.write_bom)?;
        writer.flush().context("刷新缓冲失败")?;
        // writer drop 关闭文件
        n
    };

    // 6. 原子替换: tmp → output (rename 失败则清理临时文件)
    if let Err(e) = std::fs::rename(&tmp, output) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e)
            .with_context(|| format!("重命名 {} -> {} 失败", tmp.display(), output.display()));
    }

    Ok(ConvertResult {
        bytes_in,
        bytes_out,
        encoding: detected,
        skipped: false,
    })
}

/// 真正的流式解码, 返回写入字节数
///
/// 正确的 encoding_rs 流式用法:
/// - `decode_to_utf8` 以 `CoderResult` 报告输入/输出消耗情况
/// - 输出缓冲满 (OutputFull) 时, 未消费的输入 (`read..n`) 必须继续送入解码器
/// - 之前的实现忽略了 read/result, 大文件 (输出超过缓冲容量) 会静默丢数据
fn decode_stream<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    src_enc: &'static Encoding,
    write_bom: bool,
) -> Result<u64> {
    let mut decoder = src_enc.new_decoder_with_bom_removal();
    let mut input_buf = vec![0u8; IO_BUF_SIZE];
    let mut output_buf = vec![0u8; OUTPUT_BUF_SIZE];
    let mut total_written: u64 = 0;

    // 可选 BOM 输出
    if write_bom {
        writer.write_all(&[0xEF, 0xBB, 0xBF])?;
        total_written += 3;
    }

    loop {
        let n = reader.read(&mut input_buf)?;
        let last = n == 0;
        // 本轮待解码切片 (OutputFull 时会缩短)
        let mut src: &[u8] = &input_buf[..n];

        // 内层循环: 输出缓冲满时持续消费剩余输入
        loop {
            let (result, read, written, _replaced) =
                decoder.decode_to_utf8(src, &mut output_buf, last);
            if written > 0 {
                writer.write_all(&output_buf[..written])?;
                total_written += written as u64;
            }
            src = &src[read..];
            match result {
                CoderResult::InputEmpty => break,
                CoderResult::OutputFull => continue, // 缓冲已冲刷, 继续解码剩余
            }
        }

        if last {
            break;
        }
    }

    Ok(total_written)
}

/// 为输出路径生成同目录的临时文件路径 (`.{name}.{pid}.{nanos}.tmp`)。
///
/// 后缀固定为 `.tmp`, 因此不会被 `*.csv` / `*.txt` 扫描规则再次拾取。
pub(crate) fn tmp_path_for(output: &Path) -> PathBuf {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let stem = output
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    parent.join(format!(".{}.{}.{}.tmp", stem, pid, nanos))
}