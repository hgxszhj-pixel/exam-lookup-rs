# R-PERF — Performance baselines (Phase I)

> **Date**: 2026-10-04(基线) / 2026-11-04(门禁上线 + 回归修复,见下)
> **Hardware**: macOS 24.6.0 (Apple Silicon, dev laptop)
> **Tool**: `divan 0.1.21` (Rust 1.76 workspace, release profile)
> **Fixture**: `tempdir` with `banks/demo.json` (200 questions) + `crawl/demo-问答.json` (200 cards) — programmatically generated, no real data dependency

## ⚠️ 阈值现在住在**代码**里,不在本文

> 2026-11-04 审计 P2-6:本文件里的阈值曾经只是 Markdown —— `benches/perf.rs` 六个 bench
> **零断言**,阈值永远不会被检查(**一道永远不会触发的假门禁**)。
> 现在:
> - 阈值定义在 `crates/exam-engine/src/perf_budget.rs`(常量 + 单元测试);
> - `benches/perf.rs` 每个 bench 都跑一道门禁,超限 → **进程退出码 1**;
> - 本文表格降级为**人类可读的副本** —— 改阈值改代码,别改本文。
>
> 本文的表格若与 `perf_budget.rs` 不一致,**以代码为准**。

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
| `search_like` | **LIKE 兜底路径**(2026-11-04 更正,见下) | 0.9 ms — LIKE 全表扫描 |
| `search_cards_fts` | card 表 FTS5 | 1.4 ms — 与 search_fts 同量级 |

> **2026-11-04 更正**:旧版本文把 `search_like` 描述成「多 CJK 短语 OR-join」——
> 那**正是 bug**:多字 CJK 查询会被 FTS 的「按字 OR-join」抢先命中,LIKE 兜底**根本没跑**,
> 这个 bench 量的是 FTS 路径却叫 LIKE。现在 fixture 里加了一道题干带 `A、B` 的题
> (ASCII 标点 → FTS 侧只有 `A` `B` 两个 token、没有 `AB`),查 `AB` 才必然落到 LIKE,
> 并且门禁会断言命中的 `match_kind == MatchKind::Like` 且恰好 1 条 —— **bench 自证路径**。

### Perf 预算(发版前 gate)

> 代码位置:`crates/exam-engine/src/perf_budget.rs`。判定统计量是 **P25**,不是中位数(理由见下节)。

| 指标 | 判定统计量 | 2026-11-04 实测 P25 | 容忍上限 | 代码常量 |
|------|---------|------------------|---------|---------|
| `build_index` | P25 | 17.5 ~ 22.0 ms | ≤ 28 ms | `BUILD_INDEX` |
| `search_fts` | P25 | ~1.13 ms | ≤ 2.5 ms | `SEARCH_FTS` |
| `search_like` | P25 | ~0.90 ms | ≤ 3.0 ms | `SEARCH_LIKE` |
| `search_cards_fts` | P25 | ~1.02 ms | ≤ 2.5 ms | `SEARCH_CARDS_FTS`(2026-11-04 补,此前有 bench 无阈值) |
| `load_json_banks` | P25 | 0.77 ~ 1.76 ms | ≤ 2.5 ms | `LOAD_JSON_BANKS` |
| `load_crawl_cards` | P25 | ~0.50 ms | ≤ 1.2 ms | `LOAD_CRAWL_CARDS` |

> 上表「实测 P25」是**本机**观测带。绝对耗时硬件相关,CI runner 上用
> `EXAM_PERF_BUDGET_SCALE` 统一放宽(见下)。

## 判定统计量:为什么是 P25 而不是中位数(2026-11-04 实测)

这台机器 load average 常年 3~4(邻居在跑 Safari / syspolicyd)。同一份代码、
同一 fixture,重复跑批,**中位数在批与批之间能差 60%**:

| 版本(file-backed,400 行 fixture) | 中位(批间波动) | min / P25(几乎不动) |
|---|---|---|
| 修复后 | 18.8 / 21.2 / 21.8 ms | 16.6 / 16.7 / 17.4 ms |
| 回归版(`RETURNING`) | 31.5 / 32.7 / 32.8 ms | 29.4 / 29.9 / 30.0 ms |

机器繁忙时**整个分布会平移**(某个批次整体慢了 60%),而不是产生个别离群点。
于是:

- **中位数**量的是「这台机器**当下**多忙」—— 会被邻居的 CPU 占用误伤成误红;
- **P25** 量的是「这份代码在这台机器上**能跑多快**」—— 这才是回归检测想要的量。

所以门禁用 **51 次采样的 P25**(`GATE_QUANTILE = 0.25`)判定,中位/min/max 只作诊断输出。

**诚实的边界**:P25 只挡得住**批内**的个别慢样本。**批间**的负载漂移(邻居一忙,
整批都慢)任何统计量都挡不住 —— 那部分余量靠**阈值本身留的 margin**。
换句话说:这道门禁抓的是 **≥1.3~1.4× 的回归**,不是 5% 的漂移。

