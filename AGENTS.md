# AI 协作强制规范

> 本文件是本仓库对**所有 AI 编码工具**的唯一规范来源（single source of truth）。
>
> 仓库里各工具的适配文件（`CLAUDE.md`、`.github/copilot-instructions.md`、`.continue/rules/`、
> `.junie/`、`.amazonq/rules/`、`.rules`、`.gemini/settings.json`、`.aider.conf.yml`）
> **只做两件事**：复述下方「§2 红线」、指回本文件。
> **规范正文只在 `AGENTS.md` 维护**，不要在适配文件里另写一套。
>
> Cursor / Windsurf / Cline 已原生读本文件，**不再保留适配层**。
> 各入口为什么必须在根目录、哪些已删除，见 **§7「根目录布局」**。

---

## §0 效力与冲突

- 本文件中的 **必须 / 禁止** 是硬约束，不是建议。违反即视为该改动不可接受。
- 优先级：**用户当次明确指令 > 本文件 > 工具默认行为 > 你的个人偏好**。
  用户指令与本文件冲突时，先指出冲突点并请求确认，**不要默默选一边**。
- 本文件与代码不一致时：**以代码为准**，并在同一次改动中修正本文件（见 §7）。
- 不确定就停下来问。**一次提问的成本，远低于一次错误的"顺手重构"。**

---

## §1 项目事实（先读，别猜）

| 项 | 值 |
|---|---|
| 形态 | Tauri 2 桌面应用，目标平台 Windows x64 |
| 后端 | Rust 2021，MSRV **1.90**，源码 `src-tauri/src/` |
| 前端 | 原生 HTML / CSS / JS，位于 `dist/`，**没有构建步骤**，`withGlobalTauri: true` |
| 状态管理 | 无数据库。DuckDB 内嵌，只读用户文件 |
| 测试 | `src-tauri/tests/*.rs`（集成）+ 各模块 `#[cfg(test)]`（单元） |
| 打包 | `cargo tauri build` → MSI + NSIS，产物在 `src-tauri/target/release/bundle/` |
| 版本号 | `src-tauri/Cargo.toml` 与 `src-tauri/tauri.conf.json` **两处必须同步** |
| 提交身份 | 项目级 `qianxkun <qianxkun@163.com>`（见 §3.1） |

### 模块职责（改代码前先定位到模块）

| 文件 | 职责 | 改动风险 |
|---|---|---|
| `lib.rs` | Tauri 命令、文件扫描、转换编排、事件推送 | 中 |
| `detector.rs` | 编码探测；`DetectedEncoding` 唯一枚举 | **高** |
| `converter.rs` | 单文件转码；`encoding_of()` 唯一映射；`ConvertOptions` | **高** |
| `preview.rs` | 读文件头部、嗅探分隔符、解析表头与样例行 | **高** |
| `sql.rs` | DuckDB 连接、扩展预加载、SQL 执行、结果导出、Parquet 写入 | **高** |

### 对外命令（`lib.rs`）

`scan_files` · `preview_files` · `start_conversion` · `cancel_conversion` · `run_sql` · `export_sql_result`

新增命令必须同时：① 写进 `generate_handler!`；② 前端 `invoke` 的参数名用 **camelCase**（Rust 侧 snake_case 会自动转换）。

---

## §2 红线（MUST NOT）

违反任一条即视为改动不通过，即使功能"看起来能用"。

