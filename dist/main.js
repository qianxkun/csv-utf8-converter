// Tauri 2.x 全局 API (需 tauri.conf.json 的 withGlobalTauri=true)
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

const state = {
  inputDir: '',
  outputDir: '',
  // 文件预览: previews = 后端返回的逐文件信息; selected = 勾选的文件绝对路径
  previews: [],
  selected: new Set(),
  previewLoaded: false,
  sqlDir: '',
  // 转码缓存目录; 空串 = 默认 (数据文件同目录), 非空 = 用户指定的统一缓存位置
  sqlCacheDir: '',
  isRunning: false,
  total: 0,
  startedAt: 0,
  successCount: 0,
  failedCount: 0,
  skippedCount: 0,
  sqlRunning: false,
  // 最近一次成功执行的 SQL (导出时重跑它, 而不是导出被截断的预览数据)
  lastSql: '',
  sqlExporting: false,
};

// ============== Tab 切换 ==============

function switchTab(name) {
  document.querySelectorAll('.tab').forEach(t => t.classList.toggle('active', t.dataset.tab === name));
  document.querySelectorAll('.tab-panel').forEach(p => {
    const isActive = p.dataset.tab === name;
    p.classList.toggle('active', isActive);
    p.hidden = !isActive;
  });
}

document.querySelectorAll('.tab').forEach(t => {
  t.addEventListener('click', () => switchTab(t.dataset.tab));
});

// ============== 文件夹选择 (转换) ==============

async function pickInput() {
  const result = await invoke('plugin:dialog|open', {
    options: { directory: true, multiple: false, title: '选择输入目录' },
  });
  if (result) {
    state.inputDir = result;
    $('input-dir').value = result;
    // 换了输入目录, 旧预览全部作废
    clearPreviews();
    updateButtons();
  }
}

async function pickOutput() {
  const result = await invoke('plugin:dialog|open', {
    options: { directory: true, multiple: false, title: '选择输出目录' },
  });
  if (result) {
    state.outputDir = result;
    $('output-dir').value = result;
    updateButtons();
  }
}

// ============== 文件预览 (原始编码 + 表头 + 样例行) ==============

/** 每个文件预览的数据行数 */
const PREVIEW_ROWS = 5;

/**
 * 读取输入目录下每个文件的原始编码、表头与前 5 行数据。
 * 用户据此判断哪些文件需要转码, 勾选后再转换。
 */
async function loadPreviews() {
  if (!state.inputDir) return;

  $('preview').textContent = '正在读取文件预览...';
  $('scan-btn').disabled = true;
  $('preview-toolbar').hidden = true;
  $('preview-list').hidden = true;

  try {
    const r = await invoke('preview_files', {
      inputDir: state.inputDir,
      recursive: $('recursive').checked,
      sampleRows: PREVIEW_ROWS,
    });
    state.previews = r.files || [];
    state.previewLoaded = true;
    // 默认只勾选「需要转码」的文件 —— 已是 UTF-8 的不必重复处理
    state.selected = new Set(
      state.previews.filter(f => f.needsTranscode).map(f => f.path)
    );
    renderPreviewList();
  } catch (e) {
    state.previews = [];
    state.previewLoaded = false;
    state.selected = new Set();
    $('preview').textContent = `预览失败: ${e}`;
  } finally {
    updateButtons();
  }
}

/** 输入目录 / 递归开关变化后, 旧预览作废 */
function clearPreviews() {
  state.previews = [];
  state.selected = new Set();
  state.previewLoaded = false;
  $('preview-list').innerHTML = '';
  $('preview-list').hidden = true;
  $('preview-toolbar').hidden = true;
  refreshPreviewSummary();
  updateButtons();
}

/** 顶部摘要: 未预览时给提示, 预览后给出统计与已勾选数 */
function refreshPreviewSummary() {
  const el = $('preview');
  if (!state.previewLoaded) {
    el.textContent = `将扫描 .csv / .txt, 输出为 ${formatLabel()}`;
    return;
  }
  const totalBytes = state.previews.reduce((sum, f) => sum + f.size, 0);
  const need = state.previews.filter(f => f.needsTranscode).length;
  el.textContent =
    `匹配 ${state.previews.length} 个文件 · 需转码 ${need} 个 · ` +
    `共 ${formatBytes(totalBytes)} · 已勾选 ${state.selected.size} 个`;
}

