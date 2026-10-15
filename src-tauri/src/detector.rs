//! 编码检测
//!
//! 检测策略 (按优先级):
//! 1. BOM 标记 (UTF-8 / UTF-16 LE/BE)
//! 2. 探测窗口含非 ASCII: 严格 UTF-8 校验 → chardetng 启发式
//! 3. 探测窗口纯 ASCII: 【不直接判定】向后扫描整个文件找首个非 ASCII 字节,
//!    找到后以该字节为起点取块判定 (修复: 前几 MB 英文日志 + 后置中文的 GBK 文件被误判 UTF-8)
//! 4. 全文件纯 ASCII → UTF-8 (ASCII 是其子集, 无需转换)
//! 5. 中文环境保守默认 GB18030 (GBK 超集)

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// 向后扫描时的块大小 (顺序 IO, 高位字节探测)
const SCAN_BUF_SIZE: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedEncoding {
    Utf8,
    Utf8Bom,
    Gb18030,
    Big5,
    ShiftJis,
    EucKr,
    Windows1252,
    Utf16Le,
    Utf16Be,
    #[allow(dead_code)]
    Unsupported,
}

impl DetectedEncoding {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf8Bom => "UTF-8 (BOM)",
            Self::Gb18030 => "GB18030",
            Self::Big5 => "Big5",
            Self::ShiftJis => "Shift_JIS",
            Self::EucKr => "EUC-KR",
            Self::Windows1252 => "Windows-1252",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Unsupported => "Unsupported",
        }
    }
}

/// 检测文件编码, 返回 (编码, 已 seek 回 0 的文件句柄)
pub fn detect(path: &Path, probe_size: usize) -> std::io::Result<(DetectedEncoding, File)> {
    let mut file = File::open(path)?;
    let probe = probe_size.clamp(4096, 1024 * 1024); // 4 KB ~ 1 MB

    // 1. 读满探测窗口 (单次 read 可能返回部分数据)
    let mut buf = vec![0u8; probe];
    let n = read_full(&mut file, &mut buf)?;
    buf.truncate(n);

    if n == 0 {
        file.seek(SeekFrom::Start(0))?;
        return Ok((DetectedEncoding::Utf8, file)); // 空文件
    }

    // 2. BOM 检测 (BOM 优先于一切)
    if buf.starts_with(&[0xEF, 0xBB, 0xBF]) {
        file.seek(SeekFrom::Start(0))?;
        return Ok((DetectedEncoding::Utf8Bom, file));
    }
    if buf.starts_with(&[0xFF, 0xFE]) {
        file.seek(SeekFrom::Start(0))?;
        return Ok((DetectedEncoding::Utf16Le, file));
    }
    if buf.starts_with(&[0xFE, 0xFF]) {
        file.seek(SeekFrom::Start(0))?;
        return Ok((DetectedEncoding::Utf16Be, file));
    }

    // 3. 探测窗口含非 ASCII → 直接判定
    if buf.iter().any(|&b| b >= 0x80) {
        let enc = classify_bytes(&buf);
        file.seek(SeekFrom::Start(0))?;
        return Ok((enc, file));
    }

    // 4. 探测窗口纯 ASCII → 编码信息不足, 向后扫描找首个非 ASCII 字节
    //    (典型场景: 日志类 CSV 前几 MB 全英文, 中文在文件后部)
    let mut scan_buf = vec![0u8; SCAN_BUF_SIZE];
    let mut pos = n as u64;
    loop {
        let sn = read_full(&mut file, &mut scan_buf)?;
        if sn == 0 {
            // 全文件纯 ASCII → UTF-8 子集, 无需转换
            file.seek(SeekFrom::Start(0))?;
            return Ok((DetectedEncoding::Utf8, file));
        }
        if let Some(off) = scan_buf[..sn].iter().position(|&b| b >= 0x80) {
            // 该字节必为多字节序列的 lead byte (其前全 ASCII, 边界天然对齐),
            // 从此处取块判定
            let start = pos + off as u64;
            file.seek(SeekFrom::Start(start))?;
            let mut block = vec![0u8; probe];
            let bn = read_full(&mut file, &mut block)?;
            block.truncate(bn);
            let enc = classify_bytes(&block);
            file.seek(SeekFrom::Start(0))?;
            return Ok((enc, file));
        }
        pos += sn as u64;
    }
}

/// 对数据块判定编码: 严格 UTF-8 校验 → chardetng 启发式
fn classify_bytes(bytes: &[u8]) -> DetectedEncoding {
    if bytes.is_empty() {
        return DetectedEncoding::Utf8;
    }

    // 严格 UTF-8 校验
    if let Err(e) = std::str::from_utf8(bytes) {
        if e.error_len().is_some() {
            // 块内存在确定非法字节 → chardetng 启发式
            // chardetng 1.0: new() 需指定是否允许 ISO-2022-JP, guess() 的第二个参数
            // 由 bool 改为 Utf8Detection。保持与 0.1 一致的语义 (两者均允许)。
            let mut det = EncodingDetector::new(Iso2022JpDetection::Allow);
            det.feed(bytes, true);
            return map_guess(det.guess(None, Utf8Detection::Allow));
        }
        // error_len() == None: 仅块尾序列被截断 (非块内非法) → 宽容视为 UTF-8
    }
    DetectedEncoding::Utf8
}

fn map_guess(enc: &'static encoding_rs::Encoding) -> DetectedEncoding {
    match enc.name() {
        "UTF-8" => DetectedEncoding::Utf8,
        "GBK" | "GB18030" => DetectedEncoding::Gb18030,
        "Big5" => DetectedEncoding::Big5,
        "Shift_JIS" => DetectedEncoding::ShiftJis,
        "EUC-KR" => DetectedEncoding::EucKr,
        "windows-1252" => DetectedEncoding::Windows1252,
        _ => DetectedEncoding::Gb18030, // 中文场景保守默认
    }
}

/// 循环 read 直到读满 buf 或 EOF, 返回实际读取字节数
fn read_full(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        let n = file.read(&mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}