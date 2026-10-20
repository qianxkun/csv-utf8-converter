---
trigger: always_on
description: CSV UTF-8 转换器 项目强制工作规范
---

# 项目强制工作规范

完整规范见仓库根目录 **`AGENTS.md`**（「§2 红线」为硬约束）。本文件是常驻摘要。

## 项目事实

- Tauri 2 + Rust 2021（MSRV 1.90）。后端 `src-tauri/src/`，前端 `dist/`（原生 HTML/CSS/JS，**无构建步骤**）。
- 前端调用：`window.__TAURI__.core.invoke('cmd', { camelCaseArgs })`。
- 注释用中文，解释「为什么」。

## 红线

1. Git 身份必须项目级 `qianxkun <qianxkun@163.com>`，禁止全局身份。
2. 禁止提交 `.workbuddy-ai/`、`target/`、`testdata/`、`utf8_*`。
3. `*.duckdb_extension` 走 Git LFS；禁止直接 `git add`，禁止删除现有扩展。
4. 禁止运行时代码用 `env!("CARGO_MANIFEST_DIR")` 拼路径。
5. 禁止新增/恢复 `encodings` 扩展。
6. 禁止 `Decoder::decode_to_string(..., last = false)`（静默截断）。
7. 编码映射只能来自 `converter::encoding_of()`；`forced_encoding` 必须同时传给预览与转换。
8. 禁止引入前端构建链（npm / vite / webpack / TS）。
9. 禁止把 `wix.language` 改回 `en-US`（中文 productName → `LGHT0311`）。
10. 未验证不得声称完成。
11. force-push / `reset --hard` 需用户授权。
12. 探针文件（`zz_probe_*`）用完必须删除。

## 编码管线

`detector.rs` → `DetectedEncoding` → `converter::encoding_of()` → `&'static Encoding`。
`id()` / `from_id()` 是唯一机器标识，前端 `ENCODING_CHOICES` 必须逐字一致。
一律先转 UTF-8 再交给 DuckDB；`read_csv(encoding='big5')` 会静默解错且不报错。

## 必须执行

```bash
bash scripts/check-conventions.sh
cd src-tauri && cargo check --all-targets
cargo test --lib
cargo test --test preview_files --test forced_encoding
cd .. && node --check dist/main.js
```

改了 `dist/` 或 `tauri.conf.json` → 重新 `cargo tauri build`。
