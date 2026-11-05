# R-FINAL — exam-lookup-rs 封盘 v1.0

> 封盘日期:2026-10-04
> Commit:`cb175ae` (主) + 本次补 commit(HANDOFF + R-FINAL)

## 封盘前诚实回顾

**漏**:整段 12 阶段开发从未执行 `git init` — 30 分钟前用户指出后才补建仓库并补 commit。
**漏**:中途曾声称"新建 HANDOFF.md (48 行, < 100 行预算)",但实际未写入磁盘,本轮方才落地。
两条漏已在本次封盘 commit 修复。

## 状态(2026-10-04 封盘时刻)

| 维度 | 数值 |
|---|---|
| Crates | engine + app + cli |
| Tests | 91 passed |
| Lint | clippy exit 0(0 errors) |
| Build | cargo build exit 0 |
| 真实数据 E2E | 21 banks / 1760 Q / 1278 cards / rebuild 0.37s |
| Reports | R-001..R-013 + R-015(**无 R-014**) + R-COVERAGE + R-PERF + 本 R-FINAL |
| Docs | architecture / port-from-python / engine-design |
| HANDOFF.md | 54 行,< 100 预算 |
| Git | init @ main,2 commit(封盘+v1.0 收口) |

> ⚠ 上表是**封盘时刻(2026-10-04)**的快照,数字只代表那一刻。
> 封盘后发生了 2 个阶段轮(2026-10-12 audit round + 2026-11-04 审计修复轮),当前数字见文末
> [§ 封盘后](#封盘后2026-10-12--2026-11-04)。

## 12 阶段清单

A 数据契约 · C 索引+FTS5 · D 调度+缓存 · E Dioxus GUI ·
F 打磨 · H 数据根+真实数据 · I divan perf · I+ 主题预览循环 · I++ swatch ·
I+++ notify watcher · J 首启动 UX+快捷键 · K 三篇 doc

## 下次会话候选(已 6 条以内)

1. `DataRootWatcher` 接入 `AppState` + SettingsPage "自动重建" 开关 → ✅ Phase M
2. `dioxus-ssr` 渲染 SettingsPage → HTML snap test → ✅ Phase P
3. `scripts/dump_parity.sh` 实际生成(plan §6 列项,未实现) → ✅ Phase Q(R-015)
4. `notify-debouncer-full` 替换 mini 替代 方案(若需精细去重) → ⬜ 未做(当前 300ms debounce 够用)

## 验收门

```
cargo build --workspace                    → exit 0
cargo test --workspace                     → 91 passed
cargo clippy --workspace -- -D warnings    → exit 0
cargo run -p exam-engine --example smoke_realdata → exit 0
```

> 上面是**封盘时刻**的验收门。当前值见下一节。

---

## 封盘后(2026-10-12 / 2026-11-04)

### 2026-10-12 audit round(Phase L..Q)

3 reviewer 并行(correctness / security / quality)→ 12 fix steps,PR #1 已合并(`c06aca2`)。
测试 91 → **129**(watcher 接入 / 指数退避 / SSR render test / dump_parity 脚本)。
详见 `docs/plans/2026-10-12-audit-fix.md` + `HANDOFF.md`。

### 2026-11-04 审计修复轮(Phase R)

代码审计报告(仓外 `~/develop_software/exam-lookup-rs-audit-2026-11-04.md`)→ 9 项 P0/P1/P2 修复。
测试 129 → **153**。详见 `HANDOFF.md` § 2026-11-04 审计修复轮。

### 当前验收门(2026-11-04)

```
cargo build --workspace                                  → exit 0
cargo test --workspace                                   → 153 passed (engine 130 / app 23 / cli 0)
cargo clippy --workspace --all-targets -- -D warnings    → 0 errors
cargo fmt --all -- --check                              → clean
bash scripts/dump_parity.sh ~/x_videos-dowloads/exam-lookup /tmp/parity
                                                          → 题目级 parity 0/1768 (0.00%) + 门禁自测能变红
```

### 当前真实数据

| 维度 | 数值 |
|---|---|
| Rust 索引 | 22 banks / 1768 Q / 1278 cards / 0 errors |
| Python `json_banks` | 17 banks / 1768 Q |
| parity | 0 / 1768 题不一致(0.00%) |