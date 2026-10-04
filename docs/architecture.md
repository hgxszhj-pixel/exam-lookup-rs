# exam-lookup-rs 架构

> 数据流、crate 布局、模块边界。`docs/reports/` 留底的不可绕过项以 R-* 引用。

## 数据流

```
┌──────────────────────────────────────────────────────────┐
│ x_videos-dowloads/exam-lookup/        ← 只读,Python 产出 │
│   banks/*.json                                            │
│   answers_*.json                                          │
│   crawl/问答*.json                                        │
└─────────────┬────────────────────────────────────────────┘
              │ Source::load(path)
┌─────────────▼────────────────────────────────────────────┐
│ exam-engine (lib, 89 tests)                               │
│   source/json_banks.rs    inventory 注册                  │
│   source/crawl_cards.rs   问答 split                       │
│   index.rs               CREATE 5 tables,跑全量 insert     │
│   search.rs              FTS5 bm25 + LIKE fallback         │
│   scheduler.rs           6 档 Interval 调度                │
│   cache.rs               Fresh/Stale/Expired              │
└─────────────┬────────────────────────────────────────────┘
              │ 3 路接口
┌─────────────┼──────────────────┬──────────────────────────┐
│   exam-cli (clap)              │  exam-app (Dioxus 0.7.9) │
│   sources / load / tick        │  /learn /practice /cards │
└─────────────────────────────────┴──────────────────────────┘
```

## Crate 边界

| Crate | 依赖 | 作用 | 状态 |
|------|------|------|------|
| `exam-engine` | rusqlite / serde / inventory / thiserror | 纯数据 lib,无 GUI | ✅ 89 tests |
| `exam-cli` | exam-engine / clap | CLI 入口 | ✅ Phase A |
| `exam-app` | exam-engine / dioxus 0.7.9 | Dioxus desktop | ✅ Phase J |

## 模块边界

| 模块 | 责任 | 不可做 |
|------|------|--------|
| `types.rs` | newtype (BankId/QuestionId/CardId) + DTO | 不解析 |
| `error.rs` | thiserror 枚举,顶层 anyhow 转接 | 不静默吞错 |
| `source/*` | 读 JSON → Vec<LoadResult> | 不入 SQLite |
| `index.rs` | LoadResult → SQLite INSERT | 不解析 |
| `search.rs` | SearchQuery → Vec<Hit/CardHit> | 不存结果 |
| `scheduler.rs` | Interval → 调度循环 | 不解析 |
| `cache.rs` | 三态 + SQLite 持久化 | 不读源 |

## 不变量

1. `index.sqlite` 是**派生数据**,可删可重建;`banks/` + `crawl/` 是 source of truth
2. 错题本在 `~/Library/Application Support/exam-lookup-rs/` (R-006)
3. 所有 fetch 失败 → `CacheStatus::Stale` + `R-007`,不重试
4. 文档 parse (PDF/HTML) 延后到 Phase 后 (R-002 部分关闭)

## 性能预算(R-PERF)

| 指标 | 基线 | 上限 |
|------|------|------|
| `build_index` (200 + 200) | 22 ms | 35 ms |
| `search_fts` (单 CJK) | 1.5 ms | 2.5 ms |
| `search_like` (多 CJK OR) | 1.8 ms | 3.0 ms |

发版前 `cargo bench --bench perf` 数字不退化。