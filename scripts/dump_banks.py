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
    s = re.sub(r"^[A-F][\.\)、:：]\s*", "", s or "")
    s = re.sub(r"^[（(]\s*\d+\s*[)）]\s*", "", s)
    s = re.sub(r"^\d+[\.\)、:：]\s*", "", s)
    return s.strip()

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
                "answerKeys": q.get("answerKeys") or [],
                "answerText": "、".join(q.get("answerText") or []),
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
