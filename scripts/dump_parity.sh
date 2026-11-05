#!/usr/bin/env bash
# dump_parity.sh — 对比 Rust exam-cli dump vs Python dump_banks.py dump。
#
# 用法:
#   bash scripts/dump_parity.sh <exam-lookup-root> <tmp-dir>
#
# 流程:
#   1) 跑 Python `dump_banks.py` → py_dump.json
#   2) 跑 Rust  `exam-cli dump` → rust_dump.json
#   3) 题目级比较(parity_check.py)→ 差异率 = 不一致题数 / 参与比较题数
#   4) 门禁自测(parity_check.py --self-test)→ 证明门禁**能变红**
#
# P0-2 修复记录:
#   旧版用 `diff` 行数 / `wc -l` 行数当差异率。两个 dump 都是单行 JSON,
#   `wc -l` = 0 → 分母 0 → 差异率恒为 0.00% → 门禁永远绿(Rust 少导出题库也发现不了)。
#   现在改成题目级比较,分母 = 参与比较的题目总数。
set -euo pipefail

PY_ROOT="${1:-/Users/hgxszhj/x_videos-dowloads/exam-lookup}"
TMP_DIR="${2:-/tmp/exam-parity}"
THRESHOLD="${PARITY_THRESHOLD:-1.0}"
mkdir -p "$TMP_DIR"

PY_DUMP="$TMP_DIR/py_dump.json"
RUST_DUMP="$TMP_DIR/rust_dump.json"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "[1/4] Python dump ..."
python3 "$SCRIPT_DIR/dump_banks.py" "$PY_ROOT/banks" "$PY_DUMP"

echo "[2/4] Rust dump ..."
( cd "$SCRIPT_DIR/.." && cargo run -q --bin exam-cli -- --data-root "$PY_ROOT" dump "$RUST_DUMP" )

echo "[3/4] 题目级比较 ..."
python3 "$SCRIPT_DIR/parity_check.py" "$PY_DUMP" "$RUST_DUMP" --threshold "$THRESHOLD"

echo "[4/4] 门禁自测(必须能变红)..."
python3 "$SCRIPT_DIR/parity_check.py" --self-test

echo "✅ parity 流程全部通过(题目级比较 + 门禁自测)"
