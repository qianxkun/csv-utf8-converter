//! SQL 查询引擎 (DuckDB): 对任意编码 CSV 直接执行 SQL
//!
//! 核心机制: 扫描 SQL 中的字符串字面量, 把"存在的、本次需要转码的"数据文件
//! 替换为已转码的 UTF-8 缓存文件路径, 然后交给 DuckDB 内置 CSV 解析器
//! (`read_csv_auto`) 处理. 相对路径基于"数据目录"解析为绝对路径, 避免依赖进程 CWD.
//!
//! 复用本项目已有的 detector + converter, 转码结果缓存到**数据文件所在目录**,
//! 文件名为 `utf8_<原文件名>` (如 `订单.csv` -> `utf8_订单.csv`),
//! 依据源文件 mtime 判断是否过期, 重复查询几乎零成本.

use crate::converter::{convert, ConvertOptions};
use crate::detector::{detect, DetectedEncoding};
use anyhow::{Context, Result};
use duckdb::types::{TimeUnit, Value};
use duckdb::{Config, Connection, Statement};
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

/// 需要**转码**为 UTF-8 后再查询的文本类扩展名.
/// 这些文件的编码事先未知, 必须先探测并转码, DuckDB 才能正确解析.
const DATA_EXTS: &[&str] = &["csv", "tsv", "txt", "psv"];

/// **无需转码**的二进制/自带编码的扩展名.
/// Parquet 自带 schema 与 UTF-8 字符串编码, 直接交给 DuckDB 读取即可,
/// 但相对路径仍需重写为绝对路径 (DuckDB 按进程 CWD 解析相对路径).
const BINARY_EXTS: &[&str] = &["parquet", "pq"];

/// 预置扩展目录的版本号. 必须与 `duckdb-rs` 捆绑的 DuckDB API 版本一致,
/// 否则 LOAD 会因扩展版本校验失败 (见 `duckdb.h` 的 DUCKDB_API_VERSION_*).
const EXT_VERSION: &str = "v1.5.6";

/// 预置扩展目录的平台标识
const EXT_PLATFORM: &str = "windows_amd64";

/// 应用启动时自动 LOAD 的 DuckDB extension 列表.
///
/// 物理文件位于 `<resource_dir>/duckdb-extensions/v1.5.6/windows_amd64/<name>.duckdb_extension`
/// (开发期为 `<src-tauri>/tools/duckdb-extensions/...`)。
/// 版本目录必须与本 crate 捆绑的 DuckDB API 版本严格一致 (见 `duckdb.h` 的
/// `DUCKDB_API_VERSION_MAJOR/MINOR/PATCH` = 1/5/6), 否则 LOAD 会因版本校验失败.
///
/// 按绝对路径 LOAD 不需要 DuckDB 下载缓存旁的 `.info` 旁挂文件 (那只是 INSTALL 的
/// 下载元数据: 来源 URL / 版本 / ETag), 因此项目内只放 `.duckdb_extension` 即可.
///
/// 注: 缺失或 LOAD 失败时静默跳过 (退化为按需联网 INSTALL), 结果记录在
/// [`preload_extensions`] 返回的报告中, 便于上层判断离线可用性.
///
/// **刻意不含 `encodings`**: 本项目的管线是「先转码成 UTF-8 再交给 DuckDB」,
/// DuckDB 侧永远只见 UTF-8, 该扩展从未被用到; 而它单文件 177 MB,
/// 占全部扩展载荷的 48%. `tests/eval_drop_encodings.rs` 验证过删除不影响功能.
const PRELOAD_EXTENSIONS: &[&str] = &[
    "json",
    "parquet",
    "icu",
    "httpfs",
    "fts",
    "excel",
    "spatial",
];

/// 单个 extension 的预加载结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionStatus {
    pub name: String,
    /// 是否已成功 LOAD
    pub loaded: bool,
    /// 文件是否存在于项目内
    pub present: bool,
    /// 失败原因 (缺失或 LOAD 报错)
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub row_count: usize,
    pub truncated: bool,
    pub elapsed_ms: u64,
    /// 本次查询中实际触发转码的文件 (相对路径)
    pub transcoded: Vec<String>,
    /// 命中转码缓存的文件 (相对路径)
    pub cached: Vec<String>,
    /// SQL 解析后真正被替换的路径信息 (调试用, 可选)
    pub notes: Vec<String>,
    /// 本次查询所用连接的扩展预加载结果 (离线可用性诊断)
    pub extensions: Vec<ExtensionStatus>,
}

/// 打开一个已预加载扩展的 DuckDB 内存连接, 并返回各扩展加载状态.
///
/// `allow_unsigned_extensions` 必须在 open 之前设置, 否则 database 进入 running
/// 状态后被拒. 查询与导出共用此入口, 保证行为一致.
fn open_conn() -> Result<(Connection, Vec<ExtensionStatus>)> {
    let config = Config::default()
        .allow_unsigned_extensions()
        .context("配置 allow_unsigned_extensions 失败")?;
    let conn = Connection::open_in_memory_with_flags(config).context("打开 DuckDB 失败")?;
    let status = preload_extensions(&conn);
    Ok((conn, status))
}

