//! 文件预览
//!
//! 为「编码转换」面板提供**逐个文件**的:
//! - 检测到的原始编码
//! - 分隔符 / 表头 / 前 N 行数据
//!
//! 目的是让用户在真正转换前就能判断「这个文件到底要不要转码」,
//! 从而只勾选需要转码的文件。
//!
//! 实现要点:
//! - 只读取文件头部 `PREVIEW_READ_BYTES`, 预览成本与文件大小基本无关
//! - 复用 `detector::detect` 的编码检测结果 (与转换阶段完全一致)
//! - 复用 `converter::encoding_of` 的编码映射 (与转换阶段完全一致)
//! - 极简 RFC4180 解析, 支持引号包裹 / `""` 转义 / 引号内换行

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;

use crate::converter::encoding_of;
use crate::detector::{detect, DetectedEncoding};

/// 预览时最多读取的字节数 (仅取文件头部, 足够覆盖表头与若干数据行)
const PREVIEW_READ_BYTES: usize = 64 * 1024;
/// 单元格文本最大字符数 (超长内容截断, 防止撑爆界面)
const MAX_CELL_CHARS: usize = 120;
/// 最多展示的列数 (超宽表只取前若干列)
const MAX_COLUMNS: usize = 30;

/// 单个文件的预览信息
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    /// 绝对路径 (前端勾选后回传给 `start_conversion`)
    pub path: String,
    /// 相对输入目录的路径 (界面展示用)
    pub relative_path: String,
    pub size: u64,
    /// 检测到的原始编码显示名
    pub encoding: String,
    /// 是否为非 UTF-8 编码 (即"需要转码")
    pub needs_transcode: bool,
    /// 嗅探出的分隔符 (原样字符, 如 `,` `\t`)
    pub delimiter: String,
    /// 表头列名
    pub headers: Vec<String>,
    /// 前若干行数据
    pub rows: Vec<Vec<String>>,
    /// 列数超过 `MAX_COLUMNS` 被截断
    pub columns_truncated: bool,
    /// 读取或解析失败的原因
    pub error: Option<String>,
}

/// 整个输入目录的预览结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResult {
    pub count: usize,
    /// 其中需要转码 (非 UTF-8) 的文件数
    pub needs_transcode: usize,
    pub total_bytes: u64,
    pub files: Vec<FilePreview>,
}

/// 并行生成每个文件的预览
pub fn build(
    input_root: &Path,
    files: Vec<PathBuf>,
    sample_rows: usize,
    probe_size: usize,
) -> PreviewResult {
    let items: Vec<FilePreview> = files
        .par_iter()
        .map(|p| preview_one(p, input_root, sample_rows, probe_size))
        .collect();

    let needs_transcode = items.iter().filter(|f| f.needs_transcode).count();
    let total_bytes = items.iter().map(|f| f.size).sum();

    PreviewResult {
        count: items.len(),
        needs_transcode,
        total_bytes,
        files: items,
    }
}

/// 生成单个文件的预览; 任何失败都折叠进 `error` 字段而不中断整体扫描
fn preview_one(
    path: &Path,
    input_root: &Path,
    sample_rows: usize,
    probe_size: usize,
) -> FilePreview {
    let relative_path = path
        .strip_prefix(input_root)
        .unwrap_or(path)
        .display()
        .to_string();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let abs_path = path.display().to_string();

    // 统一的失败构造, 避免多处重复
    let failed = |encoding: String, error: String| FilePreview {
        path: abs_path.clone(),
        relative_path: relative_path.clone(),
        size,
        encoding,
        needs_transcode: false,
        delimiter: String::new(),
        headers: Vec::new(),
        rows: Vec::new(),
        columns_truncated: false,
        error: Some(error),
    };

    let (detected, mut src_file) = match detect(path, probe_size) {
        Ok(v) => v,
        Err(e) => return failed("?".to_string(), format!("{:#}", e)),
    };

    let needs_transcode = !matches!(
        detected,
        DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom
    );
    let encoding = detected.name().to_string();

    let Some(enc) = encoding_of(detected) else {
        return failed(encoding, "不支持的编码, 无法预览".to_string());
    };

    let (text, truncated) = read_prefix(&mut src_file, enc, PREVIEW_READ_BYTES);
    let parsed = parse_preview(&text, sample_rows, truncated);

    FilePreview {
        path: abs_path,
        relative_path,
        size,
        encoding,
        needs_transcode,
        delimiter: parsed.delimiter.to_string(),
        headers: parsed.headers,
        rows: parsed.rows,
        columns_truncated: parsed.columns_truncated,
        error: None,
    }
}

