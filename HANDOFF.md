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
| Tests | **112 passed** (91 → 112, 2026-10-12 audit round +21) |
| Lint | cargo clippy exit 0(0 errors) |
| Build | cargo build --workspace exit 0 |
| 真实数据 E2E | 21 banks / 1760 Q / 1278 cards / rebuild 0.37s |

## 已落档阶段

A 数据契约 · C 索引+FTS5+trigram · D 调度+缓存 · E Dioxus GUI ·
F 打磨 · H SettingsPage 数据根 + 真实数据 E2E ·
I divan 6 perf gates · I+ 主题实时预览 · I++ swatch 缩略图 ·
I+++ notify watcher 模块 · J 首启动 UX + 快捷键 · K 三篇 doc ·
**L 2026-10-12 audit round (3 reviewer 并行 → 12 fix steps)**

报表:`docs/reports/R-001..R-014` + `R-COVERAGE.md` + `R-PERF.md` + **`docs/plans/2026-10-12-audit-fix.md`**
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
**PR**:#1 OPEN @ `audit-fix-2026-10-12` branch(commits `2403a7d` fix + `6b597ac` docs)。
**Author**:hgxszhj-pixel(本机 git user.name,只影响本 repo 不动 global)。

修复总览:P0 unsafe Sync → Mutex;P1 caps(query 4096B / JSON 64MiB / data_root canonicalize);P1 observability(tracing + spans);P3 test coverage(cjk_expand / watcher / rebuild)。

**NO-OP / SKIP**:
- 2.3 IndexBuilder DRY(三方法组合无重复)
- 2.4 CLI main 拆分(pure readability,Rule 3 范围外)
- 3.4 Scheduler::tick backoff(feature 未实现,等 TDD 红再做)

**测试驱动发现的契约**(已写进 test doc):
1. `rebuild` fail-fast(source 失败不 graceful 容错,`populate` 用 `?` 短路)
2. `stats.questions` / `stats.cards` 是输入计数,非 DB 行数(DB 真实行数需 `b.stats()`)

## 下次会话候选

1. `DataRootWatcher` 接入 `AppState` + SettingsPage "自动重建" 开关
2. `dioxus-ssr` 渲染 SettingsPage → HTML snap test
3. `scripts/dump_parity.sh` 实际生成(plan §6 列项,未实现)
4. `Scheduler::tick` backoff(3.4 NO-OP 升级,需先实现 feature 再补 TDD test)

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