/// 在数据目录中跑一次 SQL (转码缓存默认写到数据文件同目录).
///
/// 需要自定义缓存目录时改用 [`run_sql_with_cache`].
pub fn run_sql(data_dir: &Path, sql: &str, limit: usize) -> Result<SqlResult> {
    run_sql_with_cache(data_dir, sql, limit, None)
}

/// 在数据目录中跑一次 SQL.
///
/// `cache_dir` 为 `None` 时, 转码缓存写到数据文件同目录 (`utf8_<原文件名>`);
/// 为 `Some(d)` 时写到 `d` 下 (按源文件绝对路径哈希分子目录, 避免同名冲突).
pub fn run_sql_with_cache(
    data_dir: &Path,
    sql: &str,
    limit: usize,
    cache_dir: Option<&Path>,
) -> Result<SqlResult> {
    let started = Instant::now();

    // 1. 预处理 SQL: 把"存在的、本次需要转码的"数据文件字面量替换为 UTF-8 缓存路径
    let (rewritten, transcoded, cached, notes) =
        rewrite_sql_file_literals(sql, data_dir, cache_dir)?;

    // 2. DuckDB in-memory 执行 + 加载项目内预下载的 extension
    let (conn, extensions) = open_conn()?;
    let mut stmt: Statement<'_> = conn
        .prepare(&rewritten)
        .with_context(|| format!("SQL 解析失败: {}", rewritten))?;
    // 关键: duckdb-rs 1.10506 要求先 query/execute 才能取列信息.
    // Rows::as_ref() 拿 &Statement, 避开"同时借 stmt"的冲突, 官方文档示例.
    let mut rows = stmt.query([]).context("执行查询失败")?;
    let columns: Vec<String> = rows
        .as_ref()
        .expect("query() must return rows with a statement reference")
        .column_names();
    let col_count = columns.len();

    // 3. 收集结果, 受 limit 截断
    let mut out_rows: Vec<Vec<String>> = Vec::new();
    let mut truncated = false;
    while let Some(row) = rows.next()? {
        if out_rows.len() >= limit {
            truncated = true;
            break;
        }
        let mut vals = Vec::with_capacity(col_count);
        for i in 0..col_count {
            let v: Value = row
                .get::<usize, Value>(i)
                .with_context(|| format!("读取第 {i} 列失败"))?;
            vals.push(value_to_string(&v));
        }
        out_rows.push(vals);
    }

    Ok(SqlResult {
        row_count: out_rows.len(),
        rows: out_rows,
        columns,
        truncated,
        elapsed_ms: started.elapsed().as_millis() as u64,
        transcoded,
        cached,
        notes,
        extensions,
    })
}

// ===================== 结果导出 =====================

/// 导出格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// UTF-8 CSV (带表头, 标准 RFC4180 引号转义)
    Csv,
    /// Parquet (列式, 保留原始类型)
    Parquet,
}

impl ExportFormat {
    pub fn from_ext(path: &Path) -> Option<Self> {
        match path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("csv") => Some(Self::Csv),
            Some("parquet") | Some("pq") => Some(Self::Parquet),
            _ => None,
        }
    }

    /// 用于界面提示的简短名称
    pub fn name(self) -> &'static str {
        match self {
            Self::Csv => "CSV (UTF-8)",
            Self::Parquet => "Parquet",
        }
    }
}

/// 导出统计
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub path: String,
    pub format: String,
    pub rows: u64,
    pub bytes: u64,
    pub elapsed_ms: u64,
}

/// 把 SQL 查询结果导出为文件 (UTF-8 CSV 或 Parquet), 转码缓存默认写到数据文件同目录.
///
/// 需要自定义缓存目录时改用 [`export_sql_result_with_cache`].
pub fn export_sql_result(
    data_dir: &Path,
    sql: &str,
    out_path: &Path,
    overwrite: bool,
    write_bom: bool,
) -> Result<ExportResult> {
    export_sql_result_with_cache(data_dir, sql, out_path, overwrite, write_bom, None)
}

