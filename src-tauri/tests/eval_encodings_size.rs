//! 测量: 加载 `encodings` 扩展的运行时代价 (时间).
//!
//! 磁盘代价另计 (168.9 MB / 占预置扩展总量 46%).
//! 这里只测"多 LOAD 一个扩展"带来的连接建立开销, 用于说明去掉它的收益边界.
//!
//! **已归档**: 结论已采纳 (`encodings` 已从 PRELOAD_EXTENSIONS 与仓库中移除),
//! 该测试依赖已删除的扩展文件, 因此标记 `#[ignore]`。需要复测时先按
//! `install_all.rs` 的方式把 encodings 下载回来。

use std::path::PathBuf;

const EXT_VERSION: &str = "v1.5.6";
const EXT_PLATFORM: &str = "windows_amd64";
const BASE: &[&str] = &["json", "parquet", "icu", "httpfs", "fts", "excel", "spatial"];

fn ext_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tools")
        .join("duckdb-extensions")
        .join(EXT_VERSION)
        .join(EXT_PLATFORM)
}

/// 建立连接并 LOAD 给定扩展, 返回耗时(ms)
fn load(names: &[&str]) -> u128 {
    let t0 = std::time::Instant::now();
    let config = duckdb::Config::default()
        .allow_unsigned_extensions()
        .expect("cfg");
    let conn = duckdb::Connection::open_in_memory_with_flags(config).expect("open");
    for n in names {
        let p = ext_dir().join(format!("{n}.duckdb_extension"));
        if p.is_file() {
            let s = p.to_string_lossy().replace('\\', "/");
            conn.execute_batch(&format!("LOAD '{s}';"))
                .unwrap_or_else(|e| panic!("LOAD {n} 失败: {e}"));
        }
    }
    // 确保连接可用
    let v: i32 = conn.query_row("SELECT 1", [], |r| r.get(0)).expect("usable");
    assert_eq!(v, 1);
    t0.elapsed().as_millis()
}

#[test]
#[ignore = "encodings 扩展已从仓库移除; 需要复测时先下载回来"]
fn measure_encodings_load_cost() {
    // 预热: 让文件进入系统缓存, 避免首次读盘干扰
    for _ in 0..2 {
        let _ = load(BASE);
    }

    let rounds = 3;
    let mut ta_sum = 0u128;
    let mut tb_sum = 0u128;

    let mut with = BASE.to_vec();
    with.push("encodings");

    for _ in 0..rounds {
        ta_sum += load(BASE);
        tb_sum += load(&with);
    }

    let ta = ta_sum / rounds;
    let tb = tb_sum / rounds;

    println!("--- 连接 + LOAD 耗时 (各 {rounds} 轮均值, 已预热) ---");
    println!("不含 encodings (7 个): {ta} ms");
    println!("含   encodings (8 个): {tb} ms");
    println!("差值: {} ms", tb as i64 - ta as i64);
    println!("磁盘: 不含 encodings 195.2 MB / 含 364.1 MB (省 168.9 MB, -46%)");

    // 明确的评估结论: 若额外耗时很小, 说明收益几乎纯赚磁盘空间
    let delta = tb as i64 - ta as i64;
    assert!(
        tb > 0,
        "耗时测量异常"
    );
    if delta.abs() < 50 {
        println!("→ 额外 LOAD 耗时 < 50 ms, 运行时几乎无感");
    }
}