## 门禁怎么才算「有效」(2026-11-04)

一道 codify 了却不会触发的门禁,比没有门禁更坏 —— 它给人虚假的安全感。
本门禁按下面三条自证:

1. **能变红**:把阈值压到实测值以下,门禁必须**退出码 1**。
   自证方式:`EXAM_PERF_BUDGET_SCALE=0.001 cargo bench --bench perf; echo $?` → `rc=1`。
   (`perf_budget.rs` 里有对应单元测试 `gate_fails_when_median_exceeds_limit`。)
2. **不能用 panic 当失败信号**:divan 0.1.21 在 `thread_pool.rs::broadcast_task` 里
   `catch_unwind` 掉 bench 函数里的 panic 并把 `Err` **直接 drop** —— panic 不传播,
   进程退出码**仍然是 0**。所以门禁用「记失败标志 + `main` 统一 `exit(1)`」。
3. **bench 要自证测的是哪条路径**:否则数字在、门禁也在,量的却是别的东西
   (旧 `search_like` 就是这么名不副实的)。每个门禁都断言自己测的路径
   (命中数、`match_kind`、fixture 没变形),不对就记失败。
4. **阈值要有「能抓住 / 不误伤」两边的实测证据**:每个阈值配一条单元测试,
   一边喂「修复后的实测值」断言判绿、一边喂「回归版的实测值」断言判红
   (见 `build_index_threshold_separates_fixed_from_regressed`、
   `load_json_banks_threshold_survives_contention`)。没有对面证据的阈值
   只是拍脑袋的数字。

### 门禁能抓住什么、抓不住什么

| | 结论 |
|---|---|
| ✅ 能抓住 | ≥1.3~1.4× 的**代码级**回归(换了实现、换了 SQL、换了 schema) |
| ✅ 能抓住 | bench fixture 变形 / bench 测错路径(自证断言会红) |
| ❌ 抓不住 | 5% 级的性能漂移(阈值余量本来就比这大) |
| ❌ 抓不住 | 批与批之间的机器负载漂移(任何统计量都挡不住) |
| ❌ 抓不住 | 换硬件后的绝对差异(CI 上用 `EXAM_PERF_BUDGET_SCALE` 归一) |

### `EXAM_PERF_BUDGET_SCALE`

共享 runner 比开发机慢 2~3 倍,阈值需要一个统一的缩放因子:

```bash
EXAM_PERF_BUDGET_SCALE=3 cargo bench --bench perf   # 上限 ×3
```

非法 / 非正 / NaN / inf 一律退回 `1.0`(缩放坏了不该让门禁失效)。

## 2026-11-04:门禁上线时抓到的一个真回归

**门禁第一次上线就红了** —— `build_index` 实测 33.8 ms,而当时的上限是 35 ms
(2026-10-04 按中位定的),只剩 3% 余量。
顺着查下去,根因是审计轮里 P1-1 引入的写入路径:

```sql
INSERT INTO q(...) VALUES (...) ON CONFLICT(src, id) DO UPDATE SET ... RETURNING rowid
```

**`RETURNING` 子句在 SQLite 里让这条 upsert 慢 3~4 倍**(400 行 fixture 实测):

| 写法 | 耗时 |
|------|------|
| 同样 upsert,**不带** `RETURNING` | 7.0 ms |
| 同样 upsert,**带** `RETURNING rowid` | 22.3 ms |

贵的不是 upsert 本身(去掉 `RETURNING` 后 upsert 几乎免费),而是 `RETURNING` ——
它让 SQLite 每行多走一条「物化输出行」的路。逐项 A/B(隔离 schema / 语句 / FTS 三种变量):

| 变体 | 耗时 | 结论 |
|------|------|------|
| A 旧 schema + 旧语句 | 6.6 ms | 基线 |
| B 新 schema + 新语句(含 RETURNING) | 22.3 ms | 慢在这 |
| C 新 schema + `INSERT OR REPLACE` 写 q | 5.5 ms | FTS 无罪 |
| D 新 schema + 普通 FTS INSERT | 21.2 ms | 还是慢 → 锁定 q 的 upsert |
| E 旧 schema + 新式 FTS | 5.7 ms | FTS 无罪 |

**修法**:去掉 `RETURNING`,rowid 改用一次主键索引 `SELECT rowid FROM q WHERE src = ? AND id = ?`
拿(upsert 只 UPDATE、不改 rowid → 两种分支下拿到的都是同一条记录的 rowid,语义不变)。

效果:`populate` 26.2 ms → 12.6 ms;`build_index` 门禁 **33.8 ms → 17.5 ms**,
比 2026-10-04 的原始基线(23.3 ms)还快。

### 阈值跟着统计量一起重标定(35 → 28 ms)

判定统计量从「中位」换成「P25」之后,阈值必须**重新标定** —— 否则会出现两种
都很糟的结局:阈值比真实分布还高(放回归版溜过去),或者比真实分布还低
(在机器一忙就误红)。2026-11-04 在**同一台机器、同一 fixture、同一 bench 上下文**
里把两个版本各测了一遍:

