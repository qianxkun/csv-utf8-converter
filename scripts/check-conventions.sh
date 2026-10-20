#!/usr/bin/env bash
#
# 仓库红线自检 —— 把 AGENTS.md §2 的硬约束变成可执行检查。
#
# 用法:
#   bash scripts/check-conventions.sh          # 全量检查
#   bash scripts/check-conventions.sh --quiet  # 只输出失败项
#
# 退出码: 0 全部通过 / 1 存在违规
#
# 这不是完整测试（测试见 §6 清单），只拦截「文档里写了但没人执行」的红线。

set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

QUIET=0
[ "${1:-}" = "--quiet" ] && QUIET=1

FAIL=0
ok()   { [ "$QUIET" -eq 1 ] || printf '  \033[32mPASS\033[0m  %s\n' "$1"; }
no()   { printf '  \033[31mFAIL\033[0m  %s\n' "$1"; FAIL=1; }
warn() { [ "$QUIET" -eq 1 ] || printf '  \033[33mWARN\033[0m  %s\n' "$1"; }
head_() { [ "$QUIET" -eq 1 ] || printf '\n\033[1m%s\033[0m\n' "$1"; }

# ---------------------------------------------------------------- R1 提交身份
head_ "R1  Git 身份"
NAME=$(git config user.name 2>/dev/null || true)
EMAIL=$(git config user.email 2>/dev/null || true)
if [ "$EMAIL" = "mashubo@norinaviation.com" ] || [ "$NAME" = "麻书博" ]; then
  no "当前是全局身份 ${NAME} <${EMAIL}>，必须改为项目级 qianxkun <qianxkun@163.com>"
elif [ "$NAME" = "qianxkun" ] && [ "$EMAIL" = "qianxkun@163.com" ]; then
  ok "项目级身份 ${NAME} <${EMAIL}>"
else
  warn "身份为 ${NAME} <${EMAIL}>（非仓库约定的 qianxkun，协作者可忽略）"
fi

# ---------------------------------------------------------------- R2 禁提交路径
head_ "R2  禁提交路径"
STAGED=$(git diff --cached --name-only 2>/dev/null || true)
if [ -z "$STAGED" ]; then
  ok "暂存区为空"
else
  BAD=$(printf '%s\n' "$STAGED" | grep -E '^(\.workbuddy-ai/|src-tauri/target/|target/|testdata/|test_bigfile\.csv$)|(^|/)utf8_[^/]*$' || true)
  if [ -n "$BAD" ]; then
    no "暂存了不该提交的路径："
    printf '        %s\n' $BAD
  else
    ok "暂存区无缓存/产物/测试数据"
  fi
fi

# ---------------------------------------------------------------- R3 LFS
head_ "R3  DuckDB 扩展走 LFS"
LFS_BAD=""
while IFS= read -r f; do
  [ -z "$f" ] && continue
  attr=$(git check-attr filter -- "$f" 2>/dev/null | awk -F': ' '{print $NF}')
  [ "$attr" = "lfs" ] || LFS_BAD="$LFS_BAD $f"
done <<< "$(printf '%s\n' "$STAGED" | grep -E '\.duckdb_extension$' || true)"
if [ -n "$LFS_BAD" ]; then
  no "以下扩展未被 LFS 接管（会触发 GitHub 100MB 限制）：$LFS_BAD"
else
  ok "扩展文件 LFS 规则正常（或无扩展变更）"
fi

# ---------------------------------------------------------------- R5 encodings
head_ "R5  未恢复 encodings 扩展"
if awk '/const PRELOAD_EXTENSIONS/,/^\];/' src-tauri/src/sql.rs | grep -q '"encodings"'; then
  no "PRELOAD_EXTENSIONS 里出现了 encodings（177MB 且从未使用，见 AGENTS.md R5）"
else
  ok "PRELOAD_EXTENSIONS 不含 encodings"
fi

# ---------------------------------------------------------------- R6 流式解码
head_ "R6  未使用流式 decode_to_string"
HITS=$(grep -rn 'decode_to_string' src-tauri/src/ 2>/dev/null | grep -v '///' | grep -v '^\S*:[0-9]*: *//' || true)
if [ -n "$HITS" ]; then
  no "发现 decode_to_string 调用（会提前停止消费输入，静默截断）："
  printf '        %s\n' "$HITS"
