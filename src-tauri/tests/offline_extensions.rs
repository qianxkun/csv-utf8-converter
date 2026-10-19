//! 离线扩展验证: 确认预置的 DuckDB extension 全部能从项目目录 LOAD, 且**不联网**.
//!
//! 验证点:
//! 1. 每个扩展文件都存在于 `tools/duckdb-extensions/<ver>/<platform>/`
//! 2. 每个都能按绝对路径 LOAD 成功 (版本与本 crate 捆绑的 DuckDB 匹配)
//! 3. LOAD 后关键函数真的可用 (不只是 LOAD 返回 Ok)
//! 4. 全程离线: 禁网时依旧成功
//!
//! 注: `encodings` 已被移除 (177 MB, 占载荷 48%, 而本项目管线永远只让 DuckDB 见 UTF-8).
//! 见 `eval_drop_encodings.rs`.
//!
//! 运行: `cargo test --test offline_extensions -- --nocapture`
//!
//! 注意: 若本机 DuckDB 扩展目录被清空/版本不符, 此测试会明确失败而不是静默跳过,
//! 这正是与 `sql.rs` 里"静默降级"的区别 —— 测试要暴露问题.

use std::path::PathBuf;

/// 与 sql.rs 中 PRELOAD_EXTENSIONS 保持一致
const EXPECTED: &[&str] = &["json", "parquet", "icu", "httpfs", "fts", "excel", "spatial"];

const EXT_VERSION: &str = "v1.5.6";
const EXT_PLATFORM: &str = "windows_amd64";

fn ext_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tools")
        .join("duckdb-extensions")
        .join(EXT_VERSION)
        .join(EXT_PLATFORM)
}

fn open_conn() -> duckdb::Connection {
    let config = duckdb::Config::default()
        .allow_unsigned_extensions()
        .expect("allow_unsigned_extensions");
    duckdb::Connection::open_in_memory_with_flags(config).expect("open in-memory")
}

/// 1. 文件齐备性: 8 个文件都在, 且是非空 PE 文件
#[test]
fn all_extension_files_present() {
    let dir = ext_dir();
    assert!(
        dir.is_dir(),
        "预置扩展目录不存在: {} (离线 SQL 无法工作)",
        dir.display()
    );

    let mut missing = Vec::new();
    for name in EXPECTED {
        let p = dir.join(format!("{name}.duckdb_extension"));
        if !p.is_file() {
            missing.push(format!("{name} (缺文件)"));
            continue;
        }
        let len = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        if len < 1024 {
            missing.push(format!("{name} (文件过小: {len} B)"));
            continue;
        }
        // Windows 动态库必须是 PE (MZ)
        let head = std::fs::read(&p).unwrap_or_default();
        if head.len() < 2 || &head[0..2] != b"MZ" {
            missing.push(format!("{name} (不是有效 PE 文件)"));
        }
    }
    assert!(
        missing.is_empty(),
        "以下扩展不可用: {}\n目录: {}",
        missing.join(", "),
        dir.display()
    );
    println!("✓ {} 个扩展文件齐备且均为有效 PE", EXPECTED.len());
}