/// 把 SQL 查询结果导出为文件 (UTF-8 CSV 或 Parquet).
///
/// 关键设计: **不导出界面上那份被 limit 截断、且已字符串化的预览数据**,
/// 而是重新执行查询, 用 DuckDB 原生 `COPY (...) TO ...` 全量落盘.
/// 好处:
///   - 导出的是**完整**结果集, 不受界面 1000 行截断影响
///   - 类型信息保留 (Parquet 是真列类型, 不是字符串)
///   - 编码由 DuckDB 保证为 UTF-8 (源文件已由本项目转码为 UTF-8)
///
/// `overwrite=false` 且目标已存在时返回错误, 避免误覆盖.
/// `cache_dir` 语义同 [`run_sql_with_cache`].
pub fn export_sql_result_with_cache(
    data_dir: &Path,
    sql: &str,
    out_path: &Path,
    overwrite: bool,
    write_bom: bool,
    cache_dir: Option<&Path>,
) -> Result<ExportResult> {
    let started = Instant::now();

    if out_path.is_dir() {
        return Err(anyhow::anyhow!(
            "导出路径是目录, 请指定文件名: {}",
            out_path.display()
        ));
    }
    if out_path.exists() && !overwrite {
        return Err(anyhow::anyhow!(
            "目标文件已存在 (未勾选覆盖): {}",
            out_path.display()
        ));
    }
    let format = ExportFormat::from_ext(out_path).ok_or_else(|| {
        anyhow::anyhow!(
            "不支持的导出格式 (仅支持 .csv / .parquet): {}",
            out_path.display()
        )
    })?;

    // 若目标父目录不存在, 尝试创建
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建导出目录失败: {}", parent.display()))?;
        }
    }

    // 与 run_sql 一致: 先把数据文件字面量换成已转码的 UTF-8 路径
    let (rewritten, _transcoded, _cached, _notes) =
        rewrite_sql_file_literals(sql, data_dir, cache_dir)?;

    let (conn, _ext) = open_conn()?;

    // 去掉用户 SQL 末尾的分号, 否则嵌进 COPY (...) 会语法错误
    let body = rewritten.trim().trim_end_matches(';').trim_end();
    if body.is_empty() {
        return Err(anyhow::anyhow!("SQL 为空, 无法导出"));
    }

    // 转义单引号, 拼成 COPY 语句
    let target = out_path.to_string_lossy().replace('\\', "/").replace('\'', "''");
    // CSV 参数: 显式表头 + RFC4180 引号/转义
    // 注: DuckDB 的 COPY 没有 BOM 选项 (Unrecognized option "BOM"), BOM 在写完后单独补
    let copy_format = match format {
        ExportFormat::Csv => "FORMAT CSV, HEADER, DELIMITER ',', QUOTE '\"', ESCAPE '\"'",
        ExportFormat::Parquet => "FORMAT PARQUET",
    };
    let copy_sql = format!("COPY ({body}) TO '{target}' ({copy_format})");

    conn.execute_batch(&copy_sql)
        .with_context(|| format!("导出失败 (格式 {:?}): {}", format, out_path.display()))?;

    // CSV 可选写 UTF-8 BOM (Excel 打开中文不乱码).
    // DuckDB COPY 不支持 BOM 选项, 因此在落盘后把 BOM 前置写入.
    if format == ExportFormat::Csv && write_bom {
        prepend_bom(out_path)
            .with_context(|| format!("写入 BOM 失败: {}", out_path.display()))?;
    }

    let meta = std::fs::metadata(out_path)
        .with_context(|| format!("导出后无法读取文件信息: {}", out_path.display()))?;

    // 行数: 用 COUNT(*) 再确认一次 (COPY 的返回不足以直接取行数)
    let rows: u64 = conn
        .query_row(&format!("SELECT count(*) FROM ({body})"), [], |r| {
            r.get::<usize, i64>(0)
        })
        .map(|v| v as u64)
        .unwrap_or(0);

    Ok(ExportResult {
        path: out_path.to_string_lossy().into_owned(),
        format: match format {
            ExportFormat::Csv => "csv".into(),
            ExportFormat::Parquet => "parquet".into(),
        },
        rows,
        bytes: meta.len(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// 把一个**已是 UTF-8** 的 CSV 文件整体转换为 Parquet。
///
/// 供「编码转换」面板在输出格式选 Parquet 时调用: 源文件先由 `converter`
/// 转码为临时 UTF-8 CSV, 再经 DuckDB `read_csv_auto` + `COPY ... (FORMAT PARQUET)` 落盘。
///
/// 与 [`export_sql_result`] 的区别: 这里不执行用户 SQL, 而是整表转换。
/// `overwrite=false` 且目标已存在时返回错误, 避免误覆盖。
pub fn write_parquet_from_csv(
    csv_path: &Path,
    out_path: &Path,
    overwrite: bool,
) -> Result<ExportResult> {
    let started = Instant::now();

    if out_path.is_dir() {
        return Err(anyhow::anyhow!(
            "输出路径是目录, 请指定文件名: {}",
            out_path.display()
        ));
    }
    if out_path.exists() && !overwrite {
        return Err(anyhow::anyhow!(
            "目标文件已存在 (未勾选覆盖): {}",
            out_path.display()
        ));
    }
    if !csv_path.is_file() {
        return Err(anyhow::anyhow!(
            "待转换的临时 CSV 不存在: {}",
            csv_path.display()
        ));
    }

    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建输出目录失败: {}", parent.display()))?;
        }
    }

    let (conn, _ext) = open_conn()?;

    // 转义单引号, 统一为正斜杠 (DuckDB 在 Windows 上也接受正斜杠)
    let src = csv_path
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "''");
    let dst = out_path
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "''");

    // read_csv_auto 显式按 CSV 解析 (与文件扩展名无关), 再整表写成 Parquet
    let copy_sql =
        format!("COPY (SELECT * FROM read_csv_auto('{src}')) TO '{dst}' (FORMAT PARQUET)");
    conn.execute_batch(&copy_sql).with_context(|| {
        format!(
            "CSV -> Parquet 转换失败: {} -> {}",
            csv_path.display(),
            out_path.display()
        )
    })?;

    let meta = std::fs::metadata(out_path)
        .with_context(|| format!("转换后无法读取文件信息: {}", out_path.display()))?;

    // 行数: COPY 的返回值不足以直接取行数, 再查一次
    let rows: u64 = conn
        .query_row(
            &format!("SELECT count(*) FROM read_csv_auto('{src}')"),
            [],
            |r| r.get::<usize, i64>(0),
        )
        .map(|v| v as u64)
        .unwrap_or(0);

    Ok(ExportResult {
        path: out_path.to_string_lossy().into_owned(),
        format: "parquet".into(),
        rows,
        bytes: meta.len(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// 在文件开头插入 UTF-8 BOM (EF BB BF).
///
/// DuckDB 的 `COPY ... TO` 没有 BOM 选项 (报 "Unrecognized option BOM for csv"),
/// 因此导出后单独补写. CSV 通常不大, 这里整文件读入再重写; 已有 BOM 时幂等返回.
fn prepend_bom(path: &Path) -> Result<()> {
    const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
    let data =
        std::fs::read(path).with_context(|| format!("读取待加 BOM 文件失败: {}", path.display()))?;
    if data.starts_with(&BOM) {
        return Ok(()); // 已有 BOM, 不重复添加
    }
    let mut out = Vec::with_capacity(BOM.len() + data.len());
    out.extend_from_slice(&BOM);
    out.extend_from_slice(&data);
    std::fs::write(path, out).with_context(|| format!("写回 BOM 失败: {}", path.display()))?;
    Ok(())
}

// ===================== 扩展目录 =====================

/// 预置扩展目录 (运行时由 `run()` 注入)。
///
/// **不能**再用 `env!("CARGO_MANIFEST_DIR")`: 那是编译期常量, 会写死构建机路径,
/// 装到别人机器上必然找不到 (表现为静默退化为联网 INSTALL, 离线时直接失败)。
static EXT_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 由应用启动流程注入扩展目录; 重复调用忽略 (只认第一次)。
pub fn set_ext_dir(dir: PathBuf) {
    let _ = EXT_DIR.set(dir);
}

/// 当前生效的扩展目录; 未注入时回退到开发期路径 (便于 `cargo test` / `tauri dev`)。
fn ext_dir() -> PathBuf {
    if let Some(d) = EXT_DIR.get() {
        return d.clone();
    }
    dev_ext_dir()
}

/// 开发期路径: `<src-tauri>/tools/duckdb-extensions/<version>/<platform>`
fn dev_ext_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tools")
        .join("duckdb-extensions")
        .join(EXT_VERSION)
        .join(EXT_PLATFORM)
}

/// 按候选目录依次探测, 返回第一个真实存在的扩展目录。
///
/// 打包后优先用 `resource_dir()/duckdb-extensions`; 开发期回退到 `src-tauri/tools/...`。
pub fn resolve_ext_dir(candidates: impl IntoIterator<Item = PathBuf>) -> PathBuf {
    for base in candidates {
        let dir = base.join(EXT_VERSION).join(EXT_PLATFORM);
        if dir.is_dir() {
            return dir;
        }
    }
    dev_ext_dir()
}

// ===================== 启动时预加载 extension =====================
/// 加载项目内预下载的 DuckDB extensions, 实现离线 SQL.
///
/// 返回每个 extension 的加载结果. 文件缺失或 LOAD 失败不会中断查询
/// (退化为按需联网 INSTALL), 但会把原因回报给调用方, 避免"静默降级"
/// 导致用户直到查询报错才发现离线不可用.
fn preload_extensions(conn: &Connection) -> Vec<ExtensionStatus> {
    let ext_dir = ext_dir();

    let mut report = Vec::with_capacity(PRELOAD_EXTENSIONS.len());
    if !ext_dir.is_dir() {
        let msg = format!("未找到预置扩展目录: {}", ext_dir.display());
        for name in PRELOAD_EXTENSIONS {
            report.push(ExtensionStatus {
                name: (*name).to_string(),
                loaded: false,
                present: false,
                error: Some(msg.clone()),
            });
        }
        return report;
    }

    // allow_unsigned_extensions 已在 Connection::open_in_memory_with_flags 前设置
    for name in PRELOAD_EXTENSIONS {
        let ext_path = ext_dir.join(format!("{name}.duckdb_extension"));
        if !ext_path.is_file() {
            report.push(ExtensionStatus {
                name: (*name).to_string(),
                loaded: false,
                present: false,
                error: Some(format!("扩展文件缺失: {}", ext_path.display())),
            });
            continue;
        }
        // DuckDB SQL 字符串中 \ 需转义为 \\, 这里统一换 /
        let path_str = ext_path.to_string_lossy().replace('\\', "/");
        let sql = format!("LOAD '{path_str}';");
        match conn.execute_batch(&sql) {
            Ok(_) => {
                eprintln!("[sql] LOAD {name} OK");
                report.push(ExtensionStatus {
                    name: (*name).to_string(),
                    loaded: true,
                    present: true,
                    error: None,
                });
            }
            Err(e) => {
                eprintln!("[sql] LOAD {name} 失败 (忽略): {e}");
                report.push(ExtensionStatus {
                    name: (*name).to_string(),
                    loaded: false,
                    present: true,
                    error: Some(e.to_string()),
                });
            }
        }
    }
    report
}

/// 转码缓存的文件名前缀.
const CACHE_PREFIX: &str = "utf8_";

/// 计算源文件对应的 UTF-8 转码缓存路径.
///
/// - `cache_dir = None`（默认）：缓存与源文件**同目录**，名 `utf8_<原文件名>`，
///   如 `D:/data/订单.csv` -> `D:/data/utf8_订单.csv`。同一目录内文件名唯一，天然不冲突。
/// - `cache_dir = Some(d)`：缓存写到 `d` 下，并用**源文件绝对路径的短哈希**建一层子目录
///   隔离（`d/<hash8>/utf8_<原文件名>`）。这样不同数据目录里的同名文件不会互相覆盖，
///   同时文件名仍保持 `utf8_<原文件名>`。
///
/// 默认策略的取舍：缓存不落到系统临时目录，而是与源文件放在一起，路径可预期、
/// 用户能直接看到 / 复用 / 删除，大文件也不必先写进 C 盘临时目录；代价是要求
/// 数据目录可写（只读介质会转码失败，会给出明确报错），此时可用自定义缓存目录规避。
fn cache_path_for(source: &Path, cache_dir: Option<&Path>) -> Result<PathBuf> {
    let name = source
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("数据文件缺少文件名: {}", source.display()))?;
    let mut cache_name = String::with_capacity(CACHE_PREFIX.len() + name.len());
    cache_name.push_str(CACHE_PREFIX);
    cache_name.push_str(&name.to_string_lossy());

    match cache_dir {
        None => {
            let dir = source.parent().ok_or_else(|| {
                anyhow::anyhow!("无法确定数据文件所在目录: {}", source.display())
            })?;
            Ok(dir.join(cache_name))
        }
        Some(dir) => {
            let mut h = DefaultHasher::new();
            source.to_string_lossy().to_lowercase().hash(&mut h);
            let sub = format!("{:08x}", (h.finish() as u32));
            Ok(dir.join(sub).join(cache_name))
        }
    }
}

/// 缓存是否仍然有效: 文件存在, 且其修改时间不早于源文件.
///
/// 源文件被改写 (重新转换 / 覆盖) 后 mtime 会更新, 缓存随即失效并触发重转码.
fn cache_is_fresh(cache: &Path, source: &Path) -> bool {
    let (Ok(cm), Ok(sm)) = (std::fs::metadata(cache), std::fs::metadata(source)) else {
        return false;
    };
    if !cm.is_file() {
        return false;
    }
    match (cm.modified(), sm.modified()) {
        (Ok(c), Ok(s)) => c >= s,
        // 取不到时间戳时保守重转码, 避免用到可能过期的缓存
        _ => false,
    }
}

/// 把任意编码文本文件转为 UTF-8, 并缓存为 `utf8_<原文件名>`.
///
/// `cache_dir` 为 `None` 时缓存到源文件同目录, 否则缓存到指定目录
/// (详见 [`cache_path_for`]). 已是 UTF-8 的文件直接返回原路径, 不复制.
fn ensure_utf8(path: &Path, cache_dir: Option<&Path>) -> Result<(PathBuf, TranscodeStatus)> {
    let (enc, f) = detect(path, 65536).with_context(|| format!("检测编码失败: {}", path.display()))?;
    drop(f);
    if matches!(enc, DetectedEncoding::Utf8 | DetectedEncoding::Utf8Bom) {
        return Ok((path.to_path_buf(), TranscodeStatus::AsciiOrUtf8));
    }
    let out = cache_path_for(path, cache_dir)?;
    if cache_is_fresh(&out, path) {
        return Ok((out, TranscodeStatus::Cached));
    }
    // 自定义缓存目录可能尚不存在 (含哈希子目录), 写前确保父目录存在
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建缓存目录失败: {}", parent.display()))?;
    }
    let opts = ConvertOptions {
        probe_size: 65536,
        write_bom: false,
        retries: 1,
        skip_utf8: false,
        forced_encoding: None,
    };
    convert(path, &out, &opts).with_context(|| {
        format!(
            "转码失败: {} -> {} (缓存目录是否可写?)",
            path.display(),
            out.display()
        )
    })?;
    Ok((out, TranscodeStatus::Transcoded))
}

