//! 端到端演示: 生成真实的导出文件供人工检查.
//!
//! 运行: cargo test --test export_demo -- --nocapture
//! 输出目录: %TEMP%\csv_utf8_export_demo\

use csv_utf8_converter_lib::sql;
use std::fs;

#[test]
fn demo_generate_real_export_files() {
    let d = std::env::temp_dir().join("csv_utf8_export_demo");
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();

    // 造一个 GB18030 的销售数据 (含中文 + 金额 + 日期)
    let content = "订单号,客户,城市,金额,日期\n\
                   A001,张三,北京,1250.50,2024-01-15\n\
                   A002,李四,上海,890.00,2024-01-16\n\
                   A003,王五,广州,3200.75,2024-02-01\n\
                   A004,赵六,深圳,450.25,2024-02-03\n\
                   A005,钱七,杭州,1780.00,2024-03-11\n";
    let (bytes, _, _) = encoding_rs::GB18030.encode(content);
    fs::write(d.join("sales.csv"), bytes.as_ref()).unwrap();

    let query = "SELECT 城市, count(*) AS 订单数, sum(金额) AS 总额 \
                 FROM 'sales.csv' GROUP BY 城市 ORDER BY 总额 DESC";

    // CSV (带 BOM, 便于 Excel 直接打开)
    let csv_out = d.join("sales_summary.csv");
    let a = sql::export_sql_result(&d, query, &csv_out, true, true).expect("CSV 导出");
    // Parquet
    let pq_out = d.join("sales_summary.parquet");
    let b = sql::export_sql_result(&d, query, &pq_out, true, false).expect("Parquet 导出");

    println!("\n================ 导出结果 ================");
    println!("源文件 : sales.csv (GB18030, 含中文)");
    println!("查询   : {query}");
    println!("\nCSV     : {} ({} 行, {} 字节)", a.path, a.rows, a.bytes);
    println!("Parquet : {} ({} 行, {} 字节)", b.path, b.rows, b.bytes);

    let raw = fs::read(&csv_out).unwrap();
    println!("\n--- CSV 头部字节 (前 3 字节应为 EF BB BF = BOM) ---");
    println!("{:02X?}", &raw[..3.min(raw.len())]);
    println!("\n--- CSV 内容 ---");
    println!("{}", String::from_utf8(raw.clone()).expect("必须是合法 UTF-8"));

    // 独立校验: Parquet 读回
    let pq_sql = pq_out.to_string_lossy().replace('\\', "/");
    let back = sql::run_sql(
        &d,
        &format!("SELECT * FROM read_parquet('{pq_sql}') ORDER BY 总额 DESC"),
        10,
    )
    .expect("读回 parquet");
    println!("--- Parquet 读回 (列: {:?}) ---", back.columns);
    for r in &back.rows {
        println!("  {}", r.join(" | "));
    }

    println!("\n================ 校验 ================");
    assert!(raw.starts_with(&[0xEF, 0xBB, 0xBF]), "CSV 应有 BOM");
    let text = std::str::from_utf8(&raw).unwrap();
    assert!(text.contains("北京") && text.contains("上海"), "中文应正确");
    assert!(!text.contains('\u{FFFD}'), "不应有乱码");
    // 5 个源订单, 按城市分组后每个城市 1 行
    assert_eq!(a.rows, 5, "应有 5 个城市");
    assert_eq!(b.rows, 5);
    println!("✓ CSV 合法 UTF-8 + BOM + 中文正确");
    println!("✓ Parquet 行数与 CSV 一致 ({})", b.rows);
    println!("✓ 输出目录: {}\n", d.display());
}
