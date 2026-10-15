//! 导出功能测试: 查询结果 → UTF-8 CSV / Parquet
//!
//! 重点验证:
//! 1. CSV 是**合法 UTF-8** 且中文正确 (核心需求: "utf8格式的csv")
//! 2. Parquet 能被读回, 且**类型保留** (不是全字符串)
//! 3. 导出的是**完整**结果集, 不受界面 1000 行截断影响
//! 4. GB18030 源文件经转码后导出, 中文不乱码
//! 5. 覆盖保护 / 格式校验 / 错误处理

use csv_utf8_converter_lib::sql;
use std::fs;
use std::path::{Path, PathBuf};

/// 每个测试用独立临时目录
fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("csv_utf8_test_export_{name}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn write_gbk_csv(path: &Path, content: &str) {
    let (bytes, _, _) = encoding_rs::GB18030.encode(content);
    fs::write(path, bytes.as_ref()).unwrap();
}

// ============ CSV 导出 ============

/// 核心: 导出 UTF-8 CSV, 中文正确, 无 BOM (默认)
#[test]
fn export_csv_is_valid_utf8_without_bom() {
    let d = tmp_dir("csv_plain");
    write_gbk_csv(&d.join("orders.csv"), "id,name,city\n1,张三,北京\n2,李四,上海\n");

    let out = d.join("result.csv");
    let r = sql::export_sql_result(
        &d,
        "SELECT * FROM 'orders.csv' ORDER BY id",
        &out,
        false,
        false,
    )
    .expect("导出 CSV 应成功");

    assert_eq!(r.rows, 2);
    assert_eq!(r.format, "csv");
    assert!(r.bytes > 0);
    assert!(out.is_file());

    let raw = fs::read(&out).expect("读取导出文件");
    // 1. 必须是合法 UTF-8 (核心需求)
    let text = std::str::from_utf8(&raw).expect("导出 CSV 必须是合法 UTF-8");
    // 2. 无 BOM
    assert!(
        !raw.starts_with(&[0xEF, 0xBB, 0xBF]),
        "未指定 BOM 时不应有 BOM"
    );
    // 3. 表头 + 中文内容正确
    assert!(text.contains("id,name,city"), "应有表头, got: {text}");
    assert!(text.contains("张三"), "中文应正确: {text}");
    assert!(text.contains("上海"), "中文应正确: {text}");
    // 4. 不含 GBK 乱码痕迹
    assert!(!text.contains('\u{FFFD}'), "不应有替换字符(乱码): {text}");
    println!("✓ CSV 导出 (无 BOM) 合法 UTF-8:\n{text}");
}

/// BOM 选项生效
#[test]
fn export_csv_with_bom() {
    let d = tmp_dir("csv_bom");
    fs::write(d.join("t.csv"), "a,b\n1,中文\n").unwrap();

    let out = d.join("with_bom.csv");
    sql::export_sql_result(&d, "SELECT * FROM 't.csv'", &out, false, true)
        .expect("导出应成功");

    let raw = fs::read(&out).unwrap();
    assert!(
        raw.starts_with(&[0xEF, 0xBB, 0xBF]),
        "指定 BOM 时文件应以 UTF-8 BOM 开头, 实际前 3 字节: {:02X?}",
        &raw[..raw.len().min(3)]
    );
    // 去掉 BOM 后仍是合法 UTF-8
    let text = std::str::from_utf8(&raw[3..]).expect("BOM 之后应是合法 UTF-8");
    assert!(text.contains("中文"));
    println!("✓ CSV 导出 (带 BOM) 正确");
}

