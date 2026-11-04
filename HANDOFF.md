# HANDOFF — exam-lookup-rs

**项目**:Rust + Dioxus 0.7.9 desktop,Python stdlib 题库迁移
**位置**:`/Users/hgxszhj/develop_software/exam-lookup-rs/`
**封盘**:v1.0 @ 2026-10-04 (commit cb175ae)
**数据源**(只读):`/Users/hgxszhj/x_videos-dowloads/exam-lookup/`
**GitHub**:https://github.com/hgxszhj-pixel/exam-lookup-rs(注意:是 pixel 账号,不是 hgxszhj)

## 当前状态

| 维度 | 数值 |
|---|---|
| Crates | engine (lib) + app (bin, Dioxus) + cli (bin) |
| Tests | **153 passed** (engine 130 + app 23 + cli 0)— 历史:91 → 112 audit +9 → 121 watcher +2 → 123 watcher-restart +4 → 127 backoff +2 → 129 ssr(2026-10-12)→ **153**(2026-11-04 审计修复轮 +24) |
| Lint | cargo clippy --workspace --all-targets -- -D warnings exit 0(0 errors) |
| Build | cargo build --workspace exit 0 |
| 真实数据 E2E | Rust 索引 **22 banks / 1768 Q / 1278 cards / 0 errors**;Python `json_banks` 侧 17 banks / 1768 Q → 题目级 parity **0/1768(0.00%)** |

## 已落档阶段

A 数据契约 · C 索引+FTS5+trigram · D 调度+缓存 · E Dioxus GUI ·
F 打磨 · H SettingsPage 数据根 + 真实数据 E2E ·
I divan 6 perf gates · I+ 主题实时预览 · I++ swatch 缩略图 ·
I+++ notify watcher 模块 · J 首启动 UX + 快捷键 · K 三篇 doc ·
**L 2026-10-12 audit round (3 reviewer 并行 → 12 fix steps)** ·
**M 2026-10-12 watcher→AppState 接入(SettingsPage 自动重建 toggle + 5 步集成)** ·
**M+ 2026-10-12 sync_watcher:data_root 变化时强制重启 watcher** ·
**N 2026-10-12 Scheduler::tick 失败 source 指数退避(防雪崩,R-007 配套)** ·
**O 2026-10-12 watcher.rs test 5 unused_mut 清理(clippy --all-targets 0 error)** ·
**P 2026-10-12 SettingsPage dioxus-ssr 结构化 render test(防 UI 回归)** ·
**Q 2026-10-12 dump_parity.sh 实际生成 — 1760 Q 0.00% diff(R-015)**
**R 2026-11-04 审计修复轮 — 9 项 P0/P1/P2 修复落地(P0-1/P0-2/P1-1/P1-2/P1-3/P1-4/P2-3/P2-7 + 文档对齐 P2-5)→ 153 tests**

报表:`docs/reports/R-001..R-013` + `R-015`(**无 R-014**) + `R-COVERAGE.md` + `R-PERF.md` + `R-FINAL.md` + **`docs/plans/2026-10-12-audit-fix.md`** + **`docs/plans/2026-10-12-watcher-integration.md`**
设计:`docs/architecture.md` + `docs/port-from-python.md` + `docs/engine-design.md`

## 关键文件

```
crates/exam-engine/src/lib.rs          — pub mod 清单
crates/exam-engine/src/index.rs        — JSON → SQLite 一次性 build + rebuild 边界
crates/exam-engine/src/search.rs       — FTS5 + trigram n-gram + query cap
crates/exam-engine/src/cache.rs        — 移除 unsafe impl Sync(已 Mutex 包)
crates/exam-engine/src/scheduler.rs    — Arc<Mutex<CacheStore>>(thread-safe)
crates/exam-engine/src/watcher.rs      — notify 文件 watcher (I+++)
crates/exam-engine/src/parse.rs        — cjk_expand + norm
crates/exam-engine/src/source/         — inventory 自动注册 + load_json_bounded
crates/exam-engine/benches/perf.rs     — divan 6 gates
crates/exam-engine/examples/smoke_realdata.rs — 真实数据 smoke
crates/exam-app/src/state.rs           — AppState (db + signals)
crates/exam-app/src/ui/settings.rs     — 数据根 + 主题 + DB 徽章
```