const DELIM_LABELS = { ',': ', (逗号)', '\t': '\\t (制表符)', ';': '; (分号)', '|': '| (竖线)' };

function delimLabel(d) {
  return DELIM_LABELS[d] || d || '?';
}

function renderPreviewList() {
  const list = $('preview-list');
  list.innerHTML = '';

  if (!state.previews.length) {
    list.hidden = true;
    $('preview-toolbar').hidden = true;
    refreshPreviewSummary();
    return;
  }

  const frag = document.createDocumentFragment();
  for (const f of state.previews) frag.appendChild(buildPreviewItem(f));
  list.appendChild(frag);

  list.hidden = false;
  $('preview-toolbar').hidden = false;
  refreshPreviewSummary();
}

/** 单元格文本包一层 span, 才能可靠地做省略号截断 (td/th 自身不支持 max-width) */
function cellEl(tag, text) {
  const el = document.createElement(tag);
  const span = document.createElement('span');
  span.className = 'pv-cell';
  span.textContent = text;
  el.appendChild(span);
  return el;
}

/** 展开区: 元信息 + 表头与样例行的小表格 */
function buildPreviewDetail(detail, f) {
  const meta = document.createElement('div');
  meta.className = 'pv-meta';

  if (f.error) {
    meta.textContent = `读取失败: ${f.error}`;
    detail.appendChild(meta);
    return;
  }
  if (!f.headers.length) {
    meta.textContent = '空文件, 或没有可解析的表头';
    detail.appendChild(meta);
    return;
  }

  const parts = [
    `分隔符: ${delimLabel(f.delimiter)}`,
    `表头 ${f.headers.length} 列`,
    `预览 ${f.rows.length} 行`,
  ];
  if (f.columnsTruncated) parts.push(`仅显示前 ${f.headers.length} 列`);
  meta.textContent = parts.join(' · ');
  detail.appendChild(meta);

  const table = document.createElement('table');
  table.className = 'pv-table';

  const trh = document.createElement('tr');
  for (const h of f.headers) trh.appendChild(cellEl('th', h));
  const thead = document.createElement('thead');
  thead.appendChild(trh);
  table.appendChild(thead);

  const tbody = document.createElement('tbody');
  for (const row of f.rows) {
    const tr = document.createElement('tr');
    for (let i = 0; i < f.headers.length; i++) {
      tr.appendChild(cellEl('td', row[i] ?? ''));
    }
    tbody.appendChild(tr);
  }
  table.appendChild(tbody);

  const wrap = document.createElement('div');
  wrap.className = 'pv-table-wrap';
  wrap.appendChild(table);
  detail.appendChild(wrap);
}

/** 单个文件条目: 勾选框 + 文件名 + 大小 + 原始编码 + 状态, 展开后是表头与样例行 */
function buildPreviewItem(f) {
  const item = document.createElement('div');
  item.className = 'pv-item';

  const head = document.createElement('div');
  head.className = 'pv-head';

  const cb = document.createElement('input');
  cb.type = 'checkbox';
  cb.checked = state.selected.has(f.path);
  cb.dataset.path = f.path;
  cb.title = f.path;
  cb.addEventListener('change', () => {
    if (cb.checked) state.selected.add(f.path);
    else state.selected.delete(f.path);
    refreshPreviewSummary();
    updateButtons();
  });
  head.appendChild(cb);

  const name = document.createElement('span');
  name.className = 'pv-name';
  name.textContent = f.relativePath;
  name.title = f.path;
  head.appendChild(name);

  const size = document.createElement('span');
  size.className = 'pv-size';
  size.textContent = formatBytes(f.size);
  head.appendChild(size);

  const enc = document.createElement('span');
  enc.className = 'pv-enc';
  enc.textContent = f.encoding;
  head.appendChild(enc);

  const tag = document.createElement('span');
  tag.className = 'pv-tag ' + (f.error ? 'err' : (f.needsTranscode ? 'need' : 'utf8'));
  tag.textContent = f.error ? '读取失败' : (f.needsTranscode ? '需转码' : '已是 UTF-8');
  head.appendChild(tag);

  const toggle = document.createElement('button');
  toggle.className = 'pv-toggle';
  toggle.textContent = '折叠';
  head.appendChild(toggle);

  item.appendChild(head);

  const detail = document.createElement('div');
  detail.className = 'pv-detail';
  buildPreviewDetail(detail, f);
  item.appendChild(detail);

  toggle.addEventListener('click', () => {
    detail.hidden = !detail.hidden;
    toggle.textContent = detail.hidden ? '展开' : '折叠';
  });

  return item;
}