| # | 禁止 | 原因（真实事故） |
|---|---|---|
| **R1** | 用全局 Git 身份提交 | 全局身份是 `麻书博 <mashubo@norinaviation.com>`，曾污染 `init` 提交，需 force-push 才修掉 |
| **R2** | 提交 `.workbuddy-ai/`、`target/`、`testdata/`、`test_bigfile.csv`、`utf8_*` 缓存 | 已被 `.gitignore` 忽略；`utf8_*` 是运行时转码缓存，与数据文件同目录 |
| **R3** | 用 `git add` 直接添加 `.duckdb_extension`；删除或替换现有扩展二进制 | 走 Git LFS；单文件曾达 168 MB 触发 GitHub 100 MB 上限（GH001） |
| **R4** | 在运行时代码里用 `env!("CARGO_MANIFEST_DIR")` 拼路径 | 编译期常量，会把构建机绝对路径写死进 exe，装到别人机器上静默失效 |
| **R5** | 新增或恢复 `encodings` 扩展 | 177 MB（原载荷 48%），本项目管线只让 DuckDB 看 UTF-8，它从未被使用 |
| **R6** | 用 `Decoder::decode_to_string(..., last = false)` 解码文件内容 | 会提前停止消费输入，静默丢掉文件后半段（表头正常、后续行缺字） |
| **R7** | 在 `detector` / `converter` 之外另写一份编码映射；让预览与转换使用不同编码来源 | 会产出"预览对了、产物乱码" |
| **R8** | 引入前端构建链（npm / vite / webpack / TypeScript） | 当前零依赖零构建，改造成本高；除非用户明确要求 |
| **R9** | 把 `bundle.windows.wix.language` 改回 `en-US` | `productName` 含中文，en-US 对应代码页 1252 → `light.exe` 报 `LGHT0311` |
| **R10** | 未实际验证就声称"已完成 / 已修复" | 必须给出真实执行的命令与输出，见 §6 |
| **R11** | force-push、`git reset --hard`、`git lfs migrate`、改写已推送历史 | 破坏性操作；**必须用户明确授权**才可执行 |
| **R12** | 把探针文件留在仓库（`zz_probe_*`、临时测试、`dbg!` / `console.log` 调试输出） | 曾用 `zz_probe_*` 做能力实测，用完必须删除 |

> 上述红线中可自动检查的部分已脚本化：`bash scripts/check-conventions.sh`
> （覆盖 R1–R6、R9、R12 及版本同步、前后端编码 id 一致性）。**提交前必须跑，退出码非 0 不得提交。**

---

## §3 分域规则

### §3.1 Git 与提交

**必须**

- 提交前确认身份为项目级：

  ```bash
  git config user.name   # 期望: qianxkun
  git config user.email  # 期望: qianxkun@163.com
  ```

  若不符，写项目级配置（**不要**用 `--global`）：

  ```bash
  git config user.name  qianxkun
  git config user.email qianxkun@163.com
  ```

- 提交信息用 `type(scope): 摘要`，摘要用中文，祈使句，不超过 50 字：

  ```
  feat(preview): 支持手动指定文件编码并端到端生效
  fix(sql): 扩展目录改用 resource_dir 解析, 移除编译期路径
  chore(deps): 移除未使用的 encodings 扩展 (196MB -> 19MB)
  ```

  `type` 取值：`feat` / `fix` / `refactor` / `perf` / `test` / `docs` / `build` / `chore`。
  **禁止**用 `init`、`update`、`修改`、`fix bug` 这类无信息量的信息。

- 一个提交只做一件事。功能改动与其测试放同一提交；纯格式化单独一个提交。

- 涉及二进制（`*.duckdb_extension`）的提交，先确认 `.gitattributes` 的 LFS 规则生效：

  ```bash
  git check-attr filter src-tauri/tools/duckdb-extensions/v1.5.6/windows_amd64/json.duckdb_extension
  # 期望: filter: lfs
  ```

**禁止**

- `git commit --amend` 修作者（默认保留原作者，`GIT_AUTHOR_NAME` 环境变量无效）。
  确需修正用 `git commit --amend --author="qianxkun <qianxkun@163.com>"`，且需用户授权。

---

### §3.2 Rust 后端

**必须**

- 改动后跑通 `cargo check --all-targets`（**含测试目标**，`--lib` 不够）。
- 错误用 `anyhow::Result` 向上传播，命令层再转 `String`；**不要** `unwrap()` / `expect()`
  处理用户输入或文件 IO（测试代码除外）。
- 注释用中文，解释**为什么**而不是**做了什么**。项目里已有的 `// 关键: ...` 风格请沿用。
- 并行用 `rayon::par_iter`，单个文件失败要折叠进该文件的 `error` 字段，**不能中断整批**。
- 长耗时操作必须放在 `tauri::async_runtime::spawn_blocking` 里，不要阻塞 async 运行时。
- 文件写入用「临时文件 + 原子 `rename`」，不要直接覆盖目标文件。

**禁止**

- 引入新的重量级依赖（尤其是另一个数据库 / 另一个编码库）。先说明理由并征得同意。
- 把 `panic = "abort"` 改掉（release profile 已设，影响崩溃行为）。
- 在 `lib.rs` 里堆积业务逻辑。新逻辑优先落到对应模块。