#[derive(Debug, Clone, Copy)]
enum TranscodeStatus {
    AsciiOrUtf8,
    Cached,
    Transcoded,
}

// ===================== SQL 字符串字面量扫描 =====================

/// 在 SQL 中找出单引号字符串字面量, 返回 (开始字节偏移, 结束字节偏移(不含尾引号), 内容)
/// 正确处理 SQL 标准的 '' 转义 (两个连续单引号表示一个字面单引号)
fn find_string_literals(sql: &str) -> Vec<(usize, usize, String)> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\'' {
            i += 1;
            continue;
        }
        // 找到了一个开引号; 检查是否是 '' 引用 (字符串起始是 '' 是不合法的 SQL 字符串, 但当作 E'' 边界外)
        // 简单策略: 收集内容直到非转义的闭合引号
        let start_content = i + 1;
        let mut j = start_content;
        let mut content = String::new();
        loop {
            if j >= bytes.len() {
                return out; // 未闭合, 终止扫描
            }
            if bytes[j] == b'\'' {
                // 看看是否是 '' 转义
                if j + 1 < bytes.len() && bytes[j + 1] == b'\'' {
                    content.push('\'');
                    j += 2;
                    continue;
                }
                // 闭合引号
                out.push((i, j + 1, content));
                i = j + 1;
                break;
            }
            // 推进一字符 (按 UTF-8 边界)
            let ch_end = (j + 1..=bytes.len())
                .find(|&k| sql.is_char_boundary(k))
                .unwrap_or(bytes.len());
            content.push_str(&sql[j..ch_end]);
            j = ch_end;
        }
    }
    out
}

