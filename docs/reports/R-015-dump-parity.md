# R-015: dump_parity.sh 实际生成报告(2026-10-12)

**目的**:audit round 计划 #3 — Python `qbank.py` vs Rust `exam-cli dump` 输出 parity 校验。
**脚本**:`scripts/dump_parity.sh`(已存在,本轮首次实跑)。
**数据源**:`/Users/hgxszhj/x_videos-dowloads/exam-lookup`(21 banks / 1760 Q / 1278 cards 真实数据)

## 结果

```
[1/4] Python dump ...
dump_banks: wrote 1760 questions to /tmp/exam-parity/py_dump.json
[2/4] Rust dump ...
dump 写入 /tmp/exam-parity/rust_dump.json (1760 题, 来自 2 sources)
[3/4] Normalize ...
[4/4] diff ...
diff lines: 6 / ~0  (0.00%)
✅ parity OK (≤ 1%)
```

## 唯一差异

| 维度 | Python `dump_banks.py` | Rust `exam-cli dump` |
|---|---|---|
| 来源数 | 1 source(`json_banks` 21 banks) | 2 sources(`json_banks` 21 banks + `crawl_cards` 0 questions) |
| 题数 | 1760 | 1760 |

Rust 多了一个 source key `crawl_cards: { "保密公开题-问答": {meta:null, questions:[]} }`。
Python `dump_banks.py` 只 `glob('banks/*.json')`,**不读** `crawl_cards/` 目录,所以不会输出这个 source。

## 解释

不是 parity bug,是**采样范围不同**:
- Python 基线 = banks 目录全量 JSON(单选题库)
- Rust dump = 全 source 注册表中所有 source(json_banks + crawl_cards + 任何未来新增)

`crawl_cards` 是 0 题空 source,数据迁移未完成。Rust 实现照实输出空结构,Python 工具没扫描到。

## 结论

✅ **parity 通过(差异 ≤ 1% 阈值,实际 0.00%)**。
- 1760 题 / 21 banks 完全一致
- 唯一差异是 Rust 多了 1 个空 source key(0 数据)
- 脚本判定 OK,exit 0

## 后续(可选)

1. `dump_banks.py` 改用全 source registry 扫描(对齐 Rust)— 但需要 Python 端识别 `crawl_cards` 目录,工程量 > 收益
2. Rust 端在 `crawl_cards` 数据未迁移时跳过(降低噪音)— 但需在 source 内部判断,违反 source 抽象
3. 维持现状:parity 0.00% 通过,差异源已记录

**建议**:维持现状,本 R-015 收口。
