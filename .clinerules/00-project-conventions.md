# 项目强制工作规范（Cline / Roo Code）

完整规范见仓库根目录 **`AGENTS.md`**（「§2 红线」为硬约束）。本文件是摘要。

## 项目事实

- Tauri 2 + Rust 2021（MSRV 1.90）。后端 `src-tauri/src/`，前端 `dist/`（原生 HTML/CSS/JS，**无构建步骤**）。
- 前端调用：`window.__TAURI__.core.invoke('cmd', { camelCaseArgs })`。
- 注释用中文，解释「为什么」。

## 禁止

1. 用全局 Git 身份提交 —— 必须项目级 `qianxkun <qianxkun@163.com>`。
2. 提交 `.workbuddy-ai/`、`target/`、`testdata/`、`utf8_*` 缓存。
3. 直接 `git add` 添加 `*.duckdb_extension`（走 Git LFS）；删除现有扩展二进制。
4. 运行时代码用 `env!("CARGO_MANIFEST_DIR")` 拼路径（编译期常量 → 写死构建机路径）。
5. 新增/恢复 `encodings` 扩展。
6. 用 `Decoder::decode_to_string(..., last = false)` 解码文件（静默截断后半段）。
7. 在 `detector` / `converter` 之外另写编码映射；让预览与转换用不同编码来源。
8. 引入 npm / vite / webpack / TypeScript 构建链。
9. 把 `bundle.windows.wix.language` 改回 `en-US`。
10. 未实际验证就声称完成。
11. force-push / `reset --hard` / 改写已推送历史（需授权）。
12. 留下 `zz_probe_*` 探针文件或 `dbg!` / `console.log` 调试输出。

## 必须

- 编码一律先转 UTF-8 再交给 DuckDB。不要用 `read_csv(encoding=...)` 直读：
  ICU 名不匹配（`gbk` 直接报错），`big5` **静默解错**（`\r\n` 解成 `◙`，整文件塌成一列）。
- `forced_encoding` 同时传给 `preview_files` 与 `start_conversion`。
- 文件 IO / 用户输入不用 `unwrap()` / `expect()`，用 `anyhow::Result`。
- 批量处理用 `rayon::par_iter`，单文件失败折叠进 `error` 字段。
- 写文件用「临时文件 + 原子 rename」。
- 前端用户数据用 `textContent`，不用 `innerHTML`。
- 改了 `dist/` 必须重新 `cargo tauri build`（前端资源编译期内嵌）。
- 改完跑：`bash scripts/check-conventions.sh`、`cargo check --all-targets`、`cargo test --lib`、`node --check dist/main.js`。
