# DataRootWatcher → AppState 接入 Plan — 2026-10-12

> 启动候选:#1(来自 HANDOFF.md "下次会话候选")
> 触发:用户从 AskUserQuestion 选 "DataRootWatcher 接入 AppState"
> 期望终态:SettingsPage 勾选「自动重建」→ 监听 data_root 文件变化 → 自动触发 `rebuild_index`
> 封盘基线:112 tests, clippy clean

## 架构决策(关键 4 问)

### Q1:Watcher 线程放哪里?

**选项 A**:`AppState` 持 `Mutex<Option<WatcherHandle>>`,`WatcherHandle { _watcher: DataRootWatcher, _consumer: JoinHandle<()> }` — Drop 自动停
**选项 B**:单独 `WatcherService` struct,AppState 持 `Arc<WatcherService>`
**选项 C**:全局 static(用 `OnceLock` / `Mutex<Option<...>>`)

**决策:A**。原因:
- AppState 已是单例(`provide_context` 注入),watcher 生命周期 = app 生命周期最自然
- `Mutex<Option<...>>` 模式与现有 `db: Mutex<Option<Connection>>` 一致(Rule 11 匹配代码库约定)
- B 过度工程(独立 type 价值低),C 与单测不友好(无 AppState 隔离)

### Q2:如何防止并发 rebuild?

**问题**:`rebuild_index` 同步 + 多入口(用户点按钮 + watcher 自动触发)+ 无内部锁 → race condition(两个线程同时改 `self.db`)

**方案**:`Arc<Mutex<()>>` rebuild_lock 在 AppState,锁内只跑 `rebuild_index`。
- 自动 thread:try_lock;锁不上 → skip(下个事件会重试)
- "Rebuild now" 按钮:try_lock;锁不上 → UI 显示"重建中…"

**决策**:用 `Mutex` 而非 `RwLock` — rebuild 永远独占,无并发读语义。

### Q3:SettingsPage 切换 on/off 怎么映射?

**选项 A**:toggle 立即 spawn/kill 线程
**选项 B**:toggle 只改 flag,后台线程读 flag 决定是否触发 rebuild;线程长驻

**决策:B**。原因:
- 反复 spawn/kill 线程 + 重启 OS watcher 浪费
- 简单 `AtomicBool enabled` 即可,线程循环 `if !enabled { drop_or_sleep; continue; }`
- 顺带解决 "data_root 改了" 的需求:线程长驻,只需在循环顶部检查当前 data_root 是否与启动时一致;不一致则重启 watcher

### Q4:auto_rebuild 持久化?

**决策**:加到 `AppSettings`,`RawSettings` 同步加字段。
- 默认 `false`(用户主动开)
- 改完保存后,启动 watcher

## 实施步骤

### ✅ Step 0:Plan + task list 落地

### Step 1:数据模型 + 持久化(RED → GREEN)

- `AppSettings` 加 `pub auto_rebuild: bool`
- `Default::default()` 给 `auto_rebuild: false`
- `RawSettings` 加 `auto_rebuild: bool`
- `load_settings` / `save_settings` 序列化兼容(老 settings.json 无此字段 → 视为 false)
- `state.rs::tests`:加 2 个 test
  - `settings_roundtrip_with_auto_rebuild`
  - `settings_legacy_file_without_auto_rebuild_defaults_false`

**Regression**:112 → 114 passed

### Step 2:watcher 线程 + rebuild 锁(RED → GREEN)

- `AppState` 加:
  - `pub rebuild_lock: Arc<Mutex<()>>` — 防并发
  - `pub watcher: Arc<Mutex<Option<WatcherHandle>>>`
  - `pub watcher_enabled: Arc<AtomicBool>` — 改 AppSettings.auto_rebuild 时同步