/// CSV 引号/逗号/换行 的 RFC4180 转义
#[test]
fn export_csv_escapes_special_chars() {
    let d = tmp_dir("csv_escape");
    // 内容含逗号、双引号
    fs::write(d.join("t.csv"), "id,note\n1,\"含,逗号\"\n2,\"含\"\"引号\"\"\"\n").unwrap();

    let out = d.join("esc.csv");
    sql::export_sql_result(&d, "SELECT * FROM 't.csv' ORDER BY id", &out, false, false)
        .expect("导出应成功");

    let text = fs::read_to_string(&out).unwrap();
    // 再用 DuckDB 读回, 验证转义可逆
    let back = sql::run_sql(&d, &format!("SELECT * FROM '{}' ORDER BY id",
        out.to_string_lossy().replace('\\', "/")), 10).expect("读回应成功");
    assert_eq!(back.rows[0][1], "含,逗号", "逗号字段应能正确还原");
    assert_eq!(back.rows[1][1], "含\"引号\"", "引号字段应能正确还原");
    println!("✓ CSV 特殊字符转义正确:\n{text}");
}

// ============ Parquet 导出 ============

/// 核心: 导出 Parquet 并能读回, 且**类型保留**
#[test]
fn export_parquet_preserves_types() {
    let d = tmp_dir("parquet_types");
    // amount 是数字, 导出后应仍是数字类型 (CSV 会变字符串)
    fs::write(d.join("orders.csv"), "id,name,amount\n1,张三,100.5\n2,李四,200.0\n").unwrap();

    let out = d.join("result.parquet");
    let r = sql::export_sql_result(&d, "SELECT * FROM 'orders.csv' ORDER BY id", &out, false, false)
        .expect("导出 Parquet 应成功");

    assert_eq!(r.rows, 2);
    assert_eq!(r.format, "parquet");
    assert!(out.is_file() && r.bytes > 0);

    // 用 DuckDB 读回, 并检查 amount 的类型是数值而非 VARCHAR
    let out_sql = out.to_string_lossy().replace('\\', "/");
    let conn_sql = format!(
        "SELECT any_value(typeof(amount)) AS t, sum(amount)::VARCHAR AS s FROM read_parquet('{out_sql}')"
    );
    let back = sql::run_sql(&d, &conn_sql, 10).expect("读回 parquet 应成功");
    let ty = &back.rows[0][0];
    assert!(
        ty.contains("DECIMAL") || ty.contains("DOUBLE") || ty.contains("FLOAT"),
        "amount 应是数值类型, 实际: {ty}"
    );
    // 数值求和正确 (证明类型真的保住了)
    assert_eq!(back.rows[0][1], "300.5", "数值求和应为 300.5");

    // 中文也要正确
    let cn = sql::run_sql(
        &d,
        &format!("SELECT name FROM read_parquet('{out_sql}') ORDER BY id LIMIT 1"),
        10,
    )
    .expect("查询应成功");
    assert_eq!(cn.rows[0][0], "张三", "Parquet 中文应正确");
    println!("✓ Parquet 类型保留: amount typeof={ty}, 求和=300.5, 中文正确");
}

// ============ 完整性: 不受界面截断影响 ============

/// 关键: 界面预览被 limit=10 截断, 但导出必须是**完整** 100 行
#[test]
fn export_contains_full_result_beyond_ui_limit() {
    let d = tmp_dir("full_rows");
    // 构造 100 行
    let mut content = String::from("id,val\n");
    for i in 1..=100 {
        content.push_str(&format!("{i},v{i}\n"));
    }
    fs::write(d.join("big.csv"), content).unwrap();

    // 模拟界面: limit=10 → 只拿到 10 行
    let preview = sql::run_sql(&d, "SELECT * FROM 'big.csv'", 10).expect("预览查询");
    assert_eq!(preview.row_count, 10);
    assert!(preview.truncated, "预览应标记截断");

    // 导出: 必须拿到全部 100 行
    let out = d.join("full.csv");
    let r = sql::export_sql_result(&d, "SELECT * FROM 'big.csv'", &out, false, false)
        .expect("导出应成功");
    assert_eq!(r.rows, 100, "导出必须是完整 100 行, 而非界面截断的 10 行");

    // 实际文件行数 = 表头 + 100
    let text = fs::read_to_string(&out).unwrap();
    assert_eq!(text.lines().count(), 101, "CSV 应有 1 表头 + 100 数据行");
    println!("✓ 导出完整结果: 界面 10 行(截断) vs 导出 {} 行", r.rows);
}

