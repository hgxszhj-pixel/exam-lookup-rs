# Python → Rust 模块映射

> 对应 `x_videos-dowloads/exam-lookup/*.py` 的逐文件落位。"未移植"=有意裁剪,理由附列。

## 核心数据

| Python | Rust | 备注 |
|--------|------|------|
| `qbank.py` (Question/Bank 类型) | `crates/exam-engine/src/types.rs` | newtype + serde,与 Python dict 字段对齐 |
| `kb.py` (知识库核心) | `search.rs` + `index.rs` | FTS5 + bm25 替代 Python trigram |
| `search.py` (搜索入口) | `search.rs::search()` / `search_cards()` | 暴露同名公开 API |
| `make_bank.py` (造题库) | 不移植 | 离线工具,Python 仍可用 |

## 数据导入

| Python | Rust | 备注 |
|--------|------|------|
| `make_cards.py` (造问答卡) | `source/crawl_cards.rs` | Rust 版本直接消费 `crawl/问答*.json` |
| `make_drill.py` | 不移植 | 训练模式,与 desktop GUI 用例脱节 |
| `make_collection.py` | 不移植 | 收藏夹,延后 |
| `ingest_answers.py` | `source/json_banks.rs::LoadResult.answers` | `answers_*.json` 与 banks 合并加载 |
| `parse_baomi_records.py` | 不移植 | 保密题特殊版式,R-007 风险登记 |
| `parse_comm_safety.py` | 不移植 | 通信安全专项,延后 |
| `parse_docs_exam.py` | 不移植 | docs/ PDF/HTML parse,延后(R-002) |
| `parse_handbook_qa.py` | 不移植 | 手册 QA 解析,延后 |
| `parse_pub_banks.py` | `source/json_banks.rs`(同类 schema) | 公开题库走同一加载器 |
| `parse_pub_quiz.py` | 同上 | 同上 |
| `parse_textbook_quiz.py` | 同上 | 同上 |
| `sync_banks.py` | 不移植 | 离线同步脚本 |

## 检索 / 出题

| Python | Rust | 备注 |
|--------|------|------|
| `solve.py::lookup` | `search()` | bm25 排序与 Python LIKE 排序差异 → R-009 |
| `num_points.py` | 不移植 | 题号定位,desktop UI 用例脱节 |
| `cite_law.py` | 不移植 | 法规引用,延后 |
| `cue_recall_test.py` | 不移植 | 提示回忆测试,延后 |
| `batch_lookup.py` | CLI 子命令(待加) | 批量查询,Phase 后补 |

## GUI

| Python | Rust | 备注 |
|--------|------|------|
| `webui.py` (Flask) | `exam-app` (Dioxus desktop) | 单进程,跨 macOS/Win/Linux |
| `ask.html` | `ui/ask.rs` | 查题页 |
| (无对应) | `practice.rs` | 刷题,Rust 端新增 |
| (无对应) | `cards.rs` | 背题页,Phase G |
| (无对应) | `wrong.rs` | 错题本,R-006 落 `~/Library/...` |

## 工具(未移植)

| Python | 用途 | 决策 |
|--------|------|------|
| `audit_extract.py` | 审计抽取 | desktop 端用不到 |
| `brute_cms.py` | 暴破 CMS | 反爬 + 法律风险,R-007 |
| `capture_docs_pending.py` | 待抓文档 | R-007 |
| `debug_fetch.py` | 调试抓取 | R-007 |
| `extract_docs_q.py` | 文档抽题 | R-002 延后 |
| `fetch_*.py` (7 个) | 抓 URL 题库 | R-007,desktop 离线工具 |
| `full_check.py` | 全量校验 | Python 侧仍用,Rust 等价检查待加 |
| `ingest_saved_page.py` | 导入存档页 | R-007 |
| `kb_coverage.py` | 知识库覆盖度 | R-COVERAGE 已在 Rust 侧完成 |
| `kb_pages.py` | 知识库 docs 索引 | R-002 延后 |
| `ledger.py` | 台账 | desktop 用不到 |
| `oss_search.py` | OSS 搜索 | R-007 |
| `probe_*.py` (8 个) | 站点探测 | R-007 |
| `remaining_check.py` | 残留检查 | desktop 用不到 |
| `export_review.py` | 导出复习 | Phase 后补(导出按钮) |

## 决策口径

- **保留 Python 源**:`x_videos-dowloads/exam-lookup/` 是只读数据源,Rust 不动
- **离线工具不迁移**:`make_*` / `sync_*` / `parse_*` 留在 Python 侧,Rust 只消费成品 JSON
- **反爬 / 网络 IO 不迁移**:`fetch_*` / `probe_*` / `brute_*` 留 R-007,Rust desktop 离线
- **新功能 Rust 端新增**:practice / cards / wrong 三页 Rust 独占