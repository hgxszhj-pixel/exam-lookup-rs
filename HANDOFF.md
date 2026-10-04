# HANDOFF — exam-lookup-rs

**项目**:Rust + Dioxus 0.7.9 desktop,Python stdlib 题库迁移
**位置**:`/Users/hgxszhj/develop_software/exam-lookup-rs/`
**封盘**:v1.0 @ 2026-10-04 (commit cb175ae)
**数据源**(只读):`/Users/hgxszhj/x_videos-dowloads/exam-lookup/`

## 当前状态

| 维度 | 数值 |
|---|---|
| Crates | engine (lib) + app (bin, Dioxus) + cli (bin) |
| Tests | 91 passed |
| Lint | cargo clippy exit 0(0 errors) |
| Build | cargo build --workspace exit 0 |
| 真实数据 E2E | 21 banks / 1760 Q / 1278 cards / rebuild 0.37s |

## 已落档阶段

A 数据契约 · C 索引+FTS5+trigram · D 调度+缓存 · E Dioxus GUI ·
F 打磨 · H SettingsPage 数据根 + 真实数据 E2E ·
I divan 6 perf gates · I+ 主题实时预览 · I++ swatch 缩略图 ·
I+++ notify watcher 模块 · J 首启动 UX + 快捷键 · K 三篇 doc

报表:`docs/reports/R-001..R-014` + `R-COVERAGE.md` + `R-PERF.md`
设计:`docs/architecture.md` + `docs/port-from-python.md` + `docs/engine-design.md`

## 关键文件

```
crates/exam-engine/src/lib.rs          — pub mod 清单
crates/exam-engine/src/index.rs        — JSON → SQLite 一次性 build
crates/exam-engine/src/search.rs       — FTS5 + trigram n-gram
crates/exam-engine/src/watcher.rs      — notify 文件 watcher (I+++)
crates/exam-engine/src/source/         — inventory 自动注册 source
crates/exam-engine/benches/perf.rs     — divan 6 gates
crates/exam-engine/examples/smoke_realdata.rs — 真实数据 smoke
crates/exam-app/src/state.rs           — AppState (db + signals)
crates/exam-app/src/ui/settings.rs     — 数据根 + 主题 + DB 徽章
```

## 下次会话候选

1. `DataRootWatcher` 接入 `AppState` + SettingsPage "自动重建" 开关
2. `dioxus-ssr` 渲染 SettingsPage → HTML snap test
3. `scripts/dump_parity.sh` 实际生成(plan §6 列项,未实现)

## 验收命令

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo run --package exam-engine --example smoke_realdata
```