# 项目强制工作规范

完整规范见仓库根目录 **`AGENTS.md`**（「§2 红线」为硬约束）。本文件是 Gemini CLI 的常驻摘要。

## 项目事实

- Tauri 2 + Rust 2021（MSRV 1.90）。后端 `src-tauri/src/`，前端 `dist/`（原生 HTML/CSS/JS，**无构建步骤**）。
- 前端调用：`window.__TAURI__.core.invoke('cmd', { camelCaseArgs })`。
- 注释用中文，解释「为什么」。

## 禁止

1. 用全局 Git 身份提交 —— 必须项目级 `qianxkun <qianxkun@163.com>`（全局身份是 `麻书博`，曾污染提交历史）。
2. 提交 `.workbuddy-ai/`、`target/`、`testdata/`、`test_bigfile.csv`、`utf8_*` 缓存。
3. 直接 `git add` 添加 `*.duckdb_extension`（走 Git LFS）；删除现有扩展二进制。
4. 运行时代码用 `env!("CARGO_MANIFEST_DIR")` 拼路径（编译期常量，会写死构建机绝对路径）。
5. 新增/恢复 `encodings` 扩展（177 MB，从未被使用）。
6. 用 `Decoder::decode_to_string(..., last = false)` 解码文件内容（静默截断后半段）。
7. 在 `detector` / `converter` 之外另写编码映射；让预览与转换用不同编码来源。
8. 引入 npm / vite / webpack / TypeScript 构建链。
9. 把 `bundle.windows.wix.language` 改回 `en-US`（`productName` 含中文 → `LGHT0311`）。
10. 未实际执行验证命令就声称完成。
11. force-push / `git reset --hard` / `git lfs migrate` / 改写已推送历史（需用户授权）。
12. 留下 `zz_probe_*` 探针文件或 `dbg!` / `console.log` 调试输出。

## 编码管线

- `DetectedEncoding` 是唯一编码枚举，`converter::encoding_of()` 是唯一映射，
  `id()` / `from_id()` 是唯一机器标识（前端 `ENCODING_CHOICES` 必须逐字一致）。
- `forced_encoding` 必须同时传给 `preview_files` 与 `start_conversion`。
- 一律先转 UTF-8 再交给 DuckDB。`read_csv(encoding='big5')` 会**静默解错**
  （`\r\n` 解成 `◙`，整文件塌成一列，不报错）。

## DuckDB

- `EXT_VERSION = "v1.5.6"` 必须与 `duckdb-rs` 捆绑的 API 版本一致，否则 `LOAD` 失败且只在运行时暴露。
- 扩展目录经 `sql::set_ext_dir()` 注入，来源 `app.path().resource_dir()`。
- 每次查询约 67 ms 固定开销是**已知取舍**（隔离连接毒化 bug），不要擅自改成常驻连接。

## 前端

- 只改 `dist/index.html` / `dist/main.js` / `dist/style.css`；用户数据用 `textContent`。
- flex 子项 + `overflow: hidden` → 必须加 `flex-shrink: 0`。
- `sticky` 表头 + `border-collapse: collapse` → 分隔线用 `box-shadow: inset`。
- **改了 `dist/` 必须重新 `cargo tauri build`**（前端资源编译期内嵌）。

## 改完必跑

```bash
bash scripts/check-conventions.sh
cd src-tauri && cargo check --all-targets
cargo test --lib
cargo test --test preview_files --test forced_encoding
cd .. && node --check dist/main.js
git status --short
```