## 2026-10-12 Audit Round 摘要

3 reviewer 并行 (correctness / security / quality) → 12 fix steps 落地,见 `docs/plans/2026-10-12-audit-fix.md`。
**PR**:#1 **已合并**(`c06aca2`)。
**Author**:hgxszhj-pixel(本机 git user.name,只影响本 repo 不动 global)。

修复总览:P0 unsafe Sync → Mutex;P1 caps(query 4096B / JSON 64MiB / data_root canonicalize);P1 observability(tracing + spans);P3 test coverage(cjk_expand / watcher / rebuild)。

**NO-OP / SKIP**:
- 2.3 IndexBuilder DRY(三方法组合无重复)
- 2.4 CLI main 拆分(pure readability,Rule 3 范围外)
- 3.4 Scheduler::tick backoff(feature 未实现,等 TDD 红再做)

**测试驱动发现的契约**(已写进 test doc):
1. `rebuild` fail-fast(source 失败不 graceful 容错,`populate` 用 `?` 短路)
2. `stats.questions` / `stats.cards` 是输入计数,非 DB 行数(DB 真实行数需 `b.stats()`)

## 2026-10-12 Phase M:Watcher 接入 AppState 摘要

`I+++` 阶段的 `DataRootWatcher` 是 engine 的独立 primitive,本轮把它接入 GUI:

**架构决策(4 问)**:
1. **线程位置**:`AppState.watcher: Mutex<Option<JoinHandle<()>>>` — 线程长驻,Drop = 线程继续跑直至 enabled=false
2. **并发保护**:`AppState.rebuild_lock: Arc<Mutex<()>>` + `rebuild_index` 改用 `try_lock` — 防 watcher + 按钮 + bootstrap 三方同时跑
3. **启停模式**:`watcher_enabled: AtomicBool` 开关,消费线程 `rx.recv_timeout(500ms)` 循环,关闭最迟 500ms 内退出
4. **持久化**:`AppSettings.auto_rebuild: bool` 字段,`RawSettings` `#[serde(default)]` 兼容老 JSON

**5 步集成**:
- Step 1: `AppSettings.auto_rebuild` + `RawSettings` 字段 + 2 test
- Step 2: `AppState` 3 字段 + `ensure_watcher` / `stop_watcher` / `rebuild_index.try_lock` + 3 test
- Step 3: SettingsPage 加 checkbox 卡片(实时启停 + 状态文字)
- Step 4: `bootstrap_from` 公开 + 启动时按 saved auto_rebuild 启 watcher + 3 test
- Step 5: 本 HANDOFF + memory 同步

**测试**:118 → 121(+3)

**踩坑**:
- Dioxus `use_signal` 初始化时加 `mut` 但暂未用 → 后续用时再移除或 `#[allow(unused_mut)]`,不要"先 mut 后不加用"
- `state` 在多 closure 中共享需 `.clone()`(AppState derives Clone,Arc 字段代价小)
- `DataRootWatcher` 必须由消费线程持有(`Drop` 即停 OS watcher),不能放 handle slot
- `mpsc::Receiver::recv_timeout` 取 `&self`,消费线程不需要 `mut watcher`

## 2026-10-12 Phase M+:sync_watcher(data_root 变更时强制重启)

**问题**:Phase M 启的 watcher 监听 data_root_A;用户改路径到 B,保存后旧线程仍在监听 A,事件丢失。
**根因**:`ensure_watcher` 幂等(`is_some → no-op`),不会因为 settings 变化而重启。
**修法**:`AppState::sync_watcher(enabled: bool)` — enabled=true 走 `stop + ensure`(强制重启),enabled=false 走 `stop`。
**集成**:SettingsPage 保存按钮在 `state.settings = new_settings` 之后调 `state.sync_watcher(new_settings.auto_rebuild)`,保证监听路径与 settings 对齐。
**Test**:123 = 121 + 2(sync_watcher_restarts_after_data_root_change / sync_watcher_disabled_stops_running)。