/// 替换 SQL 中所有"存在的受支持文件字面量"为绝对路径.
///
/// 两类文件处理方式不同:
/// - **文本类** (`csv`/`tsv`/`txt`/`psv`): 先探测编码并转码为 UTF-8 缓存文件
///   (`utf8_<原文件名>`), 再把字面量指向该缓存 (DuckDB 只看到 UTF-8).
/// - **二进制类** (`parquet`/`pq`): 不转码, 仅把相对路径重写为绝对路径,
///   因为 Parquet 自带 schema 与 UTF-8 编码. 相对路径必须重写, 否则
///   DuckDB 会按进程 CWD 解析而找不到文件.
///
/// 通配字面量 (`daily_*.csv`) 只重写路径不转码.
///
/// 注: DuckDB 的裸路径替换扫描只认 `csv`/`tsv` 扩展名, 而缓存保留了原扩展名
/// (如 `utf8_订单.txt`), 因此对非 `csv`/`tsv` 的文本文件会显式包一层
/// `read_csv_auto(...)`, 让 DuckDB 仍按 CSV 解析.
///
/// `cache_dir` 见 [`cache_path_for`]: `None` = 数据文件同目录 (默认).
///
/// 返回 (新 SQL, 新转码文件列表, 缓存命中列表, 备注)
fn rewrite_sql_file_literals(
    sql: &str,
    data_dir: &Path,
    cache_dir: Option<&Path>,
) -> Result<(String, Vec<String>, Vec<String>, Vec<String>)> {
    let literals = find_string_literals(sql);
    if literals.is_empty() {
        return Ok((sql.to_string(), Vec::new(), Vec::new(), Vec::new()));
    }

    // 一次性从右往左替换, 不影响前面索引
    let mut rewritten = sql.to_string();
    let mut transcoded = Vec::<String>::new();
    let mut cached = Vec::<String>::new();
    let mut notes = Vec::<String>::new();

    let mut entries: Vec<(usize, usize, String)> = literals;
    entries.sort_by(|a, b| b.0.cmp(&a.0));

    for (start, end, content) in entries {
        let Some((file_path, kind)) = resolve_data_file(&content, data_dir) else {
            continue;
        };

        // 文本类需要先转码为 UTF-8; 二进制类 (parquet) 直接用绝对路径,
        // 因为它自带编码与 schema, 转码既无意义也会破坏文件.
        let replacement_path = match kind {
            LiteralKind::Text => {
                // 通配模式不做转码 (无法预先转码未知文件集), 仅重写为绝对路径;
                // DuckDB 会自己展开并读取原文件. 用户需保证这些文件已是 UTF-8.
                if is_glob(&content) {
                    notes.push(format!(
                        "{} (通配, 未转码, 需为 UTF-8) -> {}",
                        content,
                        file_path.display()
                    ));
                    file_path.clone()
                } else {
                    let (p, status) = ensure_utf8(&file_path, cache_dir)?;
                    let display = file_path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| file_path.display().to_string());
                    match status {
                        TranscodeStatus::Transcoded => transcoded.push(display),
                        TranscodeStatus::Cached => cached.push(display),
                        TranscodeStatus::AsciiOrUtf8 => {}
                    }
                    p
                }
            }
            LiteralKind::Binary => {
                notes.push(format!("{} (parquet, 直接读取)", file_path.display()));
                file_path.clone()
            }
        };

        // 把 rewritten[start..end] 替换为带引号的绝对路径
        // 注意: 如果原内容有特殊字符, 我们按 SQL 标准双写单引号
        let escaped = replacement_path
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''");

        // DuckDB 裸路径替换扫描只认 csv/tsv, 缓存保留原扩展名, 故对非 csv/tsv 的
        // 文本文件包一层 read_csv_auto; 通配、二进制类、以及用户已显式写
        // read_xxx('...') 的参数不包裹 (后者只替换其中的路径字符串).
        let native_ext = matches!(
            replacement_path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase())
                .as_deref(),
            Some("csv") | Some("tsv")
        );
        let wrap = matches!(kind, LiteralKind::Text)
            && !is_glob(&content)
            && !native_ext
            && !is_function_arg(sql, start);

        let new_lit = if wrap {
            format!("read_csv_auto('{}')", escaped)
        } else {
            format!("'{}'", escaped)
        };
        rewritten.replace_range(start..end, &new_lit);
    }

    Ok((rewritten, transcoded, cached, notes))
}

