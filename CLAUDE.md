@AGENTS.md

# Claude Code 补充说明

上面导入的 `AGENTS.md` 是本仓库的**唯一规范来源**，其中的「§2 红线」是硬约束。
本文件不重复规范正文，只强调最容易出错的三点：

## 动手前

1. 先读 `AGENTS.md` §1（模块职责）定位改动位置，**不要**凭猜测改代码。
2. 涉及 `detector.rs` / `converter.rs` / `preview.rs` 的改动，必须读完 `AGENTS.md` §3.3。
3. 涉及 `sql.rs` 或 `tauri.conf.json` 的改动，必须读完 §3.5 / §3.6。

## 高频红线（违反即不可接受）

- **R1** 提交身份必须是项目级 `qianxkun <qianxkun@163.com>`，不是全局的 `麻书博`。
- **R4** 禁止在运行时代码里用 `env!("CARGO_MANIFEST_DIR")` 拼路径（编译期常量，会写死构建机路径）。
- **R6** 禁止 `Decoder::decode_to_string(..., last = false)`（静默截断文件）。
- **R7** 编码映射只能来自 `converter::encoding_of()`；`forced_encoding` 必须同时传给预览和转换。
- **R10** 未实际执行验证命令，不得声称"已完成"。
- **R12** 探针测试（`zz_probe_*`）用完必须删除。

## 改完必跑

```bash
bash scripts/check-conventions.sh
cd src-tauri && cargo check --all-targets
cargo test --lib
cargo test --test preview_files --test forced_encoding
cd .. && node --check dist/main.js
git status --short
```

涉及 UI 或打包的改动，前端资源是编译期内嵌的，必须重新 `cargo tauri build` 才会生效。
