#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""parity_check.py — Rust `exam-cli dump` vs Python `dump_banks.py` 的**题目级** parity 门禁。

为什么重写(P0-2)
----------------
旧版 `dump_parity.sh` 用 `diff` 的**行数**除以 `wc -l` 的**行数**当差异率:
    DIFF_LINES / max(wc -l py, wc -l rust)
两个 dump 都是单行 JSON(normalize 阶段 `json.dump` 不带 indent),文件没有换行符
→ `wc -l` = 0 → 分母 0 → 差异率恒为 0.00% → 门禁永远 ✅。
换句话说:**Rust 端哪怕少导出一个题库,这个门禁也发现不了**。

本脚本改成题目级比较:
  * 分母 = 参与比较的题目总数(每个 bank 取两侧题目数的较大值,再求和)
  * 分子 = 字段不一致 / 单侧缺失的题目数
  * 判定 = 差异率 ≤ threshold(默认 1%)

用法:
    parity_check.py <py_dump.json> <rust_dump.json> [--threshold 1.0]
    parity_check.py --self-test      # 门禁自测:证明门禁**能变红**(H1 门禁自举)

退出码:0 = 通过;1 = 差异率超阈值;2 = 用法/IO 错误
"""
from __future__ import annotations

import argparse
import copy
import json
import sys
from typing import Any, Dict, List

# Rust 端 `QuestionType::Unknown` 的字符串形式;语义上等于「没有类型」
UNKNOWN_TYPE = "unknown"


def _as_list(value: Any) -> List[Any]:
    """标量 → 单元素列表(与 Python 端 kb_db.py:628 的契约一致:
    `at if isinstance(at, list) else ([at] if at else [])`)。"""
    if value is None or value == "":
        return []
    if isinstance(value, list):
        return list(value)
    return [value]


def _norm_options(options: Any) -> List[Dict[str, Any]]:
    out = []
    for opt in _as_list(options):
        if not isinstance(opt, dict):
            out.append({"key": None, "text": str(opt)})
            continue
        out.append({"key": opt.get("key"), "text": opt.get("text")})
    return out


def _norm_question(q: Dict[str, Any], bank_key: str, index: int) -> Dict[str, Any]:
    """把一题归一化成可比较的 dict(丢掉「空」字段 —— 两边对「没有」的表达不同)。"""
    answer_text = _as_list(q.get("answerText"))
    if isinstance(q.get("answerText"), str):
        answer_text_text = q["answerText"]
    else:
        answer_text_text = "、".join(str(x) for x in answer_text)

    norm = {
        "id": q.get("id") or f"{bank_key}-{index}",
        "number": q.get("number"),
        "year": q.get("year"),
        "type": q.get("type"),
        "typeLabel": q.get("typeLabel"),
        "question": q.get("question"),
        "nq": q.get("nq"),
        "nqstem": q.get("nqstem"),
        "options": _norm_options(q.get("options")),
        "answerKeys": [str(x) for x in _as_list(q.get("answerKeys"))],
        "answerText": answer_text_text,
        "analysis": q.get("analysis"),
        "topic": q.get("topic"),
        "srcBank": q.get("srcBank"),
    }
    # 「空」字段两边表达不一致(None / "" / [] / "unknown"),统一丢掉后再比
    empty = {k for k, v in norm.items() if v in (None, "", [])}
    empty.add("type") if norm.get("type") in (None, "", UNKNOWN_TYPE) else None
    for k in empty:
        norm.pop(k, None)
    return norm


def _bank_key(name: str) -> str:
    """Python 端用文件名(含 .json),Rust 端用文件 stem(不含 .json)→ 统一去掉后缀。"""
    return name[:-5] if name.endswith(".json") else name


def _collect(dump: Dict[str, Any]) -> Dict[str, Dict[str, List[Dict[str, Any]]]]:
    """{source: {bank_key: [归一化后的题目]}}"""
    out: Dict[str, Dict[str, List[Dict[str, Any]]]] = {}
    for source, banks in dump.items():
        if source == "_meta" or not isinstance(banks, dict):
            continue
        src_out: Dict[str, List[Dict[str, Any]]] = {}
        for bank, val in banks.items():
            if not isinstance(val, dict):
                continue
            key = _bank_key(bank)
            questions = val.get("questions") or []
            src_out[key] = [
                _norm_question(q if isinstance(q, dict) else {}, key, i)
                for i, q in enumerate(questions)
            ]
        out[source] = src_out
    return out


def compare(py: Dict[str, Any], rust: Dict[str, Any]) -> Dict[str, Any]:
    """题目级比较 → 返回统计 + 差异样本。"""
    py_c = _collect(py)
    ru_c = _collect(rust)

    sources = sorted(set(py_c) | set(ru_c))
    total = 0
    mismatched = 0
    samples: List[str] = []
    banks_only_one_side: List[str] = []

    for source in sources:
        py_banks = py_c.get(source, {})
        ru_banks = ru_c.get(source, {})
        for bank in sorted(set(py_banks) | set(ru_banks)):
            py_qs = py_banks.get(bank)
            ru_qs = ru_banks.get(bank)
            if py_qs is None or ru_qs is None:
                missing_from = "python" if ru_qs is not None else "rust"
                banks_only_one_side.append(f"{source}/{bank}(缺于 {missing_from})")
            py_qs = py_qs or []
            ru_qs = ru_qs or []
            n = max(len(py_qs), len(ru_qs))
            total += n
            for i in range(n):
                a = py_qs[i] if i < len(py_qs) else None
                b = ru_qs[i] if i < len(ru_qs) else None
                if a is None or b is None:
                    mismatched += 1
                    if len(samples) < 20:
                        samples.append(
                            f"{source}/{bank}[{i}]: 单侧缺失 "
                            f"(python={'有' if a else '无'}, rust={'有' if b else '无'})"
                        )
                    continue
                if a != b:
                    mismatched += 1
                    if len(samples) < 20:
                        diff_keys = sorted(
                            k for k in set(a) | set(b) if a.get(k) != b.get(k)
                        )
                        samples.append(
                            f"{source}/{bank}[{i}](id={a.get('id')}): 字段不同 → {diff_keys}"
                        )

    pct = (mismatched * 100.0 / total) if total else 0.0
    return {
        "total": total,
        "mismatched": mismatched,
        "pct": pct,
        "samples": samples,
        "banks_only_one_side": banks_only_one_side,
    }


def _make_synthetic(n_banks: int = 2, n_q: int = 50) -> Dict[str, Any]:
    banks = {}
    for b in range(n_banks):
        questions = []
        for i in range(n_q):
            questions.append(
                {
                    "id": f"b{b}-{i}",
                    "question": f"题干 {b}-{i}",
                    "nq": f"题干{b}-{i}",
                    "nqstem": f"题干{b}-{i}",
                    "options": [{"key": "A", "text": "选项"}],
                    "answerKeys": ["A"],
                    "answerText": "选项",
                }
            )
        banks[f"bank{b}"] = {"meta": {}, "questions": questions}
    return {"json_banks": banks, "_meta": {"total_questions": n_banks * n_q}}


def _mutate(dump: Dict[str, Any], fraction: float) -> Dict[str, Any]:
    """把 fraction 比例的题目改坏(改 answerText + 删掉一部分)。"""
    out = copy.deepcopy(dump)
    banks = out["json_banks"]
    all_q = [q for bank in banks.values() for q in bank["questions"]]
    for q in all_q[: max(1, int(len(all_q) * fraction))]:
        q["answerText"] = "改坏了"
    return out


def gate_passes(res: Dict[str, Any], threshold: float) -> bool:
    """门禁判定:分母为 0 一律判红(否则又是一道永远绿的空门禁)。"""
    if res["total"] == 0:
        return False
    return res["pct"] <= threshold


def self_test() -> int:
    """门禁自测:同一个比较 + 判定函数,必须能区分「一致」(绿)与「有差异」(红)。

    这是 H1(门禁自举)的落地:没验过能红的门禁,只是看起来在工作。
    """
    base = _make_synthetic()
    same = compare(base, copy.deepcopy(base))
    assert same["total"] == 100, same
    assert same["mismatched"] == 0, same
    assert same["pct"] == 0.0, same
    assert gate_passes(same, 1.0), "一致样本应判绿"
    print(f"[self-test] 一致样本: {same['mismatched']}/{same['total']} → 判绿 ✓")

    for fraction in (0.005, 0.02, 0.5):
        bad = compare(base, _mutate(base, fraction))
        assert bad["total"] == 100, bad
        assert bad["mismatched"] > 0, f"变异 {fraction} 却没检出差异: {bad}"
        print(
            f"[self-test] 变异 {fraction:.1%}: {bad['mismatched']}/{bad['total']} "
            f"= {bad['pct']:.2f}% → 检测到差异 ✓"
        )

    # 判定层:超过阈值必须判红(这是「能让它红」的核心断言)
    for fraction in (0.02, 0.5):
        bad = compare(base, _mutate(base, fraction))
        assert not gate_passes(bad, 1.0), f"变异 {fraction} 超过阈值却判绿: {bad}"
        print(f"[self-test] 变异 {fraction:.1%} → 判定为红 ✓")

    # 分母必须是题目数,不是「文件行数」—— 用一个空 dump 验证分母不会退化成 0
    empty = {"json_banks": {}, "_meta": {}}
    res = compare(base, empty)
    assert res["total"] == 100, f"单侧缺失时分母应为题目数: {res}"
    assert res["mismatched"] == 100, res
    assert res["pct"] == 100.0, res
    assert not gate_passes(res, 1.0), "单侧全缺必须判红"
    print(f"[self-test] 单侧全缺: {res['mismatched']}/{res['total']} = {res['pct']:.2f}% → 判红 ✓")

    # 两边都空 → 分母 0 → 判红(空门禁自暴)
    both_empty = compare(empty, empty)
    assert not gate_passes(both_empty, 1.0), "空 dump 必须判红"
    print("[self-test] 双侧皆空:分母 0 → 判红 ✓")
    print("[self-test] OK — 门禁能区分一致(绿) / 有差异(红)")
    return 0


def main(argv: List[str]) -> int:
    ap = argparse.ArgumentParser(description="题目级 dump parity 门禁")
    ap.add_argument("py_dump", nargs="?", help="Python 侧 dump JSON")
    ap.add_argument("rust_dump", nargs="?", help="Rust 侧 dump JSON")
    ap.add_argument("--threshold", type=float, default=1.0, help="允许的差异率 %% (默认 1)")
    ap.add_argument("--self-test", action="store_true", help="门禁自测(证明能变红)")
    args = ap.parse_args(argv)

    if args.self_test:
        return self_test()

    if not args.py_dump or not args.rust_dump:
        ap.error("需要 <py_dump.json> <rust_dump.json>(或使用 --self-test)")

    try:
        with open(args.py_dump, encoding="utf-8") as f:
            py = json.load(f)
        with open(args.rust_dump, encoding="utf-8") as f:
            rust = json.load(f)
    except OSError as e:
        print(f"读取 dump 失败: {e}", file=sys.stderr)
        return 2
    except json.JSONDecodeError as e:
        print(f"dump 不是合法 JSON: {e}", file=sys.stderr)
        return 2

    res = compare(py, rust)
    print(
        f"题目级 parity: {res['mismatched']} / {res['total']} 题不一致 "
        f"({res['pct']:.2f}%,阈值 {args.threshold:.2f}%)"
    )
    for note in res["banks_only_one_side"]:
        print(f"  ! 单侧独有 bank: {note}")
    for s in res["samples"]:
        print(f"  - {s}")

    if res["total"] == 0:
        print("❌ 分母为 0 —— 没有可比较的题目,门禁无法判定(按失败处理)", file=sys.stderr)
        return 1

    if res["pct"] <= args.threshold:
        print("✅ parity OK")
        return 0
    print(f"❌ parity FAIL(>{args.threshold:.2f}%)", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