/// 读取文件头部若干字节并解码为字符串。
///
/// 返回 `(文本, 是否因超出 max_bytes 而被截断)`。
///
/// 用一次性 `Encoding::decode` 而非流式 `Decoder::decode_to_string`:
/// 后者在这里会提前停止消费输入, 导致文件后半段被静默丢掉
/// (表现为「表头与首行正常、后续行缺字」)。
fn read_prefix(
    file: &mut std::fs::File,
    enc: &'static encoding_rs::Encoding,
    max_bytes: usize,
) -> (String, bool) {
    use std::io::Read;

    let mut buf = vec![0u8; max_bytes];
    let mut n = 0usize;
    while n < buf.len() {
        match file.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => break,
        }
    }
    let truncated = n == max_bytes;
    buf.truncate(n);

    // decode() 会自动去掉 BOM, 并把非法序列替换为 U+FFFD
    let (decoded, _, _) = enc.decode(&buf);
    let mut text = decoded.into_owned();

    // 缓冲区被切断时, 末尾可能残留半个多字节序列 → 一个替换字符, 去掉它
    if truncated && text.ends_with('\u{FFFD}') {
        text.pop();
    }

    (text, truncated)
}

/// `parse_preview` 的结果
struct Parsed {
    delimiter: char,
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    columns_truncated: bool,
}

/// 从文本头部解析出分隔符、表头与前 `sample_rows` 行数据
fn parse_preview(text: &str, sample_rows: usize, truncated: bool) -> Parsed {
    let delimiter = sniff_delimiter(text);

    // 多解析一条: 表头占一条
    let mut all = parse_rows(text, delimiter, sample_rows + 1);

    // 缓冲区被截断且末尾不是换行 → 最后一条记录可能被切了一半, 丢弃
    if truncated && !text.ends_with('\n') && !all.is_empty() {
        all.pop();
    }

    let mut headers = all.first().cloned().unwrap_or_default();
    let mut rows: Vec<Vec<String>> = all.into_iter().skip(1).take(sample_rows).collect();

    // 超宽表: 只保留前 MAX_COLUMNS 列
    let columns_truncated = headers.len() > MAX_COLUMNS;
    if columns_truncated {
        headers.truncate(MAX_COLUMNS);
    }

    // 行补齐/裁剪到表头宽度, 表格不会错位
    let width = headers.len();
    for r in rows.iter_mut() {
        r.truncate(width);
        r.resize(width, String::new());
    }

    // 单元格过长截断
    for h in headers.iter_mut() {
        *h = clip(h);
    }
    for r in rows.iter_mut() {
        for c in r.iter_mut() {
            *c = clip(c);
        }
    }

    Parsed {
        delimiter,
        headers,
        rows,
        columns_truncated,
    }
}

/// 在首个物理行内统计**引号外**的候选分隔符出现次数, 取最多者 (默认逗号)
fn sniff_delimiter(text: &str) -> char {
    const CANDIDATES: [char; 4] = [',', '\t', ';', '|'];

    let line = text.lines().next().unwrap_or("");
    let mut counts = [0usize; CANDIDATES.len()];
    let mut in_quotes = false;

    for ch in line.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
            continue;
        }
        if in_quotes {
            continue;
        }
        for (i, c) in CANDIDATES.iter().enumerate() {
            if ch == *c {
                counts[i] += 1;
            }
        }
    }

    let mut best = CANDIDATES[0];
    let mut best_n = 0usize;
    for (i, c) in CANDIDATES.iter().enumerate() {
        if counts[i] > best_n {
            best_n = counts[i];
            best = *c;
        }
    }
    best
}

/// 极简 RFC4180 解析: 支持引号包裹、`""` 转义、引号内的分隔符与换行。
///
/// 解析到 `max_rows` 条记录即停止 (预览不需要全量, 顺带避免大文件开销)。
fn parse_rows(text: &str, delimiter: char, max_rows: usize) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    // "" → 字面量 "
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(ch);
            }
            continue;
        }

        match ch {
            // 引号只在字段起始处开启引用
            '"' if field.is_empty() => in_quotes = true,
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                if rows.len() >= max_rows {
                    return rows;
                }
            }
            c if c == delimiter => row.push(std::mem::take(&mut field)),
            c => field.push(c),
        }
    }

    // 收尾: 最后一条记录可能没有换行结尾
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }

    rows
}

