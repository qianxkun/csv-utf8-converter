//! CSV UTF-8 转换器 - Tauri 2.0 后端
//!
//! 暴露给前端的命令:
//! - `preview_files`: 逐文件预览 (原始编码 + 表头 + 前 N 行数据), 供用户勾选后再转换
//! - `start_conversion`: 开始批量转换 (异步, 通过事件回报进度)
//! - `cancel_conversion`: 取消正在进行的任务
//! - `scan_files`: 仅统计待处理文件数与总字节数
//! - `run_sql` / `export_sql_result`: DuckDB 查询与结果导出

pub mod converter;
pub mod detector;
pub mod preview;
pub mod sql;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use walkdir::WalkDir;

use converter::{convert, ConvertOptions};
use detector::DetectedEncoding;

// ============ 输入 / 输出约束 ============

/// 编码转换**仅接受**的输入扩展名 (大小写不敏感)。
///
/// 需求: 转换功能只处理 CSV 与 TXT, 输出统一为 UTF-8 CSV 或 Parquet。
const CONVERT_EXTS: &[&str] = &["csv", "txt"];

/// 编码转换的输出格式 (统一输出, 与输入扩展名无关)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// UTF-8 CSV (文本, 可选 BOM)
    Csv,
    /// Parquet (列式二进制, 经 DuckDB 落盘)
    Parquet,
}

impl OutputFormat {
    /// 解析前端传入的格式字符串, 非法值回退为 CSV
    fn from_opt(value: Option<&str>) -> Self {
        match value.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("parquet") | Some("pq") => Self::Parquet,
            _ => Self::Csv,
        }
    }

    /// 输出文件扩展名 (不含点)
    fn ext(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Parquet => "parquet",
        }
    }

    /// 界面提示用名称
    fn label(self) -> &'static str {
        match self {
            Self::Csv => "UTF-8 CSV",
            Self::Parquet => "Parquet",
        }
    }
}

/// 判断文件名是否属于可转换类型 (.csv / .txt, 大小写不敏感)
fn is_convertible(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((_, ext)) => CONVERT_EXTS.iter().any(|e| ext.eq_ignore_ascii_case(e)),
        None => false,
    }
}

// ============ 命令参数 ============

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionOptions {
    /// 输出格式: "csv" (默认) 或 "parquet"
    pub output_format: Option<String>,
    pub recursive: Option<bool>,
    pub jobs: Option<usize>,
    pub probe_size: Option<usize>,
    pub write_bom: Option<bool>,
    pub skip_utf8: Option<bool>,
    pub retries: Option<usize>,
    pub overwrite: Option<bool>,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            output_format: Some("csv".into()),
            recursive: Some(true),
            jobs: Some(0),
            probe_size: Some(65536),
            write_bom: Some(false),
            skip_utf8: Some(false),
            retries: Some(2),
            overwrite: Some(false),
        }
    }
}

// ============ 事件 payload ============

