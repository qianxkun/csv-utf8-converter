//! 文件预览端到端测试
//!
//! 造一个 GBK(含中文表头) 与一个 UTF-8 的 CSV, 验证:
//! - 编码检测结果正确, 且「需转码」判定与之一致
//! - 表头与前 N 行数据被正确解析 (中文不乱码)
//! - 已是 UTF-8 的文件不标记为需转码

use csv_utf8_converter_lib::detector::DetectedEncoding;
use csv_utf8_converter_lib::preview;
use std::collections::HashMap;
use std::fs;

/// 用进程号隔离临时目录, 避免并行测试互相干扰
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("csvutf8-{}-{}", tag, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

#[test]
fn previews_detect_encoding_and_parse_header_and_rows() {
    let dir = temp_dir("preview");

    // GBK 编码 (中文表头 + 中文数据)
    let gbk_text = "姓名,城市,金额\n张三,北京,100\n李四,上海,200\n";
    let (gbk_bytes, _, had_errors) = encoding_rs::GB18030.encode(gbk_text);
    assert!(!had_errors, "GB18030 编码不应产生替换字符");
    fs::write(dir.join("gbk.csv"), &gbk_bytes).expect("写入 gbk.csv");

    // 已是 UTF-8
    fs::write(dir.join("utf8.csv"), "a,b\n1,2\n3,4\n").expect("写入 utf8.csv");

    let files = vec![dir.join("gbk.csv"), dir.join("utf8.csv")];
    let result = preview::build(&dir, files, 5, 65536, &HashMap::new());

    assert_eq!(result.count, 2);
    assert_eq!(result.needs_transcode, 1, "只有 GBK 文件需要转码");
    assert!(result.total_bytes > 0);

    let gbk = result
        .files
        .iter()
        .find(|f| f.relative_path == "gbk.csv")
        .expect("应包含 gbk.csv");
    assert!(gbk.needs_transcode);
    assert_eq!(gbk.encoding, "GB18030");
    assert_eq!(gbk.delimiter, ",");
    assert_eq!(gbk.headers, vec!["姓名", "城市", "金额"]);
    assert_eq!(gbk.rows.len(), 2);
    assert_eq!(gbk.rows[0], vec!["张三", "北京", "100"]);
    assert_eq!(gbk.rows[1], vec!["李四", "上海", "200"]);
    assert!(gbk.error.is_none());

    let utf8 = result
        .files
        .iter()
        .find(|f| f.relative_path == "utf8.csv")
        .expect("应包含 utf8.csv");
    assert!(!utf8.needs_transcode, "UTF-8 文件不应被标记为需转码");
    assert_eq!(utf8.headers, vec!["a", "b"]);
    assert_eq!(utf8.rows.len(), 2);

    let _ = fs::remove_dir_all(&dir);
}

/// 用户手动指定编码时: 生效编码取指定值, 自动探测结果仍然回报, 且判定随之变化。
#[test]
fn forced_encoding_overrides_detection_and_is_reported() {
    let dir = temp_dir("preview-forced");

    let gbk_text = "姓名,城市\n张三,北京\n";
    let (gbk_bytes, _, had_errors) = encoding_rs::GB18030.encode(gbk_text);
    assert!(!had_errors);
    let path = dir.join("gbk.csv");
    fs::write(&path, &gbk_bytes).expect("写入 gbk.csv");

    // 1) 不指定: 自动探测为 GB18030
    let auto = preview::build(&dir, vec![path.clone()], 5, 65536, &HashMap::new());
    let a = &auto.files[0];
    assert_eq!(a.encoding, "GB18030");
    assert_eq!(a.encoding_id, "gb18030");
    assert!(!a.overridden);
    assert!(a.needs_transcode);

    // 2) 强制按 UTF-8 解读: 生效编码变了, 不再"需转码", 中文必然变成替换字符
    let mut overrides = HashMap::new();
    overrides.insert(path.clone(), DetectedEncoding::Utf8);

    let forced = preview::build(&dir, vec![path.clone()], 5, 65536, &overrides);
    let f = &forced.files[0];
    assert_eq!(f.encoding, "UTF-8");
    assert_eq!(f.encoding_id, "utf8");
    assert_eq!(f.encoding_auto, "GB18030", "自动探测结果仍要回报给界面");
    assert!(f.overridden);
    assert!(!f.needs_transcode, "指定为 UTF-8 后不应再标记需转码");
    assert!(
        f.headers.join("").contains('\u{FFFD}'),
        "把 GBK 字节当 UTF-8 解会出现替换字符 (反过来 UTF-8 当 GB18030 解则是乱码而非替换字符)"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn preview_respects_sample_rows_limit_and_subdirectories() {
    let dir = temp_dir("preview-sub");
    fs::create_dir_all(dir.join("sub")).expect("创建子目录");

    // 表头 + 8 行数据
    let mut text = String::from("id,val\n");
    for i in 1..=8 {
        text.push_str(&format!("{i},v{i}\n"));
    }
    fs::write(dir.join("sub").join("many.csv"), text).expect("写入 many.csv");

    let files = vec![dir.join("sub").join("many.csv")];
    let result = preview::build(&dir, files, 3, 65536, &HashMap::new());

    let f = &result.files[0];
    // 相对路径保留子目录 (用 Path 比较, 避免平台分隔符差异)
    assert_eq!(
        std::path::Path::new(&f.relative_path),
        std::path::Path::new("sub").join("many.csv")
    );
    assert_eq!(f.headers, vec!["id", "val"]);
    assert_eq!(f.rows.len(), 3, "sample_rows=3 时只返回 3 行数据");
    assert_eq!(f.rows[0], vec!["1", "v1"]);
    assert_eq!(f.rows[2], vec!["3", "v3"]);

    let _ = fs::remove_dir_all(&dir);
}