/// 单元格文本裁剪: 超过 `MAX_CELL_CHARS` 时截断并补省略号
fn clip(s: &str) -> String {
    if s.chars().count() <= MAX_CELL_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX_CELL_CHARS).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_common_delimiters() {
        assert_eq!(sniff_delimiter("a,b,c\n1,2,3"), ',');
        assert_eq!(sniff_delimiter("a\tb\tc\n1\t2\t3"), '\t');
        assert_eq!(sniff_delimiter("a;b;c"), ';');
        assert_eq!(sniff_delimiter("a|b|c"), '|');
        // 无任何候选分隔符 → 默认逗号
        assert_eq!(sniff_delimiter("single"), ',');
        assert_eq!(sniff_delimiter(""), ',');
    }

    #[test]
    fn sniff_ignores_delimiters_inside_quotes() {
        // 引号内逗号不计入, 真正的分隔符是分号
        assert_eq!(sniff_delimiter("\"a,b,c,d\";x;y"), ';');
    }

    #[test]
    fn parses_quoted_fields_with_embedded_delimiter_and_newline() {
        let rows = parse_rows("\"a,1\",b\n\"x\ny\",z\n", ',', 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["a,1", "b"]);
        assert_eq!(rows[1], vec!["x\ny", "z"]);
    }

    #[test]
    fn unescapes_doubled_quotes() {
        let rows = parse_rows("\"he said \"\"hi\"\"\",2\n", ',', 10);
        assert_eq!(rows[0], vec!["he said \"hi\"", "2"]);
    }

    #[test]
    fn stops_after_max_rows() {
        let rows = parse_rows("h\n1\n2\n3\n4\n5\n6\n", ',', 3);
        assert_eq!(rows.len(), 3);
        // 只取前 3 条记录: h / 1 / 2
        assert_eq!(rows.last().unwrap(), &vec!["2".to_string()]);
    }

    #[test]
    fn handles_crlf_and_missing_trailing_newline() {
        let rows = parse_rows("a,b\r\n1,2", ',', 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["a", "b"]);
        assert_eq!(rows[1], vec!["1", "2"]);
    }

    #[test]
    fn preview_keeps_header_plus_sample_rows_and_pads_ragged_rows() {
        let p = parse_preview("h1,h2,h3\n1,2,3\n4\n", 5, false);
        assert_eq!(p.headers, vec!["h1", "h2", "h3"]);
        assert_eq!(p.rows.len(), 2);
        // 缺列补空串
        assert_eq!(p.rows[1], vec!["4", "", ""]);
    }

    #[test]
    fn preview_drops_partial_last_row_only_when_truncated() {
        let text = "h\n1\n2\n3"; // 末尾无换行 → 可能被切断

        let p = parse_preview(text, 5, true);
        assert_eq!(p.headers, vec!["h"]);
        assert_eq!(p.rows, vec![vec!["1"], vec!["2"]]);

        // 未截断 (小文件已读到 EOF) 时保留
        let p2 = parse_preview(text, 5, false);
        assert_eq!(p2.rows.len(), 3);
    }

    #[test]
    fn preview_truncates_wide_tables() {
        let header: Vec<String> = (0..MAX_COLUMNS + 5).map(|i| format!("c{i}")).collect();
        let text = format!("{}\n", header.join(","));

        let p = parse_preview(&text, 5, false);
        assert!(p.columns_truncated);
        assert_eq!(p.headers.len(), MAX_COLUMNS);
    }

    #[test]
    fn clips_overlong_cells() {
        let long = "x".repeat(MAX_CELL_CHARS + 50);
        let p = parse_preview(&format!("h\n{long}\n"), 5, false);

        let cell = &p.rows[0][0];
        assert!(cell.ends_with('…'));
        assert_eq!(cell.chars().count(), MAX_CELL_CHARS + 1);
    }

    #[test]
    fn empty_input_yields_no_headers_or_rows() {
        let p = parse_preview("", 5, false);
        assert!(p.headers.is_empty());
        assert!(p.rows.is_empty());
        assert!(!p.columns_truncated);
    }
}
