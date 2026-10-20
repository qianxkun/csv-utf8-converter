# GitHub Copilot 工作规范

本仓库的完整规范见根目录 **`AGENTS.md`**，其中的「§2 红线」是硬约束。
本文件只列最容易出错的部分，生成代码时请遵守。

## 项目要点

- Tauri 2 + Rust 2021（MSRV 1.90），后端 `src-tauri/src/`，前端 `dist/`（原生 HTML/CSS/JS，**无构建步骤**）。
- 前端调用后端用 `window.__TAURI__.core.invoke('命令名', { camelCase 参数 })`。
- 注释用中文，解释「为什么」。

## 硬约束

| # | 禁止 |
|---|---|
| R1 | 用全局 Git 身份提交；必须 `qianxkun <qianxkun@163.com>` |
| R2 | 提交 `.workbuddy-ai/`、`target/`、`testdata/`、`utf8_*` 缓存 |
| R3 | 直接用 `git add` 添加 `*.duckdb_extension`（走 Git LFS），或删除现有扩展二进制 |
| R4 | 运行时代码用 `env!("CARGO_MANIFEST_DIR")` 拼路径 |
| R5 | 新增/恢复 `encodings` 扩展（177 MB 且从未被使用） |
| R6 | 用 `Decoder::decode_to_string(..., last = false)` 解码文件内容（静默截断） |
| R7 | 在 `detector` / `converter` 之外另写编码映射；让预览与转换用不同编码来源 |
| R8 | 引入 npm / vite / webpack / TypeScript 等前端构建链 |
| R9 | 把 `bundle.windows.wix.language` 改回 `en-US`（会触发 `LGHT0311`） |
| R12 | 留下 `zz_probe_*` 等探针文件或调试输出 |

## 编码管线不变量

```
detect() → DetectedEncoding → encoding_of() → &'static Encoding
                  └→ id() → 前端下拉 → forced_encoding → 预览 & 转换
```

- `DetectedEncoding` 是唯一编码枚举；`encoding_of()` 是唯一映射；
  `id()` / `from_id()` 是唯一机器标识，前端 `ENCODING_CHOICES` 必须逐字一致。
- `forced_encoding` 必须同时传给 `preview_files` 和 `start_conversion`，否则会出现
  「预览对了、产物乱码」。
- 不要把编码问题推给 DuckDB：一律先转 UTF-8。`read_csv(encoding='big5')` 会**静默解错**。

## 生成代码时的约定

- 文件 IO 与用户输入**不要** `unwrap()` / `expect()`，用 `anyhow::Result` 传播。
- 批量处理用 `rayon::par_iter`，单文件失败折叠进该文件的 `error` 字段，不中断整批。
- 写文件用「临时文件 + 原子 rename」。
- 前端写用户数据用 `textContent`，不要 `innerHTML` 拼接。
- 不要新增重量级依赖；需要时先说明理由。

## 已知陷阱

- flex 子项加 `overflow: hidden` 会让 `min-height: auto` 解析为 0，配合 `flex-shrink: 1` 静默裁剪内容
  → 列表项加 `flex-shrink: 0`。
- `position: sticky` + `border-collapse: collapse` 下 `border` 不跟随吸顶
  → 用 `box-shadow: inset 0 -1px 0` 画分隔线。
- 改了 `dist/` 必须重新 `cargo tauri build`，前端资源是编译期内嵌的。

## 改完必跑

```bash
bash scripts/check-conventions.sh
cd src-tauri && cargo check --all-targets
cargo test --lib
cargo test --test preview_files --test forced_encoding
cd .. && node --check dist/main.js
git status --short
```

完整内容见 `AGENTS.md`。
