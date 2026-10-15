//! 让 DuckDB 自身 INSTALL 缺失的 extensions (走真实下载 endpoint)
//! 完成后人工从 C:\Users\admin\.duckdb\extensions\v1.5.6\windows_amd64\ 复制到项目内

use duckdb::Connection;

#[test]
fn install_missing_extensions() {
    let conn = Connection::open_in_memory().expect("open in-memory");

    for name in &["parquet", "icu", "httpfs", "fts"] {
        println!("-- INSTALL {} --", name);
        match conn.execute_batch(&format!("INSTALL {name};")) {
            Ok(_) => {
                let path: String = conn
                    .query_row(
                        "SELECT install_path FROM duckdb_extensions() WHERE extension_name = ?",
                        [name],
                        |r| r.get(0),
                    )
                    .expect("query install_path");
                println!("  ✓ {name} -> {path}");
            }
            Err(e) => println!("  ✗ {name}: {e}"),
        }
    }

    // 列出所有已落盘文件
    println!("\n=== cache 目录最终状态 ===");
    for n in &["json", "parquet", "icu", "httpfs", "fts", "encodings"] {
        let p: Option<String> = conn
            .query_row(
                "SELECT install_path FROM duckdb_extensions() WHERE extension_name = ?",
                [n],
                |r| r.get(0),
            )
            .ok();
        match p {
            Some(path) if !path.is_empty() => {
                let meta = std::fs::metadata(&path).ok();
                let sz = meta.map(|m| m.len() / 1024).unwrap_or(0);
                println!("  {n:<10} {sz:>8} KB  {path}");
            }
            _ => println!("  {n:<10} (未安装)"),
        }
    }
}
