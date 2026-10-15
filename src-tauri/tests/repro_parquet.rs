//! 回归复现 (已修复): SQL 无法查询 parquet 文件.
//!
//! 原缺陷: `DATA_EXTS` 只含 csv/tsv/txt/psv, `FROM 'x.parquet'` 不会被重写为
//! 绝对路径, DuckDB 按进程 CWD 解析 → "IO Error: No files found".
//!
//! 修复后由 `parquet_query.rs` 做完整覆盖; 本文件保留最小回归断言:
//! 一旦有人把 parquet 从 `BINARY_EXTS` 移除, 这里会立刻失败.

use csv_utf8_converter_lib::sql;
use std::fs;

fn tmp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("csv_utf8_pqreg_{name}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// 曾经的失败用例: 相对路径 parquet 字面量
#[test]
fn parquet_relative_literal_no_longer_fails() {
    let d = tmp_dir("direct");
    fs::write(d.join("src.csv"), "id,name\n1,张三\n").unwrap();
    sql::export_sql_result(&d, "SELECT * FROM 'src.csv'", &d.join("data.parquet"), true, false)
        .expect("造 parquet");

    // 修复前这里会失败 (IO Error: No files found)
    let r = sql::run_sql(&d, "SELECT * FROM 'data.parquet'", 10)
        .expect("parquet 相对路径查询应已修复");
    assert_eq!(r.row_count, 1);
    assert_eq!(r.rows[0][1], "张三");
    println!("✓ 回归通过: parquet 相对路径可查询");
}

/// 对照: CSV 相对路径一直可用 (确保修复没有破坏原有行为)
#[test]
fn csv_relative_still_works() {
    let d = tmp_dir("csv");
    fs::write(d.join("t.csv"), "id,name\n1,张三\n").unwrap();
    let r = sql::run_sql(&d, "SELECT * FROM 't.csv'", 10).expect("CSV 应可用");
    assert_eq!(r.row_count, 1);
    println!("✓ CSV 相对路径正常");
}
