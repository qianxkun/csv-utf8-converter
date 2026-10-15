//! SQL 引擎端到端测试: 验证对任意编码 CSV 直接跑 SQL
//!
//! 关键验证点:
//! 1. SQL 字符串字面量里的相对路径能解析到数据目录
//! 2. GB18030 编码文件被自动转码, 中文在结果中正确显示
//! 3. 列名与数据均按 SQL 解析
//! 4. 转码缓存生效

use csv_utf8_converter_lib::sql;
use encoding_rs::GB18030;
use std::fs;
use std::path::Path;

fn write_csv(path: &Path, content: &str, encoding: &'static encoding_rs::Encoding) {
    let (bytes, _, _) = encoding.encode(content);
    fs::write(path, bytes.as_ref()).unwrap();
}

/// 在目录下递归查找指定文件名
fn find_file(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, name) {
                return Some(f);
            }
        } else if p.file_name().and_then(|s| s.to_str()) == Some(name) {
            return Some(p);
        }
    }
    None
}

#[test]
fn gbk_csv_queryable_via_sql() {
    let tmp = std::env::temp_dir().join("csv_utf8_test_sql_gbk");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    let content = "id,name,city,amount\n1,张三,北京,100.5\n2,李四,上海,200.0\n3,王五,广州,150.75\n";
    let p = tmp.join("orders.csv");
    write_csv(&p, content, GB18030);

    let r = sql::run_sql(&tmp, "SELECT * FROM 'orders.csv' ORDER BY id", 10).expect("query failed");
    assert_eq!(r.columns, vec!["id", "name", "city", "amount"]);
    assert_eq!(r.row_count, 3, "应有 3 行, 实际 {}; transcoded={:?}", r.row_count, r.transcoded);
    assert_eq!(r.rows[0][1], "张三", "中文乱码!");
    assert_eq!(r.rows[0][2], "北京");
    assert_eq!(r.rows[2][3], "150.75");
    assert!(!r.transcoded.is_empty(), "GBK 文件应被转码");
    assert!(r.cached.is_empty(), "首次查询不应命中缓存");
}

#[test]
fn second_query_hits_cache() {
    let tmp = std::env::temp_dir().join("csv_utf8_test_sql_cache");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    // 必须含 GBK 中文, 否则纯 ASCII 会被 detect 判 UTF-8, 走 AsciiOrUtf8 路径, 不进缓存
    let content = "id,name\n1,测试\n2,缓存验证\n";
    let p = tmp.join("t.csv");
    write_csv(&p, content, GB18030);

    let r1 = sql::run_sql(&tmp, "SELECT * FROM 't.csv'", 10).expect("query 1 failed");
    assert!(!r1.transcoded.is_empty(), "首次 GBK 查询应触发转码, got {:?}", r1.transcoded);
    assert_eq!(r1.rows[0][1], "测试");

    let r2 = sql::run_sql(&tmp, "SELECT * FROM 't.csv'", 10).expect("query 2 failed");
    assert!(r2.cached.iter().any(|f| f == "t.csv"), "第二次应命中缓存, got {:?}", r2.cached);
    assert_eq!(r2.rows, r1.rows, "两次结果应一致");
}

/// 缓存位置: 不再落到系统临时目录, 而是数据文件同目录, 名称加 `utf8_` 前缀.
#[test]
fn cache_written_next_to_source_with_prefix() {
    let tmp = std::env::temp_dir().join("csv_utf8_test_sql_cache_loc");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    let content = "id,name\n1,测试\n2,定位\n";
    let p = tmp.join("t.csv");
    write_csv(&p, content, GB18030);

    let _ = sql::run_sql(&tmp, "SELECT * FROM 't.csv'", 10).expect("query failed");

    let cache = tmp.join("utf8_t.csv");
    assert!(cache.is_file(), "缓存应生成在数据目录: {}", cache.display());
}

/// 非 csv/tsv 扩展名: 缓存保留原扩展名 (`utf8_notes.txt`), 且 SQL 字面量
/// 被包成 `read_csv_auto(...)`, DuckDB 才能按 CSV 解析 (裸路径只认 csv/tsv).
#[test]
fn non_csv_extension_transcoded_and_readable() {
    let tmp = std::env::temp_dir().join("csv_utf8_test_sql_txt");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    let content = "id,name\n1,测试\n2,文本\n";
    let p = tmp.join("notes.txt");
    write_csv(&p, content, GB18030);

    let r = sql::run_sql(&tmp, "SELECT * FROM 'notes.txt'", 10).expect("txt query failed");
    assert_eq!(r.row_count, 2, "txt 应可查询, rows={:?}", r.rows);
    assert_eq!(r.rows[0][1], "测试", "txt 中文不应乱码");
    assert!(
        tmp.join("utf8_notes.txt").is_file(),
        "应生成保留原扩展名的缓存 utf8_notes.txt"
    );
}