#[derive(Debug, Clone, Serialize)]
pub struct StartEvent {
    pub total: usize,
    pub total_bytes: u64,
    pub input_dir: String,
    pub output_dir: String,
    /// 输出格式提示, 如 "UTF-8 CSV" / "Parquet"
    pub format: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDoneEvent {
    pub done: usize,
    pub total: usize,
    pub failed: usize,
    pub skipped: usize,
    pub file: String,
    pub status: String, // success | failed | skipped
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub encoding: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgressEvent {
    pub done: usize,
    pub total: usize,
    pub failed: usize,
    pub skipped: usize,
    pub bytes_in: u64,
    pub throughput_mb_s: f64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompleteEvent {
    pub success: usize,
    pub failed: usize,
    pub skipped: usize,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub duration_ms: u64,
    pub cancelled: bool,
}

// ============ 全局状态 ============

#[derive(Default)]
pub struct AppState {
    pub cancel_flag: Arc<AtomicBool>,
    pub is_running: Arc<AtomicBool>,
}

// ============ Tauri 命令 ============

/// scan_files 返回结构: 文件数 + 总字节数
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub count: usize,
    pub total_bytes: u64,
}

#[tauri::command]
async fn scan_files(input_dir: String, recursive: bool) -> Result<ScanResult, String> {
    let input = PathBuf::from(&input_dir);
    if !input.is_dir() {
        return Err(format!("输入路径不存在或不是目录: {}", input_dir));
    }
    let files =
        collect_files(&input, recursive).map_err(|e| format!("扫描失败: {}", e))?;
    let total_bytes = total_bytes_of(&files);
    Ok(ScanResult {
        count: files.len(),
        total_bytes,
    })
}

/// 逐文件预览: 返回原始编码 + 分隔符 + 表头 + 前 N 行数据。
///
/// 前端据此判断哪些文件需要转码, 勾选后再调用 `start_conversion`。
///
/// - `encodings`: 用户手动指定的编码 `{ 绝对路径: 编码 id }`。
///   命中时该文件跳过自动探测结果, 用于修正 chardetng 的误判。
/// - `files`: 只预览指定的文件 (前端改动单个文件的编码后局部刷新用);
///   缺省则扫描整个目录。
#[tauri::command]
async fn preview_files(
    input_dir: String,
    recursive: bool,
    sample_rows: Option<usize>,
    probe_size: Option<usize>,
    encodings: Option<HashMap<String, String>>,
    files: Option<Vec<String>>,
) -> Result<preview::PreviewResult, String> {
    let input = PathBuf::from(&input_dir);
    if !input.is_dir() {
        return Err(format!("输入路径不存在或不是目录: {}", input_dir));
    }
    let rows = sample_rows.unwrap_or(5).clamp(1, 20);
    let probe = probe_size.unwrap_or(65536);
    let overrides = parse_encoding_overrides(encodings);

    tauri::async_runtime::spawn_blocking(move || {
        let discovered = collect_files(&input, recursive).map_err(|e| format!("扫描失败: {}", e))?;
        let files = match files {
            Some(selected) => {
                let wanted: HashSet<PathBuf> = selected
                    .iter()
                    .map(|s| canonical_or_raw(Path::new(s)))
                    .collect();
                discovered
                    .into_iter()
                    .filter(|p| wanted.contains(&canonical_or_raw(p)))
                    .collect()
            }
            None => discovered,
        };
        Ok(preview::build(&input, files, rows, probe, &overrides))
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 把前端传来的 `{ 路径: 编码 id }` 解析成内部映射。
///
/// 路径统一走 `canonical_or_raw`, 与扫描结果对齐; 无法识别的编码 id 直接忽略
/// (宁可回退自动探测, 也不要让一个坏值让整个预览失败)。
fn parse_encoding_overrides(
    raw: Option<HashMap<String, String>>,
) -> HashMap<PathBuf, DetectedEncoding> {
    let mut out = HashMap::new();
    if let Some(map) = raw {
        for (path, id) in map {
            if let Some(enc) = DetectedEncoding::from_id(&id) {
                out.insert(canonical_or_raw(Path::new(&path)), enc);
            }
        }
    }
    out
}

/// 开始批量转换。
///
/// `files` 为前端勾选的文件绝对路径列表:
/// - `Some(_)` → 只处理勾选的文件 (空列表 = 不处理任何文件)
/// - `None`    → 未传该参数, 保持旧行为: 处理目录下全部可转换文件
#[tauri::command]
async fn start_conversion(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    input_dir: String,
    output_dir: String,
    options: ConversionOptions,
    files: Option<Vec<String>>,
    encodings: Option<HashMap<String, String>>,
) -> Result<(), String> {
    // 防重入
    if state.is_running.swap(true, Ordering::SeqCst) {
        return Err("已有任务正在运行".into());
    }
    state.cancel_flag.store(false, Ordering::SeqCst);

    let input = PathBuf::from(&input_dir);
    let output = PathBuf::from(&output_dir);
    if !input.is_dir() {
        state.is_running.store(false, Ordering::SeqCst);
        return Err(format!("输入路径不存在或不是目录: {}", input_dir));
    }
    if input == output {
        state.is_running.store(false, Ordering::SeqCst);
        return Err("输入与输出不能是同一路径 (源文件不被修改)".into());
    }

    // 输出格式 (统一输出, 与输入扩展名无关)
    let format = OutputFormat::from_opt(options.output_format.as_deref());

    // 收集文件 (仅 .csv / .txt)
    let recursive = options.recursive.unwrap_or(true);

    let discovered = match collect_files(&input, recursive) {
        Ok(f) => f,
        Err(e) => {
            state.is_running.store(false, Ordering::SeqCst);
            return Err(format!("扫描失败: {}", e));
        }
    };

    // 勾选列表:
    // - `Some(_)` → 只处理勾选的文件 (空列表 = 不处理任何文件)
    // - `None`    → 未传该参数, 保持旧行为: 处理目录下全部可转换文件
    //
    // 以「重新扫描得到的文件」为基准做交集, 而不是直接使用前端传来的路径,
    // 这样即使前端传入目录外的路径也不会被处理。
    let files: Vec<PathBuf> = match files {
        Some(selected) => {
            let wanted: HashSet<PathBuf> = selected
                .iter()
                .map(|s| canonical_or_raw(Path::new(s)))
                .collect();
            discovered
                .into_iter()
                .filter(|p| wanted.contains(&canonical_or_raw(p)))
                .collect()
        }
        None => discovered,
    };

    let total = files.len();
    if total == 0 {
        let _ = app.emit(
            "conversion:complete",
            CompleteEvent {
                success: 0,
                failed: 0,
                skipped: 0,
                bytes_in: 0,
                bytes_out: 0,
                duration_ms: 0,
                cancelled: false,
            },
        );
        state.is_running.store(false, Ordering::SeqCst);
        return Ok(());
    }

    // 估算总字节数
    let probe_size = options.probe_size.unwrap_or(65536).min(1024 * 1024);
    let total_bytes = total_bytes_of(&files);

    // 启动事件
    let _ = app.emit(
        "conversion:start",
        StartEvent {
            total,
            total_bytes,
            input_dir: input_dir.clone(),
            output_dir: output_dir.clone(),
            format: format.label().to_string(),
        },
    );

    // 准备参数
    // Parquet 输出时: 临时 UTF-8 CSV 不写 BOM, 且必须真实转码 (skip_utf8 无意义)
    let is_parquet = format == OutputFormat::Parquet;
    let convert_opts = ConvertOptions {
        probe_size,
        write_bom: !is_parquet && options.write_bom.unwrap_or(false),
        retries: options.retries.unwrap_or(2),
        skip_utf8: !is_parquet && options.skip_utf8.unwrap_or(false),
        // 逐文件覆盖, 见 run_conversion 内部
        forced_encoding: None,
    };
    let overrides = parse_encoding_overrides(encodings);
    let overwrite = options.overwrite.unwrap_or(false);
    let jobs = options.jobs.unwrap_or(0);

    let cancel_flag = Arc::clone(&state.cancel_flag);
    let is_running = Arc::clone(&state.is_running);

    // 配置 rayon 线程数 (仅当用户指定)
    if jobs > 0 {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build_global();
    }

    // 调度到 blocking 线程
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        run_conversion(
            app_for_task,
            files,
            input,
            output,
            convert_opts,
            overrides,
            format,
            overwrite,
            cancel_flag,
            is_running,
        );
    });

    Ok(())
}

#[tauri::command]
async fn cancel_conversion(state: tauri::State<'_, AppState>) -> Result<(), String> {
    if !state.is_running.load(Ordering::SeqCst) {
        return Err("当前没有运行中的任务".into());
    }
    state.cancel_flag.store(true, Ordering::SeqCst);
    Ok(())
}

// ============ 内部执行函数 ============

fn run_conversion(
    app: AppHandle,
    files: Vec<PathBuf>,
    input_root: PathBuf,
    output_root: PathBuf,
    opts: ConvertOptions,
    overrides: HashMap<PathBuf, DetectedEncoding>,
    format: OutputFormat,
    overwrite: bool,
    cancel_flag: Arc<AtomicBool>,
    is_running: Arc<AtomicBool>,
) {
    let total = files.len();
    let counter = Arc::new(Counter::default());
    let last_emit = Arc::new(Mutex::new(Instant::now()));
    let start_time = Instant::now();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        files.par_iter().for_each(|input_path| {
            if cancel_flag.load(Ordering::Relaxed) {
                return;
            }

            // 计算输出路径 (扩展名统一为所选输出格式)
            let output_path =
                match compute_output_path(input_path, &input_root, &output_root, format.ext()) {
                Ok(p) => p,
                Err(e) => {
                    emit_file_done(&app, &counter, total, FileDoneEvent {
                        done: 0,
                        total,
                        failed: counter.failed.load(Ordering::Relaxed) + 1,
                        skipped: counter.skipped.load(Ordering::Relaxed),
                        file: input_path.display().to_string(),
                        status: "failed".into(),
                        bytes_in: 0,
                        bytes_out: 0,
                        encoding: "?".into(),
                        error: Some(e.to_string()),
                    });
                    return;
                }
            };

            // 跳过已存在
            if !overwrite && output_path.exists() {
                counter.skipped.fetch_add(1, Ordering::Relaxed);
                emit_file_done(&app, &counter, total, FileDoneEvent {
                    done: 0,
                    total,
                    failed: counter.failed.load(Ordering::Relaxed),
                    skipped: counter.skipped.load(Ordering::Relaxed),
                    file: input_path.display().to_string(),
                    status: "skipped".into(),
                    bytes_in: 0,
                    bytes_out: 0,
                    encoding: "?".into(),
                    error: Some("输出文件已存在 (未勾选覆盖)".into()),
                });
                return;
            }

            // 执行转换 (统一输出为 UTF-8 CSV 或 Parquet)
            // 用户在该文件上手动指定过编码时, 覆盖自动探测结果,
            // 保证「预览看到的编码」与「实际按什么编码转」一致。
            let mut file_opts = opts.clone();
            file_opts.forced_encoding = overrides.get(&canonical_or_raw(input_path)).copied();
            let outcome = match format {
                OutputFormat::Csv => convert(input_path, &output_path, &file_opts),
                OutputFormat::Parquet => {
                    convert_to_parquet(input_path, &output_path, &file_opts, overwrite)
                }
            };
            match outcome {
                Ok(r) => {
                    counter.bytes_in.fetch_add(r.bytes_in, Ordering::Relaxed);
                    counter.bytes_out.fetch_add(r.bytes_out, Ordering::Relaxed);
                    // skip_utf8 命中: 已是 UTF-8, 原样复制到输出, 计为 skipped
                    let status = if r.skipped { "skipped" } else { "success" };
                    if r.skipped {
                        counter.skipped.fetch_add(1, Ordering::Relaxed);
                    } else {
                        counter.success.fetch_add(1, Ordering::Relaxed);
                    }
                    emit_file_done(&app, &counter, total, FileDoneEvent {
                        done: 0,
                        total,
                        failed: counter.failed.load(Ordering::Relaxed),
                        skipped: counter.skipped.load(Ordering::Relaxed),
                        file: input_path.display().to_string(),
                        status: status.into(),
                        bytes_in: r.bytes_in,
                        bytes_out: r.bytes_out,
                        encoding: r.encoding.name().into(),
                        error: if r.skipped {
                            Some("已是 UTF-8, 已原样复制到输出".into())
                        } else {
                            None
                        },
                    });
                }
                Err(e) => {
                    counter.failed.fetch_add(1, Ordering::Relaxed);
                    emit_file_done(&app, &counter, total, FileDoneEvent {
                        done: 0,
                        total,
                        failed: counter.failed.load(Ordering::Relaxed),
                        skipped: counter.skipped.load(Ordering::Relaxed),
                        file: input_path.display().to_string(),
                        status: "failed".into(),
                        bytes_in: 0,
                        bytes_out: 0,
                        encoding: "?".into(),
                        error: Some(format!("{:#}", e)),
                    });
                }
            }

            // 节流进度事件 (~100ms)
            let should_emit = {
                let mut last = last_emit.lock().unwrap();
                if last.elapsed() >= Duration::from_millis(100) {
                    *last = Instant::now();
                    true
                } else {
                    false
                }
            };
            if should_emit {
                let elapsed_ms = start_time.elapsed().as_millis() as u64;
                let bytes_in = counter.bytes_in.load(Ordering::Relaxed);
                let throughput_mb_s = if elapsed_ms > 0 {
                    (bytes_in as f64 / 1024.0 / 1024.0) / (elapsed_ms as f64 / 1000.0)
                } else {
                    0.0
                };
                let _ = app.emit(
                    "conversion:progress",
                    ProgressEvent {
                        done: counter.done_count(),
                        total,
                        failed: counter.failed.load(Ordering::Relaxed),
                        skipped: counter.skipped.load(Ordering::Relaxed),
                        bytes_in,
                        throughput_mb_s,
                        elapsed_ms,
                    },
                );
            }
        });
    }));