/** 批量勾选: all | none | need | utf8 */
function selectFiles(which) {
  const pickers = {
    all: () => state.previews,
    none: () => [],
    need: () => state.previews.filter(f => f.needsTranscode),
    utf8: () => state.previews.filter(f => !f.needsTranscode),
  };
  const pick = pickers[which];
  if (!pick) return;

  state.selected = new Set(pick().map(f => f.path));

  // 同步已渲染的复选框状态
  document
    .querySelectorAll('#preview-list .pv-head input[type="checkbox"]')
    .forEach(cb => { cb.checked = state.selected.has(cb.dataset.path); });

  refreshPreviewSummary();
  updateButtons();
}

/** 全部展开 / 全部折叠 */
function setAllExpanded(expanded) {
  document.querySelectorAll('#preview-list .pv-item').forEach(item => {
    const detail = item.querySelector('.pv-detail');
    const toggle = item.querySelector('.pv-toggle');
    if (detail) detail.hidden = !expanded;
    if (toggle) toggle.textContent = expanded ? '折叠' : '展开';
  });
}

// ============== 输出格式 ==============

function currentFormat() {
  return $('output-format').value; // 'csv' | 'parquet'
}

function formatLabel(fmt = currentFormat()) {
  return fmt === 'parquet' ? 'Parquet' : 'UTF-8 CSV';
}

/** 输入固定为 .csv / .txt; 输出统一为所选格式 */
function resetPreview() {
  state.total = 0;
  refreshPreviewSummary();
}

/**
 * 输出格式切换:
 * - BOM 与「跳过已是 UTF-8」仅对 CSV 输出有意义, Parquet 下禁用并清零。
 * - Parquet 输出时源文件必须先转码, 不存在"跳过"的概念。
 */
function applyFormatUI() {
  const isCsv = currentFormat() === 'csv';
  $('write-bom').disabled = !isCsv;
  $('skip-utf8').disabled = !isCsv;
  if (!isCsv) {
    $('write-bom').checked = false;
    $('skip-utf8').checked = false;
  }
  resetPreview();
}

// ============== 转换控制 ==============

function buildOptions() {
  return {
    outputFormat: currentFormat(),
    recursive: $('recursive').checked,
    jobs: parseInt($('jobs').value) || 0,
    probeSize: null,
    writeBom: $('write-bom').checked,
    skipUtf8: $('skip-utf8').checked,
    retries: 2,
    overwrite: $('overwrite').checked,
  };
}

async function startConversion() {
  if (!state.inputDir || !state.outputDir) {
    setStatus('请先选择输入和输出目录', 'warn');
    return;
  }
  if (state.inputDir === state.outputDir) {
    setStatus('输入与输出不能是同一路径', 'error');
    return;
  }
  if (!state.previewLoaded) {
    setStatus('请先点「预览文件」确认编码并勾选要转换的文件', 'warn');
    return;
  }
  if (state.selected.size === 0) {
    setStatus('请至少勾选一个要转换的文件', 'warn');
    return;
  }

  resetMetrics();
  state.isRunning = true;
  state.startedAt = Date.now();
  updateButtons();
  setStatus('正在启动...');

  try {
    await invoke('start_conversion', {
      inputDir: state.inputDir,
      outputDir: state.outputDir,
      options: buildOptions(),
      // 只转换勾选的文件
      files: [...state.selected],
    });
  } catch (e) {
    appendLog(`[错误] ${e}`, 'failed');
    setStatus(`启动失败: ${e}`, 'error');
    state.isRunning = false;
    updateButtons();
  }
}