// ============ 错误处理 ============

/// 不支持扩展名应报错
#[test]
fn export_rejects_unsupported_extension() {
    let d = tmp_dir("bad_ext");
    fs::write(d.join("t.csv"), "a\n1\n").unwrap();
    let err = sql::export_sql_result(&d, "SELECT * FROM 't.csv'", &d.join("x.txt"), false, false)
        .expect_err("不支持的扩展名应报错");
    let msg = format!("{err:#}");
    assert!(msg.contains("不支持"), "错误信息应说明格式不支持: {msg}");
    println!("✓ 拒绝不支持的格式: {msg}");
}

/// 未勾选覆盖时, 目标存在应报错; 勾选后应成功
#[test]
fn export_overwrite_protection() {
    let d = tmp_dir("overwrite");
    fs::write(d.join("t.csv"), "a\n1\n").unwrap();
    let out = d.join("out.csv");
    fs::write(&out, "已存在的旧内容").unwrap();

    // 不覆盖 → 报错
    let err = sql::export_sql_result(&d, "SELECT * FROM 't.csv'", &out, false, false)
        .expect_err("未勾选覆盖时应报错");
    assert!(format!("{err:#}").contains("已存在"), "应提示已存在");
    // 原内容未被破坏
    assert_eq!(fs::read_to_string(&out).unwrap(), "已存在的旧内容");

    // 覆盖 → 成功
    let r = sql::export_sql_result(&d, "SELECT * FROM 't.csv'", &out, true, false)
        .expect("勾选覆盖后应成功");
    assert_eq!(r.rows, 1);
    assert!(fs::read_to_string(&out).unwrap().contains('a'));
    println!("✓ 覆盖保护生效");
}

/// 导出 SQL 语法错误应给出清晰错误而非 panic
#[test]
fn export_bad_sql_reports_error() {
    let d = tmp_dir("bad_sql");
    let err = sql::export_sql_result(&d, "SELECT * FROM 不存在的表", &d.join("o.csv"), false, false)
        .expect_err("非法 SQL 应报错");
    let msg = format!("{err:#}");
    assert!(!msg.is_empty());
    println!("✓ 非法 SQL 错误: {}", msg.lines().next().unwrap_or(""));
}

/// 两种格式导出同一查询, 行数应一致 (交叉校验)
#[test]
fn csv_and_parquet_agree_on_rows() {
    let d = tmp_dir("agree");
    write_gbk_csv(&d.join("o.csv"), "id,name\n1,张三\n2,李四\n3,王五\n");

    let csv_out = d.join("r.csv");
    let pq_out = d.join("r.parquet");
    let a = sql::export_sql_result(&d, "SELECT * FROM 'o.csv'", &csv_out, false, false).unwrap();
    let b = sql::export_sql_result(&d, "SELECT * FROM 'o.csv'", &pq_out, false, false).unwrap();
    assert_eq!(a.rows, b.rows, "两种格式行数应一致");
    assert_eq!(a.rows, 3);

    // 两边读回内容一致
    let c = sql::run_sql(&d, &format!("SELECT count(*)::VARCHAR FROM '{}'",
        csv_out.to_string_lossy().replace('\\', "/")), 10).unwrap();
    let p = sql::run_sql(&d, &format!("SELECT count(*)::VARCHAR FROM read_parquet('{}')",
        pq_out.to_string_lossy().replace('\\', "/")), 10).unwrap();
    assert_eq!(c.rows[0][0], p.rows[0][0], "两种格式读回行数应一致");
    println!("✓ CSV/Parquet 行数一致: {} 行", a.rows);
}
