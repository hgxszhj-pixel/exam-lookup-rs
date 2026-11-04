#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""dump_banks.py — 给 dump_parity.sh 用的 Python 基线。

读取 banks/*.json + 跳过 answers_*.json / selftest / *.json.bak,
输出与 Rust `exam-cli dump` 同结构的 JSON,方便 diff。
"""
from __future__ import annotations
import json, re, sys, pathlib, datetime

_PUNC = re.compile(
    r"[\s，。、；：！？（）《》【】<>,.;:!?()\[\]\"'’‘“”\-—_/\\|]+"
)
def norm(s: str) -> str:
    return _PUNC.sub("", s or "")

def stem_of(s: str) -> str:
    """对齐 Python `solve.py::stem_of`(Rust `parse::stem_of` 对齐的也是它)。

    原实现是另写的一套(剥前导 A-F / 剥前导括号数字 / 剥前导题号),
    与 solve.py 无关 —— parity 比的是同一套契约,两边必须用同一个定义。
    """
    t = re.split(r"[（(]\s*[)）]\s*", s or "")[0]
    if norm(t) == norm(s):
        t = re.split(r"\s*A[.、．]\s*", s or "")[0]
    return t.strip()

def as_list(v):
    """标量 → 单元素列表。

    与 Python 端 kb_db.py:628 的契约一致:
        at = q.get("answerText")
        items = at if isinstance(at, list) else ([at] if at else [])
    真实数据里 answerText 确实存在标量形态(如 "30分钟"),不做这层归一化
    会被 join 成 "3、0、分、钟" —— parity 假差异。
    """
    if v is None or v == "":
        return []
    return list(v) if isinstance(v, list) else [v]

def main() -> int:
    if len(sys.argv) < 3:
        print("usage: dump_banks.py <banks-root> <out>", file=sys.stderr)
        return 2
    root = pathlib.Path(sys.argv[1])
    out = pathlib.Path(sys.argv[2])

    out_obj = {}
    total = 0
    for path in sorted(root.glob("*.json")):
        name = path.name
        if name.startswith("answers_"):
            continue
        if name.endswith(".bak"):
            continue
        if "selftest" in name:
            continue
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except Exception:
            continue
        meta = data.get("meta") or {}
        questions = []
        for q in data.get("questions", []):
            stem = q.get("question", "")
            questions.append({
                "id": q.get("id") or f"{path.stem}-{len(questions)}",
                "number": q.get("number"),
                "year": q.get("year"),
                "type": q.get("type"),
                "typeLabel": q.get("typeLabel"),
                "question": stem,
                "nq": norm(stem),
                "nqstem": norm(stem_of(stem)),
                "options": q.get("options") or [],
                "answerKeys": as_list(q.get("answerKeys")),
                "answerText": "、".join(str(x) for x in as_list(q.get("answerText"))),
                "analysis": q.get("analysis"),
                "topic": q.get("topic"),
                "srcBank": q.get("srcBank"),
            })
        out_obj.setdefault("json_banks", {})[path.name] = {
            "meta": meta,
            "questions": questions,
        }
        total += len(questions)

    out_obj["_meta"] = {
        "total_questions": total,
        "schema": "v1",
        "generated": datetime.datetime.now().isoformat(timespec="seconds"),
    }
    out.write_text(json.dumps(out_obj, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"dump_banks: wrote {total} questions to {out}", file=sys.stderr)
    return 0

if __name__ == "__main__":
    sys.exit(main())