/// 自定义缓存目录: 缓存写到指定目录 (按源文件哈希分子目录隔离), 不写数据目录.
#[test]
fn custom_cache_dir_is_used() {
    let tmp = std::env::temp_dir().join("csv_utf8_test_sql_cache_custom");
    let _ = fs::remove_dir_all(&tmp);
    let data = tmp.join("data");
    let cache = tmp.join("cache");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&cache).unwrap();

    let content = "id,name\n1,测试\n2,自定义\n";
    let p = data.join("t.csv");
    write_csv(&p, content, GB18030);

    let r = sql::run_sql_with_cache(&data, "SELECT * FROM 't.csv'", 10, Some(&cache))
        .expect("query failed");
    assert_eq!(r.rows[0][1], "测试");
    assert!(!r.transcoded.is_empty(), "首次查询应触发转码");

    // 缓存不应出现在数据目录
    assert!(
        !data.join("utf8_t.csv").exists(),
        "指定自定义缓存目录后, 不应再写数据目录"
    );
    // 缓存应出现在自定义目录下 (子目录隔离, 文件名仍为 utf8_t.csv)
    let found = find_file(&cache, "utf8_t.csv");
    assert!(found.is_some(), "自定义缓存目录下应生成 utf8_t.csv");

    // 第二次查询应命中该缓存
    let r2 = sql::run_sql_with_cache(&data, "SELECT * FROM 't.csv'", 10, Some(&cache))
        .expect("query 2 failed");
    assert!(
        r2.cached.iter().any(|f| f == "t.csv"),
        "第二次应命中自定义缓存, got {:?}",
        r2.cached
    );
}

#[test]
fn aggregate_query_with_groupby() {
    let tmp = std::env::temp_dir().join("csv_utf8_test_sql_agg");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    let content = "city,amount\n北京,100\n上海,200\n北京,150\n上海,50\n广州,300\n";
    let p = tmp.join("sales.csv");
    write_csv(&p, content, GB18030);

    let r = sql::run_sql(
        &tmp,
        "SELECT city, SUM(amount) AS total, COUNT(*) AS cnt
         FROM 'sales.csv' GROUP BY city ORDER BY total DESC",
        10,
    )
    .expect("agg query failed");
    assert_eq!(r.row_count, 3);
    assert_eq!(r.columns, vec!["city", "total", "cnt"]);
    assert_eq!(r.rows[0][0], "广州");
    assert_eq!(r.rows[0][1], "300");
}

#[test]
fn result_truncated_by_limit() {
    let tmp = std::env::temp_dir().join("csv_utf8_test_sql_trunc");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    let content: String = (1..=20)
        .map(|i| format!("{},value{}\n", i, i))
        .collect();
    let p = tmp.join("big.csv");
    write_csv(&p, &content, GB18030);

    let r = sql::run_sql(&tmp, "SELECT * FROM 'big.csv' ORDER BY column0", 5).expect("query failed");
    assert_eq!(r.row_count, 5);
    assert!(r.truncated, "limit=5 但有 20 行, 应标记 truncated");
}

// ============ 离线 Extension 加载 ============

/// 关键验证: 应用启动时 preload_extensions 已自动 LOAD json, 无需用户写 INSTALL/LOAD
#[test]
fn json_extension_auto_loaded() {
    let tmp = std::env::temp_dir().join("csv_utf8_test_auto_json");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    // 直接用 json() 函数, 不写 INSTALL/LOAD
    let r = sql::run_sql(&tmp, "SELECT json('[10,20,30]')::VARCHAR AS j", 10)
        .expect("json 函数应可用 (auto LOAD 成功)");
    assert_eq!(r.columns, vec!["j"]);
    let v = &r.rows[0][0];
    assert!(v.contains("10") && v.contains("20") && v.contains("30"), "json() 返回值异常: {v}");
}

/// 验证 parquet 扩展已 LOAD 且功能可用.
///
/// 注: 原实现断言 `parquet_version()` 存在, 但 DuckDB 1.5.6 并没有该标量函数
/// (它属于旧版本/别的命名), 导致测试恒失败. 这里改为断言 parquet 扩展确已
/// 载入, 并实际读写一个 parquet 文件来证明功能可用.
#[test]
fn parquet_extension_auto_loaded() {
    let tmp = std::env::temp_dir().join("csv_utf8_test_auto_parquet");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    // 1. 扩展确实处于 loaded 状态
    let r = sql::run_sql(
        &tmp,
        "SELECT count(*)::VARCHAR AS n FROM duckdb_extensions() \
         WHERE extension_name='parquet' AND loaded",
        10,
    )
    .expect("查询 duckdb_extensions() 失败");
    assert_eq!(r.rows[0][0], "1", "parquet 扩展应处于 loaded 状态");

    // 2. 实际写出并读回一个 parquet 文件 (COPY TO / read_parquet)
    let src = tmp.join("src.csv");
    fs::write(&src, "id,name\n1,张三\n2,李四\n").unwrap();
    let pq = tmp.join("out.parquet");
    let pq_sql = pq.to_string_lossy().replace('\\', "/");

    sql::run_sql(
        &tmp,
        &format!("COPY (SELECT * FROM 'src.csv') TO '{pq_sql}' (FORMAT PARQUET)"),
        10,
    )
    .expect("写入 parquet 失败 (parquet 扩展不可用)");
    assert!(pq.is_file(), "parquet 文件未生成: {}", pq.display());

    let r2 = sql::run_sql(
        &tmp,
        &format!("SELECT count(*)::VARCHAR AS n FROM read_parquet('{pq_sql}')"),
        10,
    )
    .expect("读取 parquet 失败 (parquet 扩展不可用)");
    assert_eq!(r2.rows[0][0], "2", "parquet 行数应为 2");
}
