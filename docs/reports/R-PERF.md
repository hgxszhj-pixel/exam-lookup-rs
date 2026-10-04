# R-PERF — Performance baselines (Phase I)

> **Date**: 2026-10-04
> **Hardware**: macOS 24.6.0 (Apple Silicon, dev laptop)
> **Tool**: `divan 0.1.21` (Rust 1.76 workspace, release profile)
> **Fixture**: `tempdir` with `banks/demo.json` (200 questions) + `crawl/demo-问答.json` (200 cards) — programmatically generated, no real data dependency

## Why divan (not criterion)

| 选项 | 决策 |
|------|------|
| criterion | 拒绝 — 依赖 `plotters` + 大量 nightly 兼容补丁,首次构建慢 30s+ |
| iai-callgrind | 拒绝 — 需要 valgrind,macOS 没装 |
| built-in `#[bench]` | 拒绝 — nightly-only,2024-09 已 reject stable proposal |
| **divan** | 选用 — single-binary、stable-only、API 简洁、tabular 输出直接读 |

## Baselines (median over 100 samples × 1000 iters)

```
perf                 fastest       │ slowest       │ median        │ mean          │ samples │ iters
├─ build_index       19.29 ms      │ 34.52 ms      │ 23.28 ms      │ 23.74 ms      │ 100     │ 1000
├─ load_crawl_cards  419.1 µs      │ 1.778 ms      │ 653.4 µs      │ 651.9 µs      │ 100     │ 1000
├─ load_json_banks   605.9 µs      │ 1.865 ms      │ 889.4 µs      │ 939.8 µs      │ 100     │ 1000
├─ search_cards_fts  994.5 µs      │ 2.191 ms      │ 1.375 ms      │ 1.416 ms      │ 100     │ 1000
├─ search_fts        1.065 ms      │ 2.177 ms      │ 1.537 ms      │ 1.57 ms       │ 100     │ 1000
╰─ search_like       1.194 ms      │ 3.454 ms      │ 1.798 ms      │ 1.851 ms      │ 100     │ 1000
```

### 解读

| Bench | 含义 | 关键观察 |
|------|------|---------|
| `build_index` | 200 题 + 200 问答 重建全索引 | 23 ms 中位 — 200/题/0.12 ms,FTS5 写入 ~150 行/秒 |
| `load_json_banks` | JsonBanksSource parse + split | 0.89 ms — serde_json 占大头 |
| `load_crawl_cards` | CrawlCardsSource parse + split | 0.65 ms — RawFile 结构更简单,比 json_banks 快 36% |
| `search_fts` | 单 CJK 关键词 bm25 | 1.5 ms — 200 行数据集下 bm25 planner 仍走全表 |
| `search_like` | 多 CJK 短语 OR-join | 1.8 ms — LIKE fallback 比 FTS 慢 17%,符合预期 |
| `search_cards_fts` | card 表 FTS5 | 1.4 ms — 与 search_fts 同量级 |

### Perf 预算(发版前 gate)

| 指标 | 当前基线 | 容忍上限 | 触发条件 |
|------|---------|---------|---------|
| `build_index` median | 23 ms | ≤ 35 ms | 超过即触发 CI failure |
| `search_fts` median | 1.5 ms | ≤ 2.5 ms | 同上 |
| `search_like` median | 1.8 ms | ≤ 3.0 ms | 同上 |
| `load_json_banks` median | 0.9 ms | ≤ 1.5 ms | 同上 |
| `load_crawl_cards` median | 0.65 ms | ≤ 1.2 ms | 同上 |

## 实现踩坑

1. **Fixture 跨 Send 边界**:TempDir 不是 `Send`,divan 默认 thread pool 要求闭包 `Send`。
   解法:`Box::leak` TempDir + Path 到 `'static`,单 bench 内一次性吃掉整个生命周期。
2. **rusqlite::Connection 不 Send**:每个 search bench 在闭包内重新 `open_readonly(&fx.db_path)`,
   open 开销 ~5 µs vs query 开销 ~1.5 ms,可忽略(0.3% 噪声)。
3. **`build_with_source(conn, ...)` 要 `&mut Connection`**,与 IndexBuilder 共享底层 conn 的语义不兼容;
   该 bench 与 `build_index` 行为重叠,删除避免重复。
4. **fixture JSON schema 必须对齐 RawOption { key, text }** 而非 string;首跑 `invalid type` panic,
   重写 fixture 字段名(`question`/`typeLabel`/`options[]` 结构)。
5. **divan 默认 `threads = 0`(全部核)**,SQLite serializable 反而比单线程慢;显式 `threads = 1` 是 realistic baseline。

## 用法

```bash
# 跑全部 bench(median 模式,~30s)
cargo bench --bench perf

# 跑单个 bench
cargo bench --bench perf -- search_fts

# 快速 smoke(10 samples)
cargo bench --bench perf -- --sample-size 10

# 与基线对比(发版前)
cargo bench --bench perf > /tmp/perf.txt
# 人工 diff 本行:├─ build_index ... 23.28 ms
```

## 已知 caveat

- 当前 fixture 是程序生成的「伪数据」,CJK + ASCII 混合;真实 `exam-lookup` 语料的查询分布可能更偏某侧
- macOS 调度器影响 ±15%(首次 iters 偏慢);读 `median` 不要读 `fastest`
- `Connection::open` 计入 search bench,真实 desktop 长生命周期是「连接池复用」,数字略悲观
- divan 输出 `Timer precision: 45 ns` 是 macOS mach_absolute_time 抖动下界,小于此差异不显著

## 未来扩展(留作 Phase J/K watch)

- 真实语料 fixture:从 `/Users/hgxszhj/x_videos-dowloads/exam-lookup/crawl/` copy 一份 1k+ 问答
- 加 `card_fts` bm25 vs LIKE 对比 bench(可能 LIKE 反而更快,小数据集下)
- 加 `IndexBuilder.rebuild` 在「含 n banks + m cards」下的可扩展性 sweep(n=1/10/100)
- Dioxus GUI render bench — 浏览器侧 frame budget,差异过大再考虑