    let cancelled = cancel_flag.load(Ordering::Relaxed);
    let success = counter.success.load(Ordering::Relaxed);
    let failed = counter.failed.load(Ordering::Relaxed);
    let skipped = counter.skipped.load(Ordering::Relaxed);
    let bytes_in = counter.bytes_in.load(Ordering::Relaxed);
    let bytes_out = counter.bytes_out.load(Ordering::Relaxed);

    if result.is_err() {
        eprintln!("[fatal] conversion worker panicked");
    }

    let _ = app.emit(
        "conversion:complete",
        CompleteEvent {
            success,
            failed,
            skipped,
            bytes_in,
            bytes_out,
            duration_ms: start_time.elapsed().as_millis() as u64,
            cancelled,
        },
    );

    is_running.store(false, Ordering::SeqCst);
}

#[derive(Default)]
struct Counter {
    success: AtomicUsize,
    failed: AtomicUsize,
    skipped: AtomicUsize,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
}

impl Counter {
    fn done_count(&self) -> usize {
        self.success.load(Ordering::Relaxed)
            + self.failed.load(Ordering::Relaxed)
            + self.skipped.load(Ordering::Relaxed)
    }
}

fn emit_file_done(app: &AppHandle, counter: &Counter, _total: usize, mut evt: FileDoneEvent) {
    // 调用前计数器已自增, done 直接取已完成总数 (不再 +1)
    evt.done = counter.done_count();
    let _ = app.emit("conversion:file-done", evt);
}