- `WatcherHandle` struct:`{ _watcher: DataRootWatcher, _consumer: JoinHandle<()> }`(字段用 `_` 前缀因为只持有保活)
- `AppState::ensure_watcher(&self) -> Result<(), String>` — 幂等:有就 return;没有就启动
- `AppState::stop_watcher(&self)` — Option::take,Drop 自动 stop
- `AppState::rebuild_index` 改为先 `try_lock(rebuild_lock)`,锁不上 → `Err("已有重建在进行")` 早返回
- 消费线程逻辑:
  ```rust
  while watcher_enabled.load() {
      match rx.recv_timeout(Duration::from_millis(500)) {
          Ok(_event) => { let _ = try_rebuild(); }
          Err(RecvTimeoutError::Timeout) => continue,
          Err(RecvTimeoutError::Disconnected) => break, // watcher dropped
      }
  }
  ```
- `state.rs::tests` 加 3 个 test:
  - `rebuild_lock_prevents_concurrent_rebuilds`(锁 + 第二个 try_lock 应返 Err)
  - `ensure_watcher_is_idempotent`(调两次不应 double-spawn)
  - `stop_watcher_clears_handle`

**Regression**:114 → 117 passed

### Step 3:UI 集成(SettingsPage 改造)

- 加 `<input type="checkbox">` 「自动重建」 toggle
- onchange → 改 `state.watcher_enabled` + `AppSettings.auto_rebuild` + `save_settings` + 调 `ensure_watcher` / `stop_watcher`
- DB 状态徽章处加副标题:「自动重建:开/关」
- "Rebuild now" 按钮被锁时显示 "重建中…"(现有逻辑已有,扩展处理 try_lock 失败)

**Regression**:117 passed(无新增 engine test)

### Step 4:bootstrap 路径

- `AppState::bootstrap` 末尾:如果 `settings.auto_rebuild` 为 true → 调 `ensure_watcher`
- data_root 默认值是 `/Users/hgxszhj/x_videos-dowloads/exam-lookup`,该目录存在 → 应能启动
- 启动失败(目录消失)→ `ensure_watcher` 返 Err,UI 显示错误,**不** panic

**Regression**:117 passed

### Step 5:documentation + HANDOFF 同步

- HANDOFF.md 加 Phase M "watcher→AppState 接入"
- `~/memory/projects/exam-lookup-rs.md` 加本轮踩坑(如有)
- `docs/handoff/sessions/2026-10-12-watcher-integration.md` 完整流水(参照之前的 handoff 模板)

## 文件清单

| 文件 | 改动 |
|---|---|
| `crates/exam-app/src/state.rs` | 加 `auto_rebuild` 字段 + `WatcherHandle` + `rebuild_lock` + `watcher_enabled` + 4 method + 5 test |
| `crates/exam-app/src/ui/settings.rs` | 加 checkbox + onchange handler + 副标题 + 锁状态 UI |
| `crates/exam-app/src/main.rs` | 不动(tracing init 已有) |
| `HANDOFF.md` | 加 Phase M + tests 117 |
| `docs/plans/2026-10-12-watcher-integration.md` | 本文件 |
| `docs/handoff/sessions/2026-10-12-watcher-integration.md` | 完整 session 流水 |

## 风险与回滚

- **线程泄漏**:`stop_watcher` 不 join(只 Option::take) — OK,因为 consumer 线程是 daemon 风格,app 退出时整进程关
- **rebuild 太频繁**:data_root 写 1 次 → debouncer 触发 1 次,简单循环再触发 → 频率 < 1Hz;真实场景下手动 `cp` 一批 bank 才会有 burst
- **rebuild 失败**:写 db_status = Error,SettingsPage 徽章变红;用户可手动重试
- **目录消失**:`watcher` 启动时 `is_dir()` 失败 → 返 Err,UI 不挂;重新选目录可恢复

## 验收命令

```bash
cargo test --workspace              # 117 passed
cargo clippy --workspace -- -D warnings
cargo run --package exam-app        # GUI 模式;勾选"自动重建"→ 改 data_root 文件 → DB 徽章应变化
```

## 不在本次范围

- 跨平台(Windows)watcher 行为差异测试(macOS-only smoke)
- 多 data_root 并行(假设单 data_root)
- rebuild 进度条(目前只 OK/Err 反馈)