/// 判断一个字面量是否作为"函数参数"出现: 紧邻其左侧的非空白字符是 `(` 或 `,`.
///
/// 用于避免对 `read_csv_auto('x.txt', header=true)` 这类用户已显式指定读取器的
/// 写法重复包裹 (否则会变成 `read_csv_auto(read_csv_auto('x.txt'), header=true)`).
fn is_function_arg(sql: &str, lit_start: usize) -> bool {
    let bytes = sql.as_bytes();
    let mut i = lit_start;
    while i > 0 {
        i -= 1;
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            continue;
        }
        return b == b'(' || b == b',';
    }
    false
}

/// 字面量解析结果: 该字面量指向什么类型的文件
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiteralKind {
    /// 文本类 (csv/tsv/...), 需要转码
    Text,
    /// 二进制类 (parquet/pq), 无需转码, 仅重写路径
    Binary,
}

/// 解析 SQL 字面量为绝对路径, 仅当扩展名受支持且文件真实存在时返回.
///
/// 支持通配 (如 `daily_*.csv`): 只要 glob 能匹配到至少一个文件就接受,
/// 路径重写交给 DuckDB 自己展开.
/// 返回 (绝对路径, 类型). 绝对路径中的 `\` 已换成 `/`.
fn resolve_data_file(literal: &str, data_dir: &Path) -> Option<(PathBuf, LiteralKind)> {
    if literal.is_empty() {
        return None;
    }
    let ext = Path::new(literal)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())?;

    let kind = if DATA_EXTS.contains(&ext.as_str()) {
        LiteralKind::Text
    } else if BINARY_EXTS.contains(&ext.as_str()) {
        LiteralKind::Binary
    } else {
        return None;
    };

    // 解析路径 (绝对 vs 相对); 统一使用 / 以适配 DuckDB
    let p = Path::new(literal);
    let cand = if p.is_absolute() {
        p.to_path_buf()
    } else {
        data_dir.join(literal)
    };

    // 存在性检查: 支持通配模式
    let exists = if is_glob(literal) {
        glob_matches(&cand, data_dir)
    } else {
        cand.is_file()
    };
    if !exists {
        return None;
    }
    Some((cand, kind))
}

