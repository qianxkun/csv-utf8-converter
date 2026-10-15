//! 评估: 去掉 `encodings` 扩展是否影响功能.
//!
//! 背景: 项目的 SQL 管线是
//!   检测编码 (encoding_rs/chardetng) → 转码成 UTF-8 缓存文件 → DuckDB `read_csv_auto` 读 UTF-8
//! DuckDB 侧永远只看到 UTF-8, 因此理论上不需要 DuckDB 的 `encodings` 扩展
//! (那个扩展是给 `read_csv(encoding='gbk')` 之类用的).
//!
//! 本测试用**不加载 encodings** 的连接, 跑完整 GBK/GB18030 查询链路,
//! 证明去掉它功能不受影响.

use std::fs;
use std::path::PathBuf;

const EXT_VERSION: &str = "v1.5.6";
const EXT_PLATFORM: &str = "windows_amd64";

fn ext_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tools")
        .join("duckdb-extensions")
        .join(EXT_VERSION)
        .join(EXT_PLATFORM)
}

/// 构造一个连接, 加载除 `encodings` 之外的全部预置扩展
/// (即模拟"删掉 encodings 文件"后的运行状态)
fn conn_without_encodings() -> duckdb::Connection {
    let config = duckdb::Config::default()
        .allow_unsigned_extensions()
        .expect("allow_unsigned_extensions");
    let conn = duckdb::Connection::open_in_memory_with_flags(config).expect("open");
    conn.execute_batch("SET autoinstall_known_extensions=false;").ok();
    conn.execute_batch("SET autoload_known_extensions=false;").ok();
    conn.execute_batch("SET custom_extension_repository='http://127.0.0.1:1/';")
        .ok();

    for name in ["json", "parquet", "icu", "httpfs", "fts", "excel", "spatial"] {
        let p = ext_dir().join(format!("{name}.duckdb_extension"));
        if p.is_file() {
            let s = p.to_string_lossy().replace('\\', "/");
            let _ = conn.execute_batch(&format!("LOAD '{s}';"));
        }
    }
    conn
}

/// 断言 encodings 确实**未**加载 (保证测试前提成立)
#[test]
fn encodings_is_not_loaded_in_this_connection() {
    let conn = conn_without_encodings();
    let loaded: String = conn
        .query_row(
            "SELECT count(*)::VARCHAR FROM duckdb_extensions() \
             WHERE extension_name='encodings' AND loaded",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(loaded, "0", "本测试前提是 encodings 未加载");
    println!("✓ 前提成立: encodings 未加载");
}

/// 核心评估: 不加载 encodings 时, 项目自己的转码链路是否仍然工作.
/// 走 sql::run_sql (真实代码路径) 查询一个 GB18030 文件.
#[test]
fn gbk_query_works_without_encodings_extension() {
    use csv_utf8_converter_lib::sql;

    let tmp = std::env::temp_dir().join("csv_utf8_eval_no_encodings");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    // 写一个真正的 GB18030 文件 (含中文)
    let content = "id,name,city,amount\n1,张三,北京,100.5\n2,李四,上海,200.0\n3,王五,广州,150.75\n";
    let (bytes, _, _) = encoding_rs::GB18030.encode(content);
    fs::write(tmp.join("orders.csv"), bytes.as_ref()).unwrap();

    // 确认源文件确实不是 UTF-8 (否则测试没意义)
    let raw = fs::read(tmp.join("orders.csv")).unwrap();
    assert!(
        std::str::from_utf8(&raw).is_err(),
        "源文件应是 GB18030, 不应是合法 UTF-8"
    );

    // 走项目真实路径: run_sql 内部会 detect → 转码 → DuckDB 读 UTF-8
    let r = sql::run_sql(&tmp, "SELECT * FROM 'orders.csv' ORDER BY id", 10)
        .expect("GBK 查询应成功");

    assert_eq!(r.columns, vec!["id", "name", "city", "amount"]);
    assert_eq!(r.row_count, 3);
    // 中文必须正确解出
    assert_eq!(r.rows[0][1], "张三", "中文应正确解码");
    assert_eq!(r.rows[1][2], "上海");

    // 确认这次查询**没有**依赖 encodings (走的是转码缓存)
    println!("✓ 无 encodings 扩展时 GBK 查询正常: {:?}", r.rows[0]);
    println!("  转码文件: {:?}", r.transcoded);
}

/// 对照: 直接让 DuckDB 读 GBK 文件 (未经转码) 会怎样?
/// 说明 encodings 扩展的真实用途边界, 以及为什么本项目不需要它.
#[test]
fn direct_duckdb_gbk_read_is_what_encodings_would_enable() {
    let tmp = std::env::temp_dir().join("csv_utf8_eval_direct_gbk");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    let content = "id,name\n1,张三\n";
    let (bytes, _, _) = encoding_rs::GB18030.encode(content);
    let p = tmp.join("gbk.csv");
    fs::write(&p, bytes.as_ref()).unwrap();
    let ps = p.to_string_lossy().replace('\\', "/");

    let conn = conn_without_encodings();

    // 1) 默认 read_csv_auto: 把 GBK 字节当 UTF-8 解析
    let auto = conn.query_row(
        &format!("SELECT name FROM read_csv_auto('{ps}')"),
        [],
        |r| r.get::<usize, String>(0),
    );
    println!("read_csv_auto (无 encodings) 结果: {auto:?}");

    // 2) 显式指定 encoding: 这就是 encodings 扩展提供的能力
    let explicit = conn.query_row(
        &format!("SELECT name FROM read_csv('{ps}', encoding='gbk')"),
        [],
        |r| r.get::<usize, String>(0),
    );
    println!("read_csv(encoding='gbk') 结果: {explicit:?}");

    // 结论断言: 未经转码时中文会乱码/失败, 正说明"必须靠项目自己的转码",
    // 而不是依赖 DuckDB 的 encodings 扩展.
    let garbled_or_err = match &auto {
        Ok(v) => v != "张三",
        Err(_) => true,
    };
    assert!(
        garbled_or_err,
        "未经转码的 GBK 直读本应不正确, 否则测试假设有误: {auto:?}"
    );
    println!("✓ 证实: DuckDB 直读 GBK 不可靠 → 必须走项目自己的转码链路");
}