// ============ 文件收集 ============

/// 递归收集输入目录下**仅限** `.csv` / `.txt` 的文件 (大小写不敏感)。
fn collect_files(input: &Path, recursive: bool) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let walker = if recursive {
        WalkDir::new(input)
    } else {
        WalkDir::new(input).max_depth(1)
    };

    for entry in walker.into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if is_convertible(name) {
            files.push(path);
        }
    }
    Ok(files)
}

/// 规范化路径以便比较; 失败时退回原路径 (不阻断流程)。
///
/// 前端传来的路径与扫描得到的路径可能一个已规范化、一个没有,
/// 统一处理后才能可靠求交集。
fn canonical_or_raw(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// 计算输出路径: 保留相对目录结构, 扩展名统一替换为所选输出格式。
fn compute_output_path(
    input: &Path,
    input_root: &Path,
    output_root: &Path,
    ext: &str,
) -> anyhow::Result<PathBuf> {
    let rel = input
        .strip_prefix(input_root)
        .map_err(|_| anyhow::anyhow!("路径不在输入目录下: {}", input.display()))?;
    let mut out = output_root.join(rel);
    out.set_extension(ext);
    Ok(out)
}

/// CSV → Parquet 输出: 先把任意编码的源文件转码为临时 UTF-8 CSV,
/// 再交给 DuckDB `COPY (...) TO ... (FORMAT PARQUET)` 落盘, 最后清理临时文件。
///
/// 临时文件与最终产物同目录, 命名为 `.<name>.<pid>.<nanos>.tmp`
/// (非 `.csv` / `.txt` 结尾, 不会被后续扫描再次拾取)。
fn convert_to_parquet(
    input: &Path,
    output: &Path,
    opts: &ConvertOptions,
    overwrite: bool,
) -> anyhow::Result<converter::ConvertResult> {
    // 1) 转码为临时 UTF-8 CSV (不写 BOM, 交给 DuckDB 解析)
    let tmp_csv = converter::tmp_path_for(output);
    let _ = std::fs::remove_file(&tmp_csv);

    let csv_opts = ConvertOptions {
        probe_size: opts.probe_size,
        write_bom: false,
        retries: opts.retries,
        skip_utf8: false,
        // 用户手动指定的编码必须一路传到转码阶段, 否则预览与产物会不一致
        forced_encoding: opts.forced_encoding,
    };

    let transcoded = match convert(input, &tmp_csv, &csv_opts) {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_csv);
            return Err(e);
        }
    };

    // 2) 临时 UTF-8 CSV → Parquet
    let parquet = sql::write_parquet_from_csv(&tmp_csv, output, overwrite);
    let _ = std::fs::remove_file(&tmp_csv);
    let parquet = parquet?;

    Ok(converter::ConvertResult {
        bytes_in: transcoded.bytes_in,
        bytes_out: parquet.bytes,
        encoding: transcoded.encoding,
        skipped: false,
    })
}

