//! 实验: bundled build 实际可用哪些 DuckDB Extension
//!
//! 测三件事:
//! 1. 系统表 duckdb_extensions() 返回的列表 (内置 / 已安装 / 已加载)
//! 2. LOAD json (内置, 不需网络) 是否能直接用
//! 3. INSTALL httpfs FROM community; LOAD httpfs; (需网络访问 extensions.duckdb.org)

use duckdb::Connection;

fn print_extensions(label: &str) {
    let conn = Connection::open_in_memory().expect("open in-memory");
    // 真实列名未知, 用 SELECT * 兜底, 然后按列名解析
    let mut stmt = conn
        .prepare("SELECT * FROM duckdb_extensions() ORDER BY extension_name")
        .expect("prepare");
    let mut rows = stmt.query([]).expect("query");
    let cols: Vec<String> = rows.as_ref().unwrap().column_names();
    println!("\n=== {} (列: {:?}) ===", label, cols);

    let mut n_total = 0;
    let mut n_loaded = 0;
    let mut n_installed = 0;
    while let Some(row) = rows.next().expect("next") {
        n_total += 1;
        // 关键列用 String 兜底, 拿不到就 "?"
        for (i, c) in cols.iter().enumerate() {
            let v: String = row.get::<usize, String>(i).unwrap_or_else(|_| "?".into());
            if c == "loaded" && v == "true" { n_loaded += 1; }
            if c == "installed" && v == "true" { n_installed += 1; }
            print!("{}={}  ", c, v);
        }
        println!();
    }
    println!("==> 共 {} 个, {} 个 installed, {} 个 loaded (内置)\n",
        n_total, n_installed, n_loaded);
}

#[test]
fn list_extensions_and_load_json() {
    print_extensions("duckdb_extensions() 实际状态");

    // bundled build 没有任何 extension 是静态链接的, 必须先 INSTALL (从网络下载)
    println!("=== 尝试从网络 INSTALL json (验证网络可达) ===");
    let conn = Connection::open_in_memory().expect("open");
    match conn.execute_batch("INSTALL json;") {
        Ok(_) => {
            println!("✓ INSTALL json 成功 (网络可达 extensions.duckdb.org)");
            conn.execute_batch("LOAD json;").expect("LOAD");
            let v: String = conn
                .query_row("SELECT json('[1,2,3]')::VARCHAR", [], |r| r.get(0))
                .expect("json 函数应可用");
            // 格式可能是 "[1,2,3]" 或 "[1, 2, 3]", 含数字即可
            assert!(v.contains('1') && v.contains('2') && v.contains('3'), "got: {}", v);
            println!("✓ json('[1,2,3]') = {}", v);
        }
        Err(e) => {
            println!("✗ INSTALL json 失败: {}\n", e);
            println!("可能原因:");
            println!("  1. 网络无法访问 https://community-extensions.duckdb.org/");
            println!("  2. 防火墙/代理拦截");
            println!("  3. 走错默认仓库 (DuckDB CLI 默认 = community, 可改 SET custom_extension_repository)");
        }
    }

    // 验证 INSTALL 后 cached 路径
    let cache = std::path::Path::new(r"C:\Users\admin\.duckdb\extensions");
    if cache.exists() {
        println!("\n=== 本地 extension 缓存目录 (std::fs 递归) ===");
        fn walk(p: &std::path::Path, depth: usize) {
            if depth > 4 { return; }
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    let path = e.path();
                    if path.is_file() && path.extension().map_or(false, |x| x == "duckdb_extension") {
                        println!("  {} ({} KB)", path.display(), e.metadata().map(|m| m.len()/1024).unwrap_or(0));
                    } else if path.is_dir() {
                        walk(&path, depth + 1);
                    }
                }
            }
        }
        walk(cache, 0);
    }
}
