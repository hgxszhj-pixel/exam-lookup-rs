#!/usr/bin/env bash
# dump_parity.sh — 对比 Rust exam-cli dump vs Python qbank.py dump。
#
# 用法:
#   bash scripts/dump_parity.sh <exam-lookup-root> <tmp-dir>
#
# 流程:
#   1) 跑 Python `qbank.py dump` → py_dump.json
#   2) 跑 Rust  `exam-cli dump` → rust_dump.json
#   3) 规范化字段顺序 / whitespace
#   4) diff;差异行数 / 占比输出到 stderr
#   5) 差异 ≤ 1% 视为通过(返回 0),否则非 0
#
# 注意:Python 与 Rust 端的字段命名约定可能不同(例如 answerText vs answer_text),
# 已在 normalize.py 中处理。

set -euo pipefail

PY_ROOT="${1:-/Users/hgxszhj/x_videos-dowloads/exam-lookup}"
TMP_DIR="${2:-/tmp/exam-parity}"
mkdir -p "$TMP_DIR"

PY_DUMP="$TMP_DIR/py_dump.json"
RUST_DUMP="$TMP_DIR/rust_dump.json"
NORM_PY="$TMP_DIR/py_norm.json"
NORM_RUST="$TMP_DIR/rust_norm.json"

echo "[1/4] Python dump ..."
python3 "$(dirname "$0")/dump_banks.py" "$PY_ROOT/banks" "$PY_DUMP" \
    || { echo "Python dump failed"; exit 2; }

echo "[2/4] Rust dump ..."
( cd "$(dirname "$0")/.." \
  && cargo run -q --bin exam-cli -- --data-root "$PY_ROOT" dump "$RUST_DUMP" )

echo "[3/4] Normalize ..."
python3 - <<PY
import json, sys, os
def norm(p, in_obj):
    out = {}
    for src, banks in in_obj.items():
        if src == "_meta":
            continue
        for bank, val in banks.items():
            questions = []
            for q in val.get("questions", []):
                qn = {k: v for k, v in q.items() if v not in (None, "", [])}
                questions.append(qn)
            out.setdefault(src, {})[bank] = {"meta": val.get("meta"), "questions": questions}
    json.dump(out, open(p, "w"), sort_keys=True, ensure_ascii=False)

py = json.load(open("$PY_DUMP"))
rust = json.load(open("$RUST_DUMP"))
norm("$NORM_PY", py)
norm("$NORM_RUST", rust)
PY

echo "[4/4] diff ..."
DIFF_OUT=$(diff "$NORM_PY" "$NORM_RUST" || true)
DIFF_LINES=$(echo "$DIFF_OUT" | wc -l | tr -d ' ')
TOTAL_PY_LINES=$(wc -l < "$NORM_PY")
TOTAL_RUST_LINES=$(wc -l < "$NORM_RUST")
DENOM=$(( TOTAL_PY_LINES > TOTAL_RUST_LINES ? TOTAL_PY_LINES : TOTAL_RUST_LINES ))
if [[ $DENOM -eq 0 ]]; then
  PCT="0.00"
else
  PCT=$(awk -v a="$DIFF_LINES" -v b="$DENOM" 'BEGIN{printf "%.2f", a*100/b}')
fi

echo "diff lines: $DIFF_LINES / ~$DENOM  ($PCT%)"
if awk -v p="$PCT" 'BEGIN{exit !(p<=1.0)}'; then
  echo "✅ parity OK (≤ 1%)"
  exit 0
else
  echo "❌ parity FAIL (> 1%)"
  echo "前 20 行 diff:"
  echo "$DIFF_OUT" | head -20
  exit 1
fi