fn total_bytes_of(files: &[PathBuf]) -> u64 {
    files
        .par_iter()
        .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .sum()
}

// ============ SQL 查询 ============

#[tauri::command]
async fn run_sql(
    data_dir: String,
    sql: String,
    limit: Option<usize>,
    cache_dir: Option<String>,
) -> Result<sql::SqlResult, String> {
    let dir = PathBuf::from(&data_dir);
    if !dir.is_dir() {
        return Err(format!("数据目录不存在: {}", data_dir));
    }
    let cache = normalize_cache_dir(cache_dir);
    tauri::async_runtime::spawn_blocking(move || {
        sql::run_sql_with_cache(&dir, &sql, limit.unwrap_or(1000), cache.as_deref())
            .map_err(|e| format!("{:#}", e))
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 规范化前端传来的缓存目录: 空串/纯空白视为"未指定"(None, 即用数据文件同目录).
fn normalize_cache_dir(raw: Option<String>) -> Option<PathBuf> {
    raw.filter(|s| !s.trim().is_empty())
        .map(|s| PathBuf::from(s.trim()))
}

/// 把 SQL 查询结果导出为文件 (UTF-8 CSV 或 Parquet).
///
/// 注意: 导出会**重新执行** `sql` 并全量落盘, 因此不受界面 1000 行预览截断影响.
#[tauri::command]
async fn export_sql_result(
    data_dir: String,
    sql: String,
    out_path: String,
    overwrite: Option<bool>,
    write_bom: Option<bool>,
    cache_dir: Option<String>,
) -> Result<sql::ExportResult, String> {
    let dir = PathBuf::from(&data_dir);
    if !dir.is_dir() {
        return Err(format!("数据目录不存在: {}", data_dir));
    }
    let out = PathBuf::from(&out_path);
    let overwrite = overwrite.unwrap_or(false);
    let write_bom = write_bom.unwrap_or(false);
    let cache = normalize_cache_dir(cache_dir);

    tauri::async_runtime::spawn_blocking(move || {
        sql::export_sql_result_with_cache(&dir, &sql, &out, overwrite, write_bom, cache.as_deref())
            .map_err(|e| format!("{:#}", e))
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

// ============ run 入口 ============

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        // DuckDB 扩展目录必须在任何 SQL 调用之前确定。
        // 打包后位于 resource_dir/duckdb-extensions, 开发期回退到 src-tauri/tools/...
        .setup(|app| {
            let mut candidates = Vec::new();
            if let Ok(res) = app.path().resource_dir() {
                // 数组形式 resources 会保留相对结构 → $RESOURCE/tools/duckdb-extensions
                candidates.push(res.join("tools").join("duckdb-extensions"));
                // 兼容映射形式 (若日后改成 resources: { "tools/duckdb-extensions/": "duckdb-extensions/" })
                candidates.push(res.join("duckdb-extensions"));
            }
            candidates.push(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tools")
                    .join("duckdb-extensions"),
            );
            sql::set_ext_dir(sql::resolve_ext_dir(candidates));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_conversion,
            cancel_conversion,
            scan_files,
            preview_files,
            run_sql,
            export_sql_result,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event {
                let state = window.state::<AppState>();
                if state.is_running.load(Ordering::SeqCst) {
                    state.cancel_flag.store(true, Ordering::SeqCst);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convertible_extensions_are_case_insensitive() {
        assert!(is_convertible("a.csv"));
        assert!(is_convertible("a.CSV"));
        assert!(is_convertible("DATA.TXT"));
        assert!(is_convertible("archive.tar.txt"));
        // 非目标类型
        assert!(!is_convertible("a.parquet"));
        assert!(!is_convertible("a.tsv"));
        assert!(!is_convertible("a.psv"));
        // 无扩展名 / 仅有点
        assert!(!is_convertible("noext"));
        assert!(!is_convertible("trailing."));
    }

    #[test]
    fn output_format_parsing_is_case_insensitive_with_csv_fallback() {
        assert_eq!(OutputFormat::from_opt(Some("parquet")), OutputFormat::Parquet);
        assert_eq!(OutputFormat::from_opt(Some("PQ")), OutputFormat::Parquet);
        assert_eq!(
            OutputFormat::from_opt(Some("  Parquet ")),
            OutputFormat::Parquet
        );
        assert_eq!(OutputFormat::from_opt(Some("csv")), OutputFormat::Csv);
        // 缺省 / 非法值一律回退 CSV
        assert_eq!(OutputFormat::from_opt(None), OutputFormat::Csv);
        assert_eq!(OutputFormat::from_opt(Some("bogus")), OutputFormat::Csv);
    }

    #[test]
    fn output_format_ext_and_label() {
        assert_eq!(OutputFormat::Csv.ext(), "csv");
        assert_eq!(OutputFormat::Parquet.ext(), "parquet");
        assert_eq!(OutputFormat::Csv.label(), "UTF-8 CSV");
        assert_eq!(OutputFormat::Parquet.label(), "Parquet");
    }

    #[test]
    fn compute_output_path_keeps_relative_structure_and_sets_ext() {
        let in_root = Path::new("/data/in");
        let out_root = Path::new("/data/out");
        let src = Path::new("/data/in/sub/a.txt");

        let pq = compute_output_path(src, in_root, out_root, "parquet").unwrap();
        assert_eq!(pq, PathBuf::from("/data/out/sub/a.parquet"));

        // 输入扩展名与输出扩展名无关 (txt → csv)
        let csv = compute_output_path(src, in_root, out_root, "csv").unwrap();
        assert_eq!(csv, PathBuf::from("/data/out/sub/a.csv"));
    }

    #[test]
    fn compute_output_path_rejects_path_outside_input_root() {
        let res = compute_output_path(
            Path::new("/elsewhere/a.csv"),
            Path::new("/data/in"),
            Path::new("/data/out"),
            "csv",
        );
        assert!(res.is_err());
    }
}