/// 字面量是否包含通配符
fn is_glob(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

/// 判断 glob 模式能否匹配到至少一个文件.
///
/// DuckDB 的 glob 语义包含 `**` (递归), 这里为判断"是否存在"做保守近似:
/// 先按标准 glob 匹配, 若失败且含 `**`, 则在数据目录下递归查找。
fn glob_matches(pattern: &Path, data_dir: &Path) -> bool {
    let pat = pattern.to_string_lossy().replace('\\', "/");
    if let Ok(mut it) = glob::glob(&pat) {
        if it.any(|m| m.map(|p| p.is_file()).unwrap_or(false)) {
            return true;
        }
    }
    // `**` 兜底: 递归遍历数据目录, 用文件名部分做匹配
    if pat.contains("**") {
        let file_pat = pat.rsplit('/').next().unwrap_or(&pat);
        if let Ok(it) = glob::glob(&format!("{}/{file_pat}",
            data_dir.to_string_lossy().replace('\\', "/"))) {
            if it.into_iter().any(|m| m.map(|p| p.is_file()).unwrap_or(false)) {
                return true;
            }
        }
        // 再退一步: 目录内递归按扩展名找
        let ext = Path::new(file_pat)
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase());
        if let Some(ext) = ext {
            let hit = walkdir::WalkDir::new(data_dir)
                .max_depth(8)
                .into_iter()
                .filter_map(|e| e.ok())
                .any(|e| {
                    e.file_type().is_file()
                        && e.path()
                            .extension()
                            .and_then(|s| s.to_str())
                            .map(|s| s.eq_ignore_ascii_case(&ext))
                            .unwrap_or(false)
                });
            if hit {
                return true;
            }
        }
    }
    false
}

// ===================== Value 格式化 =====================

