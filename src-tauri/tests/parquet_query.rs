//! Parquet 查询支持测试.
//!
//! 背景: 原先 `DATA_EXTS` 只含 csv/tsv/txt/psv, 导致 `FROM 'x.parquet'` 不会被
//! 重写为绝对路径, DuckDB 按进程 CWD 解析 → "IO Error: No files found".
//! 修复后 parquet/pq 作为"二进制类"仅重写路径, 不转码.

use csv_utf8_converter_lib::sql;
use std::fs;
use std::path::{Path, PathBuf};

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("csv_utf8_pq_{name}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// 造一个 parquet 文件 (CSV → COPY TO parquet)
fn make_parquet(d: &Path, name: &str, csv_body: &str) -> PathBuf {
    fs::write(d.join("src.csv"), csv_body).unwrap();
    let out = d.join(name);
    sql::export_sql_result(d, "SELECT * FROM 'src.csv'", &out, true, false).expect("造 parquet");
    out
}

/// 核心: 相对路径直接 FROM 'x.parquet' 现在应该可用
#[test]
fn parquet_relative_literal_works() {
    let d = tmp_dir("direct");
    make_parquet(&d, "data.parquet", "id,name\n1,张三\n2,李四\n");

    let r = sql::run_sql(&d, "SELECT * FROM 'data.parquet' ORDER BY id", 10)
        .expect("相对路径查 parquet 应成功");

    assert_eq!(r.columns, vec!["id", "name"]);
    assert_eq!(r.row_count, 2);
    assert_eq!(r.rows[0][1], "张三", "中文应正确");
    println!("✓ FROM 'data.parquet' 可用: {:?}", r.rows);
}

/// `.pq` 简写: **DuckDB 本身只按 `.parquet` 扩展名分派读取器**,
/// 因此 `FROM 'data.pq'` 会报 "No extension found that is capable of reading"。
/// 这是 DuckDB 的既定行为, 不是本项目的缺陷。
/// 但通过显式 `read_parquet()` 可以读取 `.pq`, 且相对路径重写依然生效。
#[test]
fn pq_extension_only_via_read_parquet() {
    let d = tmp_dir("pq");
    make_parquet(&d, "data.pq", "id,v\n1,a\n");

    // 直接 FROM 'x.pq' → DuckDB 拒绝 (扩展名分派失败)
    let direct = sql::run_sql(&d, "SELECT * FROM 'data.pq'", 10);
    assert!(
        direct.is_err(),
        "DuckDB 不支持按 .pq 扩展名自动分派, 预期失败"
    );
    let msg = format!("{:#}", direct.unwrap_err());
    assert!(
        msg.contains("No extension found") || msg.contains("capable of reading"),
        "错误应符合 DuckDB 的扩展名分派行为: {msg}"
    );

    // 显式 read_parquet() → 可用, 且相对路径被正确重写
    let r = sql::run_sql(&d, "SELECT * FROM read_parquet('data.pq')", 10)
        .expect("read_parquet('data.pq') 应可用");
    assert_eq!(r.row_count, 1);
    assert_eq!(r.rows[0][1], "a");
    println!("✓ .pq 需显式 read_parquet(); 相对路径重写生效");
}

/// 子目录相对路径
#[test]
fn parquet_in_subdir_works() {
    let d = tmp_dir("subdir");
    let sub = d.join("nested");
    fs::create_dir_all(&sub).unwrap();
    make_parquet(&sub, "data.parquet", "id,name\n1,王五\n");

    let r = sql::run_sql(&d, "SELECT * FROM 'nested/data.parquet'", 10)
        .expect("子目录相对路径应可用");
    assert_eq!(r.rows[0][1], "王五");
    println!("✓ 子目录 'nested/data.parquet' 可用");
}

/// 绝对路径
#[test]
fn parquet_absolute_path_works() {
    let d = tmp_dir("abs");
    let pq = make_parquet(&d, "data.parquet", "id\n7\n");
    let abs = pq.to_string_lossy().replace('\\', "/");
    let r = sql::run_sql(&d, &format!("SELECT * FROM '{abs}'"), 10).expect("绝对路径应可用");
    assert_eq!(r.rows[0][0], "7");
    println!("✓ 绝对路径可用");
}

/// read_parquet() 相对路径也应可用 (同样走重写)
#[test]
fn read_parquet_function_relative_works() {
    let d = tmp_dir("func");
    make_parquet(&d, "data.parquet", "id,name\n1,张三\n");
    let r = sql::run_sql(&d, "SELECT * FROM read_parquet('data.parquet')", 10)
        .expect("read_parquet 相对路径应可用");
    assert_eq!(r.row_count, 1);
    println!("✓ read_parquet('data.parquet') 可用");
}

/// parquet 与 csv 可以 JOIN (混合查询)
#[test]
fn parquet_joins_with_csv() {
    let d = tmp_dir("join");
    make_parquet(&d, "a.parquet", "id,name\n1,张三\n2,李四\n");
    fs::write(d.join("b.csv"), "id,city\n1,北京\n2,上海\n").unwrap();

    let r = sql::run_sql(
        &d,
        "SELECT a.name, b.city FROM 'a.parquet' a JOIN 'b.csv' b ON a.id = b.id ORDER BY a.id",
        10,
    )
    .expect("parquet 与 csv JOIN 应成功");

    assert_eq!(r.row_count, 2);
    assert_eq!(r.rows[0][0], "张三");
    assert_eq!(r.rows[0][1], "北京");
    println!("✓ parquet JOIN csv 可用: {:?}", r.rows);
}

/// 通配多文件: dir/*.parquet (应能识别存在并重写)
#[test]
fn parquet_glob_works() {
    let d = tmp_dir("glob");
    fs::write(d.join("s.csv"), "id,v\n1,a\n").unwrap();
    fs::write(d.join("s2.csv"), "id,v\n2,b\n").unwrap();
    // 造两个 parquet
    sql::export_sql_result(
        &d,
        "SELECT * FROM 's.csv'",
        &d.join("p1.parquet"),
        true,
        false,
    )
    .unwrap();
    sql::export_sql_result(
        &d,
        "SELECT * FROM 's2.csv'",
        &d.join("p2.parquet"),
        true,
        false,
    )
    .unwrap();

    let r = sql::run_sql(&d, "SELECT count(*)::VARCHAR AS n FROM 'p*.parquet'", 10)
        .expect("parquet 通配应可用");
    assert_eq!(r.rows[0][0], "2", "两个文件各 1 行, 合计 2");
    println!("✓ parquet 通配 'p*.parquet' 可用, 匹配 2 行");
}

/// parquet 里查不到中文乱码 (编码不应被破坏)
#[test]
fn parquet_chinese_intact() {
    let d = tmp_dir("cn");
    make_parquet(
        &d,
        "cn.parquet",
        "id,name,city\n1,张三,北京\n2,李四,上海\n3,王五,广州\n",
    );
    let r = sql::run_sql(&d, "SELECT name, city FROM 'cn.parquet' ORDER BY id", 10).unwrap();
    assert_eq!(r.rows[0][1], "北京");
    assert_eq!(r.rows[2][0], "王五");
    // 不应出现替换字符
    for row in &r.rows {
        for cell in row {
            assert!(!cell.contains('\u{FFFD}'), "不应有乱码: {cell}");
        }
    }
    println!("✓ parquet 中文完好: {:?}", r.rows);
}

/// 不存在的 parquet 应报错而非静默
#[test]
fn missing_parquet_reports_error() {
    let d = tmp_dir("missing");
    let err = sql::run_sql(&d, "SELECT * FROM 'nope.parquet'", 10)
        .expect_err("不存在的 parquet 应报错");
    let msg = format!("{err:#}");
    println!("✓ 缺失 parquet 报错: {}", msg.lines().next().unwrap_or(""));
    assert!(!msg.is_empty());
}

/// 回归: CSV 转码链路不受影响
#[test]
fn csv_transcode_still_works() {
    let d = tmp_dir("csv_regress");
    let (bytes, _, _) = encoding_rs::GB18030.encode("id,name\n1,张三\n");
    fs::write(d.join("g.csv"), bytes.as_ref()).unwrap();

    let r = sql::run_sql(&d, "SELECT * FROM 'g.csv'", 10).expect("GBK CSV 应可用");
    assert_eq!(r.rows[0][1], "张三");
    assert!(!r.transcoded.is_empty(), "GBK 文件应被转码");
    println!("✓ CSV 转码回归正常, 转码文件: {:?}", r.transcoded);
}