## 2026-10-12 Phase N:Scheduler::tick 失败 source 指数退避**问题**:Audit 3.4 标 NO-OP — 失败 source 当前每 tick 都重试,雪崩风险。
**修法**:`ScheduleEntry` 加 `attempts: u32` + `next_retry_at: Option<i64>`。
- 失败 path:`attempts++` + `next_retry_at = now + next_backoff_secs(attempts)`
- 成功 path:`attempts = 0` + `next_retry_at = None`
- `due_indices` 顶部:`now < next_retry_at → continue` 防重试
- backoff 序列:5, 10, 20, 40, 80, 160, 300(MAX 封顶;5s 基线覆盖 Interval::Test=1s 跳过 ≥5 tick)
- `next_backoff_secs` 纯函数 + 单测覆盖 1/2/3/4/7/1000 attempts

**Test**:127 = 123 + 4
- `tick_does_not_retry_failed_source_within_backoff_window` — 3 tick 在窗口内,counter=1
- `tick_retries_failed_source_after_backoff_window_expires` — t+6 后重试,counter=2
- `tick_success_clears_backoff_state` — flaky source 转 OK 后 attempts=0
- `next_backoff_secs_grows_then_caps` — 指数递增 + MAX 封顶 + 极端 attempts 不 overflow

**Commit**:`0b59024`

**踩坑**:
- `clippy::unnecessary_cast` — `BASE_BACKOFF_SECS as i64` 多余(const 已是 i64);直接 `BASE_BACKOFF_SECS.saturating_mul(...)`
- `cargo clippy --all-targets -- -D warnings` 5 个 pre-existing warnings 在 `watcher.rs:78/111/136/161/185` 的 `let mut watcher`(audit round 未修,test target 专属,非本 PR 范围)
- `1i64 << shift` 必须 `min(20)` 否则 `attempts > 64` 时 UB;saturating_sub + min 双保险

## 2026-10-12 Phase Q:dump_parity.sh 实际生成(R-015)

**目的**:Audit 计划 #3 — Python `qbank.py` dump vs Rust `exam-cli dump` 输出 parity 校验。
**数据源**:`/Users/hgxszhj/x_videos-dowloads/exam-lookup`(21 banks / 1760 Q / 1278 cards)。

**结果**:
- Python:1760 questions (1 source `json_banks`,21 banks)
- Rust:1760 questions (2 sources `json_banks` + `crawl_cards`)
- diff:6 行 ≈ 0.00% → ✅ parity OK (≤ 1%)

**唯一差异**:Rust 多 1 source key `crawl_cards: {"保密公开题-问答": {meta:null, questions:[]}}`(空 source,数据迁移未完成)。
**根因**:Python `dump_banks.py` 只 `glob('banks/*.json')`,Rust 全 source registry 都输出。**不是 bug**,是采样范围差异。

**收口**:差异来源已记录,无需修代码(改任一方都是过度工程)。Report:`docs/reports/R-015-dump-parity.md`。

> ⚠ **2026-11-04 修正**:上面 "diff:6 行 ≈ 0.00%" 的**判据是坏的** —— 分母用 `wc -l` 行数,而两个 dump 都是单行 JSON → 分母 0 → 差异率恒为 0.00%(假绿门禁)。P0-2 已把分母改成题目条数,同日重跑真实数据:**0/1768 不一致(0.00%)**,并加了门禁自测证明它能变红。详见 R-015。

## 2026-11-04 审计修复轮(Phase R)