async function cancelConversion() {
  try {
    await invoke('cancel_conversion');
    setStatus('正在取消...');
  } catch (e) {
    appendLog(`[取消失败] ${e}`, 'failed');
  }
}

// ============== UI 更新 ==============

function updateButtons() {
  const dirsReady = state.inputDir && state.outputDir && !state.isRunning;
  // 转换前必须先预览并勾选: 未预览 / 未勾选都不允许开始
  const canConvert = dirsReady && state.previewLoaded && state.selected.size > 0;
  $('convert-btn').disabled = !canConvert;
  $('convert-btn').title = !state.previewLoaded
    ? '请先点「预览文件」'
    : (state.selected.size === 0 ? '请至少勾选一个文件' : '');
  $('cancel-btn').disabled = !state.isRunning;
  $('scan-btn').disabled = !state.inputDir || state.isRunning;
  $('sql-run-btn').disabled = !state.sqlDir || state.sqlRunning;
  // 导出: 需要有成功查询过的 SQL, 且当前不忙
  const canExport = !!state.lastSql && !state.sqlRunning && !state.sqlExporting;
  $('export-csv-btn').disabled = !canExport;
  $('export-parquet-btn').disabled = !canExport;
}

function setStatus(text) {
  $('status').textContent = text;
}

function resetMetrics() {
  state.successCount = 0;
  state.failedCount = 0;
  state.skippedCount = 0;
  $('m-success').textContent = '0';
  $('m-failed').textContent = '0';
  $('m-skipped').textContent = '0';
  $('m-throughput').textContent = '--';
  $('m-elapsed').textContent = '--';
  $('progress-bar').style.width = '0%';
  $('log').innerHTML = '';
}

function updateMetric(id, value) {
  $(id).textContent = value;
}

function appendLog(text, cls = '') {
  const el = document.createElement('div');
  el.className = `entry ${cls}`;
  el.textContent = text;
  const log = $('log');
  log.appendChild(el);
  log.scrollTop = log.scrollHeight;
}

function formatBytes(b) {
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
  if (b < 1024 * 1024 * 1024) return `${(b / 1024 / 1024).toFixed(2)} MB`;
  return `${(b / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

function formatDuration(ms) {
  if (ms < 1000) return `${ms} ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)} s`;
  const m = Math.floor(s / 60);
  const rs = (s - m * 60).toFixed(0);
  return `${m}m ${rs}s`;
}

// ============== SQL 面板 ==============

const SQL_EXAMPLES = {
  basic: `SELECT *
FROM 'orders.csv'
LIMIT 20;`,
  groupby: `SELECT city, COUNT(*) AS cnt, ROUND(AVG(amount), 2) AS avg_amt
FROM 'orders.csv'
WHERE amount IS NOT NULL
GROUP BY city
ORDER BY cnt DESC
LIMIT 20;`,
  glob: `-- DuckDB 用 read_csv_auto 通配多文件, union_by_name 兼容列数差异
SELECT filename, COUNT(*) AS rows
FROM read_csv_auto('daily_*.csv', union_by_name = true)
GROUP BY filename
ORDER BY 1;`,
  multi: `SELECT a.id, a.city, a.amount, b.note
FROM 'orders.csv' a
LEFT JOIN 'notes.csv' b ON a.id = b.order_id
LIMIT 50;`,
  parquet: `-- Parquet 可直接作为表名查询 (CSV 与 Parquet 可混合 JOIN)
SELECT c.name, p.city
FROM 'orders.csv' c
JOIN 'cities.parquet' p ON c.id = p.id
LIMIT 50;`,
  parquetGlob: `-- 通配多个 Parquet 文件 (DuckDB 自动合并)