/// 2. 逐个 LOAD 成功 (版本匹配是本测试的核心)
#[test]
fn every_extension_loads() {
    let dir = ext_dir();
    let conn = open_conn();
    let mut failures = Vec::new();

    for name in EXPECTED {
        let p = dir.join(format!("{name}.duckdb_extension"));
        if !p.is_file() {
            failures.push(format!("{name}: 文件缺失"));
            continue;
        }
        let path_str = p.to_string_lossy().replace('\\', "/");
        let sql = format!("LOAD '{path_str}';");
        match conn.execute_batch(&sql) {
            Ok(_) => println!("✓ LOAD {name}"),
            Err(e) => {
                println!("✗ LOAD {name}: {e}");
                failures.push(format!("{name}: {e}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "以下扩展 LOAD 失败 (通常是与捆绑 DuckDB 版本不匹配):\n  {}",
        failures.join("\n  ")
    );
}

/// 3. LOAD 之后, 每个扩展的核心函数真的能调用.
///    只验证 LOAD 返回 Ok 是不够的 —— 依赖缺失/符号问题时函数调用才暴露。
#[test]
fn loaded_extensions_actually_work() {
    let dir = ext_dir();
    let conn = open_conn();
    for name in EXPECTED {
        let p = dir.join(format!("{name}.duckdb_extension"));
        if !p.is_file() {
            continue;
        }
        let path_str = p.to_string_lossy().replace('\\', "/");
        conn.execute_batch(&format!("LOAD '{path_str}';")).ok();
    }

    // (扩展名, 验证 SQL, 期望结果包含)
    // 注意: 用 ::VARCHAR 显式转换, 否则 SELECT 1 返回 Int 而 row.get::<String> 会失败
    let probes: &[(&str, &str, &str)] = &[
        ("json", "SELECT json_extract('{\"a\":42}', '$.a')::VARCHAR", "42"),
        (
            "parquet",
            "SELECT count(*)::VARCHAR FROM duckdb_extensions() WHERE extension_name='parquet'",
            "1",
        ),
        ("icu", "SELECT length('中文字符串')::VARCHAR", "5"),
        (
            "httpfs",
            "SELECT count(*)::VARCHAR FROM duckdb_extensions() WHERE extension_name='httpfs' AND loaded",
            "1",
        ),
        (
            "fts",
            "SELECT count(*)::VARCHAR FROM duckdb_extensions() WHERE extension_name='fts' AND loaded",
            "1",
        ),
        (
            "excel",
            "SELECT count(*)::VARCHAR FROM duckdb_extensions() WHERE extension_name='excel' AND loaded",
            "1",
        ),
        ("spatial", "SELECT ST_AsText(ST_Point(1.0, 2.0))", "POINT"),
        (
            "excel_func",
            "SELECT count(*)::VARCHAR FROM duckdb_functions() WHERE function_name='read_xlsx'",
            "1",
        ),
        (
            "fts_func",
            "SELECT count(*)::VARCHAR FROM duckdb_functions() WHERE function_name='stem'",
            "1",
        ),
        (
            "httpfs_func",
            // read_blob 有多个重载, 只要存在 (非 0) 即可
            "SELECT (count(*) > 0)::VARCHAR FROM duckdb_functions() WHERE function_name='read_blob'",
            "true",
        ),
        (
            "spatial_func",
            "SELECT ST_GeometryType(ST_Point(1.0, 2.0))",
            "POINT",
        ),
    ];

    let mut failures = Vec::new();
    for (name, sql, expect) in probes {
        match conn.query_row(sql, [], |r| r.get::<usize, String>(0)) {
            Ok(v) => {
                if v.contains(expect) {
                    println!("✓ {name}: {v}");
                } else {
                    println!("✗ {name}: 得到 {v:?}, 期望含 {expect:?}");
                    failures.push(format!("{name}: {v:?} 不含 {expect:?}"));
                }
            }
            Err(e) => {
                println!("✗ {name}: 查询失败 {e}");
                failures.push(format!("{name}: {e}"));
            }
        }
    }
    assert!(failures.is_empty(), "扩展功能验证失败:\n  {}", failures.join("\n  "));
}

/// 5. 离线证明: 把 DuckDB 的扩展仓库指向不可达地址, 并禁用自动安装,
///    证明预置扩展的 LOAD **完全不依赖网络/INSTALL 回退**.
///
/// 做法: 每个扩展单独用一个新连接, 先设置
///   - `SET autoinstall_known_extensions=false`
///   - `SET autoload_known_extensions=false`
///   - `SET custom_extension_repository='http://127.0.0.1:1/'` (不可达)
/// 再按绝对路径 LOAD. 若某个扩展实际上依赖联网下载, 这里必然失败.
#[test]
fn offline_load_without_network_fallback() {
    let dir = ext_dir();
    let mut failures = Vec::new();

    for name in EXPECTED {
        let p = dir.join(format!("{name}.duckdb_extension"));
        if !p.is_file() {
            failures.push(format!("{name}: 文件缺失"));
            continue;
        }

        let config = duckdb::Config::default()
            .allow_unsigned_extensions()
            .expect("allow_unsigned_extensions");
        let conn = duckdb::Connection::open_in_memory_with_flags(config).expect("open");

        // 关掉一切联网回退能力
        for pragma in [
            "SET autoinstall_known_extensions=false;",
            "SET autoload_known_extensions=false;",
            // 指向不可达端口: 任何真实下载都会立刻失败
            "SET custom_extension_repository='http://127.0.0.1:1/';",
        ] {
            conn.execute_batch(pragma)
                .unwrap_or_else(|e| panic!("设置 {pragma} 失败: {e}"));
        }

        let path_str = p.to_string_lossy().replace('\\', "/");
        match conn.execute_batch(&format!("LOAD '{path_str}';")) {
            Ok(_) => {
                // 再确认确实处于 loaded 状态
                let loaded: String = conn
                    .query_row(
                        "SELECT count(*)::VARCHAR FROM duckdb_extensions() \
                         WHERE extension_name = ? AND loaded",
                        [name],
                        |r| r.get(0),
                    )
                    .unwrap_or_else(|_| "0".into());
                if loaded == "1" {
                    println!("✓ {name} 离线 LOAD 成功");
                } else {
                    println!("✗ {name} LOAD 返回 Ok 但未标记 loaded");
                    failures.push(format!("{name}: LOAD 后未 loaded"));
                }
            }
            Err(e) => {
                println!("✗ {name} 离线 LOAD 失败: {e}");
                failures.push(format!("{name}: {e}"));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "以下扩展无法在断网条件下加载 (说明依赖 INSTALL 回退):\n  {}",
        failures.join("\n  ")
    );
}
/// 6. 通过项目自身的 preload 路径验证 (走 sql.rs 的真实逻辑, 端到端)
#[test]
fn project_preload_reports_all_loaded() {
    use csv_utf8_converter_lib::sql;
    use std::fs;

    let tmp = std::env::temp_dir().join("csv_utf8_test_offline_ext");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("t.csv"), "a,b\n1,2\n").unwrap();

    let r = sql::run_sql(&tmp, "SELECT * FROM 't.csv'", 10).expect("query ok");

    let mut not_loaded = Vec::new();
    for s in &r.extensions {
        println!(
            "  {:<12} loaded={} present={} err={:?}",
            s.name, s.loaded, s.present, s.error
        );
        if !s.loaded {
            not_loaded.push(format!("{}: {:?}", s.name, s.error));
        }
    }
    assert!(
        not_loaded.is_empty(),
        "以下扩展未成功预加载 (离线 SQL 会退化联网):\n  {}",
        not_loaded.join("\n  ")
    );
    assert_eq!(r.extensions.len(), EXPECTED.len());

    let _ = fs::remove_dir_all(&tmp);
}