**来源**:代码审计报告 `~/develop_software/exam-lookup-rs-audit-2026-11-04.md`(仓外)。
**分支**:`worktree-audit-fix-2026-11-04`(worktree 隔离,未合 main)。
**结果**:129 → **153 tests**(engine 130 / app 23 / cli 0),clippy 0 error,fmt clean,真实数据 parity 0.00%。

| # | 修复 | 根因 → 修法 |
|---|---|---|
| P0-1 | 宽容反序列化 + per-file 隔离 | `answerText` 类型不同 → 整个 bank 反序列化失败 → `Option` 宽容解析 + 单文件失败不拖垮整 bank |
| P0-2 | parity 门禁分母修正 | 分母用 `wc -l` 行数(两个 dump 都是单行 JSON → 分母 0 → 恒 0.00% 假绿)→ 改题目级比较,分母 = 题目条数;新增门禁自测(能变红) |
| P1-1 | `q_fts` 扇出 + 跨 bank 同 id 丢题 | FTS 触发词多行 + `q.id` 主键跨 bank 撞 id → `q_fts` 去重 + id 加 bank 前缀 |
| P1-2 | LIKE 转义失效 | `LIKE` 未带 `ESCAPE` → `%`/`_` 语义被吃 → 补 `ESCAPE '\'` 并让测试真的验证 SQL(旧测试查的是常量,不是行为) |
| P1-3 | watcher 代际隔离 + 真正 join | 共享 stop flag → 新代 `store(true)` 复活旧代,旧代用**旧 data_root** 重建覆盖新索引 → 每代专属 flag + `stop_watcher` 真正 `join()`(旧代码只 `take()` 句柄 = detach) |
| P1-4 | 重建索引移出 UI 线程 | 同步重建 → 窗口冻结,且 `rebuilding.set(true)` 与 `set(false)` 之间无让出点 → "重建中…" 永远渲染不出来 → 重建搬到后台 OS 线程,结果经 `futures_channel::oneshot` 回 UI 线程再更新 signal |
| P2-3 | DB 迁移路径 | 无 `user_version` → schema 变更无迁移通道 → 加 `user_version` + 迁移脚本 |
| P2-7 | cache status 按读取时刻计算 | 存的 `status` 是**写时**值 → 读出来永远"新鲜" → 读取时按 age 重算(但保留写时的 `Expired` 失败标记) |
| P2-5 | 文档数字对齐 | README/HANDOFF/报表的测试数、bank 数、source 数、Dioxus 版本、Phase 序号全漂 → 全部对齐实测值 |

**踩坑**:
- `dioxus::spawn` **不存在** —— `spawn` 在 `dioxus::prelude`(`dioxus_core` 导出);且 Dioxus desktop 的 executor 就是主线程,光用 `dioxus::prelude::spawn` 照样卡 UI → 阻塞活儿必须走 `std::thread`。
- `Signal<T>` 默认 `UnsyncStorage`(非 `Send`/`Sync`)→ 跨线程回写 signal 会编译失败;用 oneshot 把结果送回 UI 线程再 `set`。
- cache status 重算一度把 scheduler 的**失败标记**一起洗掉(`tick` 失败时写 `status=Expired`)。判别依据:成功加载恒写 `Fresh`(`compute_status(now, ttl, now)` → age 0 < ttl;所有 `Interval` 的 ttl ≥ 1s),所以"存储值是 `Expired`" ⇔ "上次加载失败",保留不动。

## 下次会话候选

- **合并本轮**:branch `worktree-audit-fix-2026-11-04` → main(PR)。
- **R-004 决策未落地**:若仍要精确锁 `dioxus =0.7.9`,改 `crates/exam-app/Cargo.toml` 的 caret 为 `=0.7.9`(当前实解 0.7.10,代码在其上编译/测试全绿)。

## 验收命令

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo run --package exam-engine --example smoke_realdata
# CLI smoke
cargo run --bin exam-cli -- load --source json_banks --data-root /tmp/nonexistent_xxx
cargo run --bin exam-cli -- load --source json_banks --data-root /tmp/.
```