/// 把 DuckDB 的动态类型值序列化为表格单元格的字符串
fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Boolean(b) => b.to_string(),
        Value::TinyInt(n) => n.to_string(),
        Value::SmallInt(n) => n.to_string(),
        Value::Int(n) => n.to_string(),
        Value::BigInt(n) => n.to_string(),
        Value::HugeInt(n) => n.to_string(),
        Value::UHugeInt(n) => n.to_string(),
        Value::UTinyInt(n) => n.to_string(),
        Value::USmallInt(n) => n.to_string(),
        Value::UInt(n) => n.to_string(),
        Value::UBigInt(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Double(f) => f.to_string(),
        Value::Decimal(d) => d.to_string(),
        Value::Text(s) => s.clone(),
        Value::Blob(b) => format!("<blob {} bytes>", b.len()),
        Value::Geometry(b) => format!("<geometry {} bytes>", b.len()),
        Value::Date32(d) => format!("<date +{} days>", d),
        Value::Time64(unit, t) => {
            let u = match unit {
                TimeUnit::Second => "s",
                TimeUnit::Millisecond => "ms",
                TimeUnit::Microsecond => "us",
                TimeUnit::Nanosecond => "ns",
            };
            format!("<time {} {}>", t, u)
        }
        Value::Timestamp(unit, t) => {
            let u = match unit {
                TimeUnit::Second => "s",
                TimeUnit::Millisecond => "ms",
                TimeUnit::Microsecond => "us",
                TimeUnit::Nanosecond => "ns",
            };
            format!("<timestamp {} {}>", t, u)
        }
        Value::Interval { months, days, nanos } => {
            format!("<interval {}m {}d {}ns>", months, days, nanos)
        }
        Value::Enum(s) => s.clone(),
        // 容器类型用 Debug 格式
        _ => format!("{:?}", v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_glob_detects_wildcards() {
        assert!(is_glob("*.csv"));
        assert!(is_glob("data/*.parquet"));
        assert!(is_glob("a?.csv"));
        assert!(is_glob("**/*.csv"));
        assert!(is_glob("data/[0-9].csv"));
        // 普通文件名 / 路径不是通配
        assert!(!is_glob("orders.csv"));
        assert!(!is_glob("data/orders.csv"));
        assert!(!is_glob(""));
    }

    #[test]
    fn value_to_string_formats_scalars_and_null() {
        assert_eq!(value_to_string(&Value::Null), "NULL");
        assert_eq!(value_to_string(&Value::Boolean(true)), "true");
        assert_eq!(value_to_string(&Value::Int(42)), "42");
        assert_eq!(value_to_string(&Value::Text("hi".into())), "hi");
        assert_eq!(
            value_to_string(&Value::Blob(vec![1, 2, 3])),
            "<blob 3 bytes>"
        );
    }

    #[test]
    fn cache_path_keeps_dir_and_adds_prefix() {
        // 默认 (None): 缓存与源文件同目录, 名加 utf8_ 前缀, 保留原扩展名
        let p = cache_path_for(Path::new("D:/data/订单.csv"), None).unwrap();
        assert_eq!(p, PathBuf::from("D:/data/utf8_订单.csv"));
        let p2 = cache_path_for(Path::new("D:/data/notes.txt"), None).unwrap();
        assert_eq!(p2, PathBuf::from("D:/data/utf8_notes.txt"));
        assert_eq!(p2.parent(), Some(Path::new("D:/data")));
    }

    #[test]
    fn cache_path_errors_without_file_name() {
        // 目录形式 (无文件名) 应报错
        assert!(cache_path_for(Path::new("D:/"), None).is_err());
    }

    #[test]
    fn cache_path_custom_dir_isolates_same_named_sources() {
        let cache = Path::new("D:/cache");
        let a = cache_path_for(Path::new("D:/a/t.csv"), Some(cache)).unwrap();
        let b = cache_path_for(Path::new("D:/b/t.csv"), Some(cache)).unwrap();
        // 文件名仍保持 utf8_<原文件名>
        assert_eq!(a.file_name().unwrap(), "utf8_t.csv");
        assert_eq!(b.file_name().unwrap(), "utf8_t.csv");
        // 都落在自定义缓存目录下
        assert!(a.starts_with(cache) && b.starts_with(cache));
        // 不同源目录的同名文件用不同子目录隔离, 不互相覆盖
        assert_ne!(a.parent(), b.parent());
        // 同一源文件稳定映射到同一路径 (缓存可复用)
        assert_eq!(cache_path_for(Path::new("D:/a/t.csv"), Some(cache)).unwrap(), a);
    }

    #[test]
    fn is_function_arg_detects_call_context() {
        let sql = "SELECT * FROM read_csv_auto('a.txt', header=true)";
        let pos = sql.find("'a.txt'").unwrap();
        assert!(is_function_arg(sql, pos));
        // 逗号后 (第二个参数) 也算函数参数
        let sql2 = "read_csv_auto(x, 'b.txt')";
        assert!(is_function_arg(sql2, sql2.find("'b.txt'").unwrap()));
        // FROM 后 (替换扫描) 不算函数参数 -> 需要包裹
        let sql3 = "SELECT * FROM 'a.txt'";
        assert!(!is_function_arg(sql3, sql3.find("'a.txt'").unwrap()));
        // 行首字面量
        assert!(!is_function_arg("'x.csv'", 0));
    }
}