else
  ok "仅注释中提及，无实际调用"
fi

# ---------------------------------------------------------------- R4 扩展目录
head_ "R4  扩展目录未退回编译期路径"
if grep -q 'pub fn set_ext_dir' src-tauri/src/sql.rs; then
  ok "sql.rs 提供 set_ext_dir()"
else
  no "sql.rs 缺少 set_ext_dir()"
fi
if grep -q 'sql::set_ext_dir' src-tauri/src/lib.rs; then
  ok "lib.rs 在启动流程中注入扩展目录"
else
  no "lib.rs 未调用 sql::set_ext_dir（安装后必然找不到扩展）"
fi
if awk '/fn preload_extensions/,/^}/' src-tauri/src/sql.rs | grep -q 'env!'; then
  no "preload_extensions 内出现 env!（必须经 ext_dir() 解析）"
else
  ok "preload_extensions 不依赖编译期路径"
fi

# ---------------------------------------------------------------- R9 WiX 语言
head_ "R9  WiX 语言与资源"
if grep -q '"language": *"zh-CN"' src-tauri/tauri.conf.json; then
  ok "wix.language = zh-CN"
else
  no "wix.language 不是 zh-CN（中文 productName 会触发 LGHT0311）"
fi
if grep -q '"tools/duckdb-extensions/"' src-tauri/tauri.conf.json; then
  ok "bundle.resources 含 duckdb-extensions"
else
  no "bundle.resources 缺少 tools/duckdb-extensions/（安装包会缺扩展）"
fi

# ---------------------------------------------------------------- 版本同步
head_ "版本号同步"
V_CARGO=$(grep -m1 '^version' src-tauri/Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
V_TAURI=$(grep -m1 '"version"' src-tauri/tauri.conf.json | sed 's/.*"\(.*\)".*/\1/')
if [ "$V_CARGO" = "$V_TAURI" ]; then
  ok "Cargo.toml 与 tauri.conf.json 一致（$V_CARGO）"
else
  no "版本不一致：Cargo.toml=$V_CARGO / tauri.conf.json=$V_TAURI"
fi

# ---------------------------------------------------------------- I3 编码 id
head_ "I3  前后端编码 id 一致"
BE=$(awk '/pub fn id\(&self\)/,/^    }$/' src-tauri/src/detector.rs | grep -o '"[a-z0-9_]*"' | tr -d '"' | sort -u)
FE=$(awk '/const ENCODING_CHOICES = \[/,/^\];/' dist/main.js | grep -o "'[a-z0-9_]*'" | tr -d "'" | sort -u)
if [ -z "$BE" ] || [ -z "$FE" ]; then
  no "无法提取编码 id（源码结构可能已变，请更新本脚本）"
else
  MISSING=$(comm -13 <(printf '%s\n' "$BE") <(printf '%s\n' "$FE") || true)
  if [ -n "$MISSING" ]; then
    no "前端存在后端不认的 id（会被静默忽略）：$(echo $MISSING)"
  else
    ok "前端 $(echo "$FE" | wc -l) 个 id 均为后端已知"
  fi
fi

# ---------------------------------------------------------------- R12 探针
head_ "R12 无探针文件 / 调试输出"
PROBES=$(ls src-tauri/tests/zz_probe_* 2>/dev/null || true)
if [ -n "$PROBES" ]; then
  no "残留探针测试：$(echo $PROBES)"
else
  ok "无 zz_probe_* 残留"
fi
DBG=$(grep -rn 'dbg!(' src-tauri/src/ 2>/dev/null || true)
if [ -n "$DBG" ]; then
  no "源码中有 dbg!()："
  printf '        %s\n' "$DBG"
else
  ok "无 dbg!()"
fi

# ---------------------------------------------------------------- 汇总
echo
if [ "$FAIL" -eq 0 ]; then
  printf '\033[32m全部通过\033[0m\n'
else
  printf '\033[31m存在违规，见上方 FAIL 项（规范见 AGENTS.md §2）\033[0m\n'
fi
exit "$FAIL"