---

### §3.3 编码管线（最高风险区，改动前必须读完本节）

数据流是**单向**的，任何一环绕开都会产出错误文件：

```
文件 ──► detect() ──► DetectedEncoding ──► encoding_of() ──► &'static Encoding
                            │
                            └─► id() ──► 前端下拉 ──► forced_encoding ─┐
                                                                       ├─► preview（读头部 64KB）
                                                                       └─► convert（整文件流式）
```

**不变量（invariants）**

| # | 规则 |
|---|---|
| **I1** | `DetectedEncoding` 是唯一编码枚举。不要在别处再定义编码列表 |
| **I2** | `converter::encoding_of()` 是 `DetectedEncoding → &'static Encoding` 的**唯一**映射 |
| **I3** | `DetectedEncoding::id()` / `from_id()` 是编码标识的**唯一**定义。前端 `ENCODING_CHOICES`（`dist/main.js`）的 id 必须与之逐字一致 |
| **I4** | `forced_encoding` 必须**同时**传给 `preview_files` 与 `start_conversion`。只传一边 = 预览与产物不一致 |
| **I5** | 解码文件头部一律用一次性 `enc.decode(&buf)`；截断时手工 pop 末尾的 `\u{FFFD}` |
| **I6** | **不把编码问题推给 DuckDB**。管线永远先把文件转成 UTF-8，DuckDB 只看到 UTF-8 |

**I6 的理由（实测，不要重新论证）**

DuckDB 的 `read_csv(..., encoding='X')` 用 **ICU converter 名**，不是常见别名：

- `gb18030`、`UTF-16`（带 BOM）可用；
- `gbk` / `cp936` / `utf-16le` 直接报 "does not support"；
- `big5` **被接受但静默解错**（`\r\n` 解成 `◙`，整文件塌成一列，**不报错**）；
- `windows-950-2000` 中文正确但 ASCII 列名乱码。

因此「直读」不能作为主路径。`encoding=` 还是整条扫描共用的单一参数，glob 场景下无法逐文件适配。

**新增编码时**

1. 在 `DetectedEncoding` 加变体；
2. 同步 `name()`（显示名）、`id()`、`from_id()`（机器标识）、`encoding_of()`（映射）；
3. 若探测器需要识别它，改 `detector.rs` 的判定逻辑并补测试；
4. 更新 `dist/main.js` 的 `ENCODING_CHOICES`；
5. 更新 `README.md` 的编码支持列表。

漏掉任一步都会造成静默不一致（前端传了一个后端不认的 id，覆盖被忽略且不报错）。

**预览行数陷阱**

`PREVIEW_ROWS = 5`，但**实际渲染通常是表头 + 4 行**：`parse_preview` 多解析 1 条给表头，
而只读头部 64 KB 时缓冲区几乎必然切在记录中间，截断保护又会 `pop()` 掉半条记录。
净效果 `5 + 1 - 1 = 5` 条 → 表头 1 行 + 数据 4 行。文档写行数时**以实际渲染为准**，
不要照抄常量。

---

### §3.4 前端（`dist/`，无构建步骤）

**必须**

- 只改 `dist/index.html`、`dist/main.js`、`dist/style.css` 三个文件。
- 用 `window.__TAURI__.core.invoke(...)`（`withGlobalTauri` 已开），不要引入 `@tauri-apps/api`。
- 用 `textContent` 写用户数据，**不要** `innerHTML` 拼接文件内容（CSV 里可能有任意字符）。
- 改完跑语法检查：

  ```bash
  node --check dist/main.js
  ```

- 涉及 UI 的改动，**必须重新 `cargo tauri build`** 才能进安装包 —— 前端资源是编译期内嵌的。
  仅 `dist/` 改动不会自动反映到已构建的 exe。

**CSS 约定（两条踩过的坑）**

1. **flex 子项 + `overflow: hidden` 会取消自动最小尺寸保护**（`min-height: auto` 解析为 0），
   配合默认的 `flex-shrink: 1` 会静默裁剪内容。列表项一律加 `flex-shrink: 0`。
