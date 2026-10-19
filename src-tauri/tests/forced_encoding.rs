//! 验证「手动指定编码」在**转换阶段**同样生效, 而不只是预览阶段。
//!
//! 这是该功能最容易出错的地方: 预览按用户选的编码解读了, 转换却仍走自动探测,
//! 用户会看到「预览正常、产物乱码」。

use csv_utf8_converter_lib::converter::{convert, ConvertOptions};
use csv_utf8_converter_lib::detector::DetectedEncoding;
use std::fs;
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("csvutf8-{}-{}", tag, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn opts(forced: Option<DetectedEncoding>) -> ConvertOptions {
    ConvertOptions {
        probe_size: 65536,
        write_bom: false,
        retries: 0,
        skip_utf8: false,
        forced_encoding: forced,
    }
}

#[test]
fn forced_encoding_is_honored_by_conversion() {
    let dir = temp_dir("forced-encoding");

    // 造一个 GBK 文件: 自动探测会得到 GB18030
    let text = "姓名,城市\n张三,北京\n";
    let (bytes, _, had_errors) = encoding_rs::GB18030.encode(text);
    assert!(!had_errors);
    let src = dir.join("gbk.csv");
    fs::write(&src, &bytes).expect("写入 gbk.csv");

    // 1) 自动: 产物应为正确中文
    let auto_out = dir.join("auto.csv");
    convert(&src, &auto_out, &opts(None)).expect("自动转换");
    let auto = fs::read_to_string(&auto_out).expect("读取产物");
    assert!(auto.contains("张三"), "自动探测应还原中文, 实际: {auto:?}");
    assert!(auto.contains("北京"));

    // 2) 强制按 UTF-8 解读: 产物必然出现替换字符 —— 这正是 forced 生效的证据
    let forced_out = dir.join("forced.csv");
    convert(
        &src,
        &forced_out,
        &opts(Some(DetectedEncoding::Utf8)),
    )
    .expect("强制转换");
    let forced = fs::read_to_string(&forced_out).expect("读取产物");
    assert!(
        forced.contains('\u{FFFD}'),
        "把 GBK 当 UTF-8 转应产生替换字符, 实际: {forced:?}"
    );
    assert!(
        !forced.contains("张三"),
        "强制 UTF-8 后不应还原出中文, 实际: {forced:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn forced_encoding_overrides_a_wrong_autodetect() {
    let dir = temp_dir("forced-fix");

    // 造一个 UTF-8 文件, 但强制按 GB18030 解读:
    // 用来证明 forced 完全绕开了自动探测 (自动探测会正确判为 UTF-8)。
    //
    // 注意: UTF-8 中文按 GB18030 解**不会**产生 U+FFFD —— GB18030 双字节几乎覆盖
    // 所有组合, 结果是「看起来像中文的乱码」(如 姓名 → 濮撳悕)。反过来
    // (GBK 字节按 UTF-8 解) 才会出现 U+FFFD。所以「有没有替换字符」不能作为
    // 判断编码是否选对的可靠依据, 只能作为其中一种信号。
    let original = "姓名,城市\n张三,北京\n";
    let src = dir.join("u8.csv");
    fs::write(&src, original).expect("写入 u8.csv");

    let out = dir.join("misread.csv");
    convert(&src, &out, &opts(Some(DetectedEncoding::Gb18030))).expect("转换");

    let s = fs::read_to_string(&out).expect("读取产物");
    assert_ne!(
        s, original,
        "强制用错误编码解读, 产物不应与原文件相同 (说明 forced 未生效)"
    );
    assert!(
        !s.contains("张三"),
        "按 GB18030 解 UTF-8 中文不可能还原出正确汉字, 实际: {s:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}