| 版本 | P25 | min | 相对 28 ms 阈值 |
|---|---|---|---|
| 修复后(upsert + `SELECT rowid`) | 22.0 ms | 20.1 ms | 余量 27% ✓ |
| 回归版(写回 `RETURNING`) | 33.3 ms | 30.9 ms | 超限 19% ✗ |

28 ms 落在两者之间 —— 修好的代码过,回归的代码红,两边都有实测支撑。
中间的 24 ms 也试过(按空闲批的 17.5 ms 留 37% 余量),但那只对空闲批成立:
繁忙批的实测 P25 就是 22.0 ms,24 ms 只剩 9% 余量,机器再忙一点就误红。

`load_json_banks` 同理重标定 **1.5 → 2.5 ms**:纯 serde_json 解析在繁忙时
P25 会漂到 1.76 ms,1.5 ms 的上限落在波动带中间 → 门禁会**误红**。
2.5 ms 给最差观测值留 42% 余量,仍能抓住 ≥1.4× 的回归。

回归测试(`crates/exam-engine/src/index.rs`):
- `fts_rowid_matches_q_rowid_one_to_one` —— `q_fts.rowid` 与 `q.rowid` 严格 1:1,
  且第二次 populate(走 UPDATE 分支)后 rowid 不变;
- `repopulate_updates_existing_row_content` —— 改题干后重新 populate,`q` 与 FTS 索引都要是
  新文本(挡住「插不进去就跳过」那类退化成静默错的改写)。

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
6. **`RETURNING` 在 SQLite 的 upsert 上很贵**(2026-11-04,见上)—— 别为了拿 rowid 就加它。

## 用法

```bash
# 跑全部 bench(median 模式,~30s)—— 超预算 → 退出码 1
cargo bench --bench perf

# 跑单个 bench
cargo bench --bench perf -- search_fts

# 快速 smoke(10 samples)
cargo bench --bench perf -- --sample-size 10

# 共享 runner 上放宽阈值
EXAM_PERF_BUDGET_SCALE=3 cargo bench --bench perf

# 自证门禁能变红
EXAM_PERF_BUDGET_SCALE=0.001 cargo bench --bench perf; echo "rc=$?"   # 期望 rc=1
```

## 已知 caveat

- 当前 fixture 是程序生成的「伪数据」,CJK + ASCII 混合;真实 `exam-lookup` 语料的查询分布可能更偏某侧
- macOS 调度器影响 ±15%(首次 iters 偏慢);读 `median` 不要读 `fastest`
- `Connection::open` 计入 search bench,真实 desktop 长生命周期是「连接池复用」,数字略悲观
- divan 输出 `Timer precision: 45 ns` 是 macOS mach_absolute_time 抖动下界,小于此差异不显著
- 门禁用**自己**的 51 次采样取 P25,与 divan 自己的 median 不是同一个测量 —— 两者数量级可比,不要逐位对齐
- 门禁的阈值是**本机**标定的;换机器/换 CPU 必须重新标定(否则要么误红,要么抓不住)

## CI 接线(2026-11-04)

仓库此前**没有任何 workflow** —— fmt / clippy / test 三道门禁从没在 CI 上跑过,
本文写的「Perf 预算超过即触发 CI failure」是一句**指向不存在的 CI 的话**。

现在 `.github/workflows/ci.yml` 有 3 个 job:

| job | runner | 内容 |
|---|---|---|
| `engine` | ubuntu | `cargo fmt --all -- --check` + `clippy -p exam-engine -p exam-cli --all-targets -D warnings` + `test -p exam-engine -p exam-cli` |
| `app` | macos | `clippy -p exam-app` + `test -p exam-app`(Dioxus desktop 在 Linux 上要 webkit2gtk 一堆 apt 包,macOS 原生构建零系统依赖) |
| `perf` | ubuntu | `cargo bench -p exam-engine --bench perf`,带 `EXAM_PERF_BUDGET_SCALE=3` |

Rust 版本锁在 `rust-toolchain.toml`(1.99.1)—— rustfmt 版本漂移会让 fmt 门禁
在无关 PR 上变红,那比没有门禁更坏。

**parity 门禁(`scripts/dump_parity.sh`)故意不在 CI 里**:它要外部 Python 仓库
`~/x_videos-dowloads/exam-lookup`,CI runner 上没有 —— 它仍然是手工门禁。

## 未来扩展(留作 Phase J/K watch)

- 真实语料 fixture:从 `/Users/hgxszhj/x_videos-dowloads/exam-lookup/crawl/` copy 一份 1k+ 问答
- 加 `card_fts` bm25 vs LIKE 对比 bench(可能 LIKE 反而更快,小数据集下)
- 加 `IndexBuilder.rebuild` 在「含 n banks + m cards」下的可扩展性 sweep(n=1/10/100)
- Dioxus GUI render bench — 浏览器侧 frame budget,差异过大再考虑