2. **`position: sticky` + `border-collapse: collapse`** 下，`border` 画的分隔线不会跟随吸顶，
   滚动时表头和数据会糊在一起。用 `box-shadow: inset 0 -1px 0 <color>` 代替。

**禁止**

- 引入任何 CDN 脚本、外部字体、图标库（应用需离线可用）。
- 把 `csp` 从 `null` 改成策略值而不做完整验证（会静默阻断内联样式/脚本）。

---

### §3.5 DuckDB 与扩展

**版本锁（改动前必读）**

`sql.rs` 的 `EXT_VERSION = "v1.5.6"` / `EXT_PLATFORM = "windows_amd64"` 必须与
`duckdb-rs` 捆绑的 DuckDB API 版本严格一致（见 `duckdb.h` 的 `DUCKDB_API_VERSION_*`）。
不一致时 `LOAD` 会因版本校验失败，且**只会在运行时暴露**。

升级 `duckdb` crate 时的完整步骤：

1. 改 `src-tauri/Cargo.toml` 的 `duckdb` 版本；
2. 确认新的 API 版本号；
3. 跑 `cargo test --test install_all` 重新下载对应版本的扩展（**需联网**）；
4. 从 `%USERPROFILE%\.duckdb\extensions\<ver>\windows_amd64\` 复制到
   `src-tauri/tools/duckdb-extensions/<ver>/windows_amd64/`；
5. 改 `EXT_VERSION`；
6. 跑 `cargo test --test offline_extensions` 确认 7 个扩展离线可加载。

**当前扩展集（7 个，约 196 MB）**

`json` · `parquet` · `icu` · `httpfs` · `fts` · `excel` · `spatial`

- `encodings` **已移除**，不要加回来（R5）。
- 每个扩展的实际用途要能说清；新增前先证明「不加就无法实现」。

**扩展目录解析（不要退回编译期路径，R4）**

```rust
// 正确：run() 的 setup 里注入一次
sql::set_ext_dir(sql::resolve_ext_dir(candidates));  // candidates 首选 resource_dir()
```

- 候选顺序：`app.path().resource_dir()/duckdb-extensions` → 开发期回退 `CARGO_MANIFEST_DIR`。
- `tauri.conf.json` 的 `bundle.resources` 必须包含 `tools/duckdb-extensions/`，
  否则安装包里根本没有扩展文件（安装包会从 57 MB 掉回 8 MB，这是最快的自查信号）。
- 预加载失败会**静默降级为联网 `INSTALL`**。任何改动后都要检查预加载报告，不要假设它成功。

**已知性能事实**

每次查询固定开销约 **67 ms**（新建连接 + 7 次 `LOAD`）。这是**已知且被接受**的取舍：
每查询一个新连接，可以让 duckdb-rs 的「`prepare()` 语法错误永久毒化连接」bug 天然隔离。
**不要**擅自改成常驻连接 —— 那需要配套的毒化检测、`SELECT 1` 探活、父进程 kill 与重启逻辑。

**Parquet 依赖扩展**

`COPY ... (FORMAT PARQUET)` **需要 parquet 扩展**。此前"不需要"的结论是错的，
那次测试之所以通过，是因为 `autoload_known_extensions` 自动加载了本机已 `INSTALL` 的扩展。
关掉自动加载后报 `Catalog Error: Copy Function with name "parquet" is not in the catalog`。
因此「编码转换」面板的 Parquet 输出**同样依赖扩展目录**。

---

### §3.6 打包与发布

**必须**

- 打包命令：`cd src-tauri && cargo tauri build`（或仓库根目录 `cargo tauri build`）。
- 打包后**核对产物大小**，这是资源是否真的进去的最快信号：

  | 产物 | 正常区间 | 异常含义 |
  |---|---|---|
  | NSIS `*_x64-setup.exe` | ~57 MiB | 掉到 ~8 MiB = 扩展没打进去 |
  | MSI `*_x64_zh-CN.msi` | ~80 MiB | 同上 |

- 发布前给出 **SHA256 + 字节数**，不要只给"约 xx MB"。
- 首次全量编译约 9 分钟；仅改配置约 3 分钟；带 196 MB resources 打包额外数分钟。
  用后台任务跑，不要在前台死等。

**禁止**

- 手工编辑 `src-tauri/target/release/wix/**` 下的生成文件（每次构建会覆盖）。
- 声称"离线可用"而不验证扩展目录在**安装后的路径**下存在。

---

## §4 已知陷阱速查（症状 → 原因 → 做法）

| 症状 | 真实原因 | 正确做法 |
|---|---|---|
| 别人装上后 SQL 全失败，离线不成立 | `env!("CARGO_MANIFEST_DIR")` 是编译期常量 | `set_ext_dir` + `resource_dir()`（R4） |
| `light.exe` 报 `LGHT0311`，代码页 1252 | `productName` 中文 + WiX `en-US` | `wix.language = "zh-CN"`（R9） |
| 「全部展开」表头/数据被裁掉 | flex 子项 `overflow:hidden` 使 `min-height:auto` 归零 | `flex-shrink: 0` + 内层 `overflow:auto` |
| 吸顶表头的分隔线滚动后消失 | `border-collapse: collapse` 下 border 不跟随 | `box-shadow: inset 0 -1px 0` |
| 表头正常但后续行缺字 | `decode_to_string(..., last=false)` 提前停止消费 | 一次性 `Encoding::decode()`（R6） |
| 预览正确、产物乱码 | `forced_encoding` 只传给了 preview | 两端同一来源（I4） |
| 单元测试全过，真实文件仍解析错 | 单测用手写字节，没覆盖流式解码 | 集成测试用**真实 GBK 文件** |
| 预览行数比常量少 1 | 多解析 1 条给表头 + 截断 pop 半条 | 文档写实际渲染结果 |
| `read_csv(encoding='big5')` 不报错但塌成一列 | ICU 名不匹配、静默错解 | 一律先转码（I6） |
| `cargo tauri build` 报 `os error 5` | Windows 文件锁瞬时占用 | 确认无残留 `rustc*` 临时目录后**重试一次** |
| 改了 `dist/` 但安装包没变 | 前端资源编译期内嵌 | 重新 `cargo tauri build` |
| `git lfs migrate import` 卡住不动 | 交互式询问 `override changes?` | `yes \| git lfs migrate import ...` |
| LFS 迁移后工作区只剩 133 字节文件 | 只迁了索引，未检出实体 | `git lfs checkout` |
| `bash: $'\r': command not found`，脚本无法执行 | Windows `core.autocrlf=true` 把 `.sh` checkout 成 CRLF | `.gitattributes` 固定 `*.sh text eol=lf` |
| `curl` 报 `CRYPT_E_NO_REVOCATION_CHECK` | Windows schannel 吊销检查 | 加 `--ssl-no-revoke` |

---

## §5 变更流程（固定五步，不要跳）

1. **读** —— 定位到具体模块，读现有实现与相邻注释。**禁止**凭猜测改代码。
2. **说** —— 一句话说明根因或改动点。若涉及 §2 红线或架构选择，**先问再做**。
3. **改** —— 最小改动。不要顺手重构无关代码、不要顺手改格式。
4. **验** —— 执行 §6 中与改动相关的全部命令，**保留真实输出**。
5. **报** —— 说明：改了哪些文件、为什么、验证结果、遗留风险。
   产物类改动要给绝对路径与 SHA256。

**汇报时禁止**

- 把"应该可以了"写成"已完成"。
- 隐藏失败的命令输出。
- 声称验证过 UI —— 除非你真的看到了截图。UI 行为应明确标注"需用户确认"。

---

## §6 提交前自查清单

```bash
# 0. 身份（必须是项目级）+ 红线自检
git config user.name && git config user.email
bash scripts/check-conventions.sh

# 1. 编译（含测试目标）
cd src-tauri && cargo check --all-targets

# 2. 单元测试
cargo test --lib

# 3. 与改动相关的集成测试（按需选）
cargo test --test preview_files
cargo test --test forced_encoding
cargo test --test offline_extensions
cargo test --test sql_e2e
cargo test --test parquet_query

# 4. 前端语法
cd .. && node --check dist/main.js

# 5. 工作区检查：确认没有探针文件、缓存、大文件
git status --short
git diff --stat
```

**打包相关改动额外执行**

```bash
cargo tauri build
sha256sum src-tauri/target/release/bundle/nsis/*.exe \
          src-tauri/target/release/bundle/msi/*.msi
stat -c '%s  %n' src-tauri/target/release/bundle/nsis/*.exe \
                 src-tauri/target/release/bundle/msi/*.msi
```

**结果判定**

- `cargo check` 有 `warning` 也要处理；新增 warning 视为未完成。
- 任何测试失败 → **不得**标记完成，也不得用 `#[ignore]` 掩盖（除非该测试本身已被废弃，
  且要说明原因）。

---

## §7 本文件的维护

**必须**在本文件里更新的场景：

- 新增 / 删除编码类型 → §3.3 的「新增编码时」步骤与 §1 模块表
- 新增 / 删除 Tauri 命令 → §1 命令清单
- 升级 DuckDB 或调整扩展集 → §3.5
- 修掉一个"会重复踩"的 bug → §4 陷阱表**追加一行**（这是本文件最有价值的部分）
- 改变打包方式 / 产物大小量级 → §3.6
- 新增一条可自动检查的红线 → 同步写进 `scripts/check-conventions.sh`（否则它只是标语）

**不要**把以下内容写进本文件：一次性的排查过程、临时路径、某个具体 commit 的细节、
工具版本号（除非构成硬约束）。

修改本文件后，若「§2 红线」条目有增减，需同步更新各工具适配文件中的红线摘要表。

### 根目录布局（为什么有的能挪、有的不能）

各 AI 工具对入口路径的处理方式不同：**能配置的已统一指向 `AGENTS.md`，硬编码的只能留在根目录**。
不要为了「整洁」擅自移动下表标 ❌ 的文件 —— 移走后工具会**静默读不到规范且不报错**，
这是最难发现的一类失效。

| 位置 | 工具 | 可移动性 |
|---|---|---|
| `AGENTS.md` | 跨工具事实标准（Codex 等） | ❌ 根目录硬编码 |
| `CLAUDE.md` | Claude Code | ❌ 根目录硬编码（内容 = `@AGENTS.md` + 红线摘要） |
| `.rules` | Zed | ❌ 根目录硬编码 |
| `.aider.conf.yml` | Aider | ❌ 根目录硬编码（但内部 `read:` 可指向任意路径） |
| `.github/copilot-instructions.md` | GitHub Copilot | ❌ 由 GitHub 约定 |
| `.gemini/settings.json` | Gemini CLI | ✅ 用 `context.fileName` 指向 `AGENTS.md` |
| `.continue/rules/`、`.junie/`、`.amazonq/rules/` | 各自工具 | ❌ 目录名硬编码（已在点目录内，不占根目录） |

> Cursor / Windsurf / Cline **不在此表**：它们原生读根目录本文件，适配层已删除，见下。

**已合并删除，不要重新添加**：

- `GEMINI.md` → 改由 `.gemini/settings.json` 的 `context.fileName: ["AGENTS.md"]` 指定。
  注意键名是 `context.fileName`（`contextFileName` 已从官方文档移除），写错会导致规范静默不加载。
- `CONVENTIONS.md` → `.aider.conf.yml` 的 `read:` 直接指向 `AGENTS.md`。
- `.cursorrules` → Cursor 已弃用该文件。

**已删除整个目录（工具原生读本文件，2026-10 核实官方文档）**：

| 目录 | 依据 | 前提 |
|---|---|---|
| `.cursor/rules/` | Cursor 文档把 `AGENTS.md` 列为规则类型：「simple alternative to `.cursor/rules`」，支持项目根目录 | Cursor 较新版本 |
| `.windsurf/rules/` | Windsurf/Devin 文档：根目录 `AGENTS.md` **always-on**，`.windsurf/rules` 仅为 legacy fallback | — |
| `.clinerules/` | Cline v3.37.0 起支持 `AGENTS.md` | Cline ≥ 3.37.0 |

**刻意保留**（不是漏删）：`.gemini/` 是让 Gemini 认 `AGENTS.md` 的指针，删掉会退回读已不存在的
`GEMINI.md`；`.continue/` 与 `.amazonq/` 官方文档未提供 `AGENTS.md` 支持；`.junie/` 未查证，
不做「可能静默失效」的删除。

删除理由不是「根目录难看」，而是这些都是 `AGENTS.md` §2 红线的**复制**：
复制必然漂移（`.cursorrules` 的「改完跑」清单就漏了 `scripts/check-conventions.sh`）。
**同一份规则只允许存在一处正文。**