SELECT COUNT(*) AS total
FROM 'data/*.parquet';`,
};

async function pickSqlData() {
  const result = await invoke('plugin:dialog|open', {
    options: { directory: true, multiple: false, title: '选择数据目录' },
  });
  if (result) {
    state.sqlDir = result;
    $('sql-data-dir').value = result;
    updateButtons();
  }
}

function syncSqlFromOutput() {
  if (!state.outputDir) {
    appendLog(`[提示] 请先在「编码转换」面板选择输出目录`, 'info');
    return;
  }
  state.sqlDir = state.outputDir;
  $('sql-data-dir').value = state.outputDir;
  updateButtons();
}

// 选择自定义转码缓存目录 (留空/恢复默认 = 数据文件同目录)
async function pickSqlCache() {
  const result = await invoke('plugin:dialog|open', {
    options: { directory: true, multiple: false, title: '选择转码缓存目录' },
  });
  if (result) {
    state.sqlCacheDir = result;
    $('sql-cache-dir').value = result;
  }
}

// 恢复默认: 清除自定义缓存目录, 缓存回到数据文件同目录
function clearSqlCache() {
  state.sqlCacheDir = '';
  $('sql-cache-dir').value = '';
}

function setSqlStatus(text, cls = '') {
  const el = $('sql-status');
  el.textContent = text;
  el.className = 'sql-status ' + cls;
}

function renderResultTable(columns, rows, meta) {
  const table = $('sql-result-table');
  const thead = table.querySelector('thead');
  const tbody = table.querySelector('tbody');
  const empty = $('sql-result-empty');
  thead.innerHTML = '';
  tbody.innerHTML = '';

  if (!columns.length) {
    table.style.display = 'none';
    empty.style.display = 'block';
    empty.textContent = '查询无结果 (0 列)';
    return;
  }
  table.style.display = '';
  empty.style.display = 'none';

  // 表头
  const trh = document.createElement('tr');
  for (const c of columns) {
    const th = document.createElement('th');
    th.textContent = c;
    trh.appendChild(th);
  }
  thead.appendChild(trh);

  // 数据 (DOM 限制下 1000 行已经很流畅, 后端已截断)
  const frag = document.createDocumentFragment();
  for (const row of rows) {
    const tr = document.createElement('tr');
    for (let i = 0; i < columns.length; i++) {
      const td = document.createElement('td');
      const v = row[i] ?? '';
      td.textContent = v;
      // 数字右对齐
      if (/^-?\d+(\.\d+)?$/.test(v) || v === 'NULL') {
        td.classList.add('num');
      }
      tr.appendChild(td);
    }
    frag.appendChild(tr);
  }
  tbody.appendChild(frag);

  // 元信息
  const meta_el = $('sql-result-meta');
  let txt = `共 ${meta.rowCount} 行, ${columns.length} 列, 耗时 ${formatDuration(meta.elapsedMs)}`;
  if (meta.truncated) txt += ' (结果已截断)';
  if (meta.transcoded?.length) txt += ` · 本次转码: ${meta.transcoded.join(', ')}`;
  if (meta.cached?.length) txt += ` · 命中缓存: ${meta.cached.join(', ')}`;
  meta_el.textContent = txt;
}

async function runSql() {
  if (!state.sqlDir) {
    setSqlStatus('请先选择数据目录', 'warn');
    return;
  }
  const sql = $('sql-editor').value.trim();
  if (!sql) {
    setSqlStatus('SQL 不能为空', 'warn');
    return;
  }
  state.sqlRunning = true;
  updateButtons();
  setSqlStatus('执行中...', 'busy');
  const t0 = Date.now();
  try {
    const result = await invoke('run_sql', {
      dataDir: state.sqlDir,
      sql: sql,
      limit: 1000,
      cacheDir: state.sqlCacheDir || null,
    });
    // 记住这次成功执行的 SQL, 导出时重跑它以拿到完整结果
    state.lastSql = sql;
    renderResultTable(result.columns, result.rows, {
      rowCount: result.rowCount,
      elapsedMs: result.elapsedMs,
      truncated: result.truncated,
      transcoded: result.transcoded || [],
      cached: result.cached || [],
    });
    // 离线扩展状态: 有扩展未加载时提示, 避免"静默降级为联网"用户无感知
    const badExt = (result.extensions || []).filter(e => !e.loaded);
    if (badExt.length) {
      setSqlStatus(
        `完成 · ${result.rowCount} 行 · 警告: ${badExt.length} 个扩展未加载 (${badExt.map(e => e.name).join(', ')})`,
        'warn'
      );
    } else {
      setSqlStatus(`完成 · ${result.rowCount} 行 · ${formatDuration(Date.now() - t0)}`, 'ok');
    }
  } catch (e) {
    setSqlStatus(`失败: ${e}`, 'error');
    const empty = $('sql-result-empty');
    empty.textContent = `执行失败:\n${e}`;
    empty.style.display = 'block';
    $('sql-result-table').style.display = 'none';
  } finally {
    state.sqlRunning = false;
    updateButtons();
  }
}

$('sql-editor').addEventListener('keydown', (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
    e.preventDefault();
    runSql();
  }
});

// ============== 导出查询结果 ==============

/**
 * 导出当前查询结果为文件.
 * 注意: 后端会**重新执行** state.lastSql 并全量落盘,
 * 因此导出内容不受界面 1000 行截断影响.
 */
async function exportResult(format) {
  if (!state.lastSql) {
    setSqlStatus('请先执行一次查询', 'warn');
    return;
  }
  const isCsv = format === 'csv';
  const ext = isCsv ? 'csv' : 'parquet';

  // 默认文件名: query_result_<时间戳>.<ext>
  const ts = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 14);
  let outPath;
  try {
    outPath = await invoke('plugin:dialog|save', {
      options: {
        title: isCsv ? '导出为 CSV (UTF-8)' : '导出为 Parquet',
        defaultPath: `query_result_${ts}.${ext}`,
        filters: [{
          name: isCsv ? 'CSV (UTF-8)' : 'Parquet',
          extensions: isCsv ? ['csv'] : ['parquet'],
        }],
      },
    });
  } catch (e) {
    setSqlStatus(`打开保存对话框失败: ${e}`, 'error');
    return;
  }
  if (!outPath) return; // 用户取消

  state.sqlExporting = true;
  updateButtons();
  setSqlStatus(`正在导出 ${ext.toUpperCase()}...`, 'busy');
  const t0 = Date.now();
  try {
    const r = await invoke('export_sql_result', {
      dataDir: state.sqlDir,
      sql: state.lastSql,
      outPath: outPath,
      overwrite: $('export-overwrite').checked,
      // BOM 只对 CSV 有意义
      writeBom: isCsv && $('export-bom').checked,
      cacheDir: state.sqlCacheDir || null,
    });
    setSqlStatus(
      `已导出 ${r.rows} 行 (${formatBytes(r.bytes)}) · ${formatDuration(Date.now() - t0)}`,
      'ok'
    );
    $('sql-result-meta').textContent =
      `已导出 → ${r.path}  (${r.rows} 行, ${formatBytes(r.bytes)})`;
  } catch (e) {
    setSqlStatus(`导出失败: ${e}`, 'error');
  } finally {
    state.sqlExporting = false;
    updateButtons();
  }
}

document.querySelectorAll('[data-example]').forEach(btn => {
  btn.addEventListener('click', () => {
    const key = btn.dataset.example;
    if (SQL_EXAMPLES[key]) $('sql-editor').value = SQL_EXAMPLES[key];
  });
});

// ============== 事件监听 (转换) ==============

listen('conversion:start', (event) => {
  const p = event.payload;
  state.total = p.total;
  appendLog(
    `开始转换: 共 ${p.total} 个文件, 约 ${formatBytes(p.total_bytes)} → 输出 ${p.format || 'UTF-8 CSV'}`,
    'info'
  );
  setStatus(`处理中 0 / ${p.total}`);
});

listen('conversion:file-done', (event) => {
  const p = event.payload;
  const shortFile = p.file.length > 60 ? '...' + p.file.slice(-57) : p.file;
  if (p.status === 'success') state.successCount++;
  else if (p.status === 'failed') state.failedCount++;
  else if (p.status === 'skipped') state.skippedCount++;
  updateMetric('m-success', state.successCount);
  updateMetric('m-failed', state.failedCount);
  updateMetric('m-skipped', state.skippedCount);
  const pct = (p.done / p.total) * 100;
  $('progress-bar').style.width = `${pct}%`;
  if (p.status === 'success') {
    appendLog(`[OK] ${shortFile} (${p.encoding}, ${formatBytes(p.bytes_in)} -> ${formatBytes(p.bytes_out)})`, 'success');
  } else if (p.status === 'skipped') {
    appendLog(`[跳过] ${shortFile} (${p.error || '已存在'})`, 'skipped');
  } else if (p.status === 'failed') {
    appendLog(`[失败] ${shortFile}: ${p.error || ''}`, 'failed');
  }
  setStatus(`处理中 ${p.done} / ${p.total}`);
});

listen('conversion:progress', (event) => {
  const p = event.payload;
  $('m-throughput').textContent = `${p.throughput_mb_s.toFixed(2)} MB/s`;
  $('m-elapsed').textContent = formatDuration(p.elapsed_ms);
});

listen('conversion:complete', (event) => {
  const p = event.payload;
  state.isRunning = false;
  updateButtons();
  const duration = formatDuration(p.duration_ms);
  const speed = p.duration_ms > 0
    ? (p.bytes_in / 1024 / 1024 / (p.duration_ms / 1000)).toFixed(2) + ' MB/s'
    : '--';
  $('m-throughput').textContent = speed;
  $('m-elapsed').textContent = duration;
  $('progress-bar').style.width = '100%';
  if (p.cancelled) {
    setStatus(`已取消 - 成功 ${p.success}, 失败 ${p.failed}, 跳过 ${p.skipped}`);
    appendLog(`[取消] 用时 ${duration}, 速度 ${speed}`, 'info');
  } else {
    setStatus(`完成 - 成功 ${p.success}, 失败 ${p.failed}, 跳过 ${p.skipped}`);
    appendLog(`[完成] ${formatBytes(p.bytes_in)} -> ${formatBytes(p.bytes_out)}, 用时 ${duration}, 速度 ${speed}`, 'info');
  }
});

// ============== 绑定事件 ==============

$('pick-input').addEventListener('click', pickInput);
$('pick-output').addEventListener('click', pickOutput);
$('convert-btn').addEventListener('click', startConversion);
$('cancel-btn').addEventListener('click', cancelConversion);
$('scan-btn').addEventListener('click', loadPreviews);
$('sel-need').addEventListener('click', () => selectFiles('need'));
$('sel-all').addEventListener('click', () => selectFiles('all'));
$('sel-none').addEventListener('click', () => selectFiles('none'));
$('sel-expand').addEventListener('click', () => setAllExpanded(true));
$('sel-collapse').addEventListener('click', () => setAllExpanded(false));
$('clear-log').addEventListener('click', () => { $('log').innerHTML = ''; });

$('pick-sql-data').addEventListener('click', pickSqlData);
$('pick-sql-cache').addEventListener('click', pickSqlCache);
$('clear-sql-cache').addEventListener('click', clearSqlCache);
$('sql-run-btn').addEventListener('click', runSql);
$('sql-sync-output').addEventListener('click', syncSqlFromOutput);
$('export-csv-btn').addEventListener('click', () => exportResult('csv'));
$('export-parquet-btn').addEventListener('click', () => exportResult('parquet'));

$('output-format').addEventListener('change', applyFormatUI);
// 递归开关会影响扫描结果, 旧预览作废
$('recursive').addEventListener('change', clearPreviews);

setStatus('准备就绪 - 请选择输入与输出目录');
// 初始化输出格式相关 UI (禁用项 / 预览文案)
applyFormatUI();
// 初始化按钮状态 (导出按钮在查询成功前保持禁用)
updateButtons();