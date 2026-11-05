# exam-lookup-rs

把 [`x_videos-dowloads/exam-lookup`](../../x_videos-dowloads/exam-lookup) 的 Python 备考台(题库 + 知识库 + Web UI)用 Rust 重写,借鉴 [`newsnext/newsnow`](https://github.com/newsnext/newsnow) 的引擎设计模式。

> **状态**:Phase Q 完成 + **2026-11-04 审计修复轮**(9 项 P0/P1/P2 修复已落地)——
> **153 tests**(engine 130 / app 23 / cli 0),clippy 0 error,真实数据 parity 0.00%(1768 题)。
> 真实数据 E2E:22 banks / 1768 题 / 1278 cards / 0 errors(Python `json_banks` 侧 17 banks / 1768 题)。
> 详细计划见 `~/.claude/plans/temporal-churning-sonnet.md`,深度文档见 [`docs/`](docs/)。

---

## 架构

```
┌──────────────────────────────┐
│ x_videos-dowloads/exam-lookup│  ← 只读数据源(Python 项目)
│   banks/*.json               │
└────────────┬─────────────────┘
             │ Source trait (load)
┌────────────▼─────────────────┐
│ exam-engine (纯 lib)          │
│   inventory 自动注册 source   │
│   6 档 Interval 调度          │
│   SQLite FTS5 + trigram(下期) │
└────────────┬─────────────────┘
             │ HTTP / Desktop
┌────────────▼─────────────────┐
│ exam-app  (Dioxus 0.7 GUI)    │  ← Phase E
│ exam-cli (CLI 工具,已可用)   │
└──────────────────────────────┘
```

### Crate 列表

| Crate | 角色 | 当前状态 |
|---|---|---|
| `exam-engine` | 纯数据 lib:types / error / source 注册表 + JsonBanksSource / CrawlCardsSource | ✅ Phase Q |
| `exam-cli` | CLI:`sources` 列表 + `load` 加载题库摘要 | ✅ Phase Q |
| `exam-app` | Dioxus 0.7 desktop GUI + CardsPage 真实卡片数据 | ✅ Phase Q |

> **Dioxus 版本**:`crates/exam-app/Cargo.toml` 声明 `0.7.9`(**caret**,不是 `=0.7.9` 精确锁定),`Cargo.lock` 实解 **0.7.10**。详见 [R-004](docs/reports/R-004.md)。

---

## newsnow 引擎模式 → Rust 映射

| newsnow (TS) | Rust 实现 |
|---|---|
| `glob:./sources/{*.ts,**/index.ts}` 自动注册 | `inventory::submit!(SourceEntry { source: &INSTANCE })` |
| `SourceID` 模板字面量类型推导 | `BankId` / `QuestionId` newtype + `FromStr` |
| `Time.Test/Realtime/Fast/Default/Common/Slow` | `enum Interval` 6 档 |
| `defineSource()` 包装 | `pub trait Source { fn load(...) }` |
| `SourceResponse.status: "success" \| "cache"` | `enum CacheStatus { Fresh, Stale, Expired }`(Phase D) |

---

## 开发

```bash
# 编译 / 测试 / lint(全部 green)
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# CLI 用法
cargo run --bin exam-cli -- sources
# 已注册 source (2):
#   - json_banks           本地 JSON 题库           interval=slow (60min)
#   - crawl_cards          问答材料 JSON (crawl/问答*.json) interval=slow (60min)

cargo run --bin exam-cli -- --data-root /path/to/exam-lookup load --source json_banks
# json_banks: 加载 17 个 bank(来自 /path/to/exam-lookup)
```

---

## 数据契约(`banks/<name>.json`)

```json
{
  "meta": {
    "title": "示例题库",
    "external": false,
    "year": "2024",
    "sourceNote": "出处"
  },
  "questions": [
    {
      "id": "Q-001",
      "number": 1,
      "year": "2024",
      "type": "single",
      "typeLabel": "单选",
      "question": "题干",
      "options": [{"key":"A","text":"..."}],
      "answerKeys": ["B"],
      "answerText": ["2"],
      "analysis": "...",
      "topic": "...",
      "srcBank": "..."
    }
  ]
}
```

跳过:`answers_*.json`(答案 schema 不同)/ `selftest` / `*.json.bak`。

---

## 阶段

> 状态列的测试数 = **该阶段结束时的快照**;当前总数见文首状态行(153 tests)。
> Phase 编号沿用历史(`B` 编号空缺、`I+++` / `M+` 等带 `+` 后缀),**不重排** —— 重排会让各 R-xxx 报表里的阶段引用失效。

| Phase | 内容 | 状态(阶段结束时) |
|---|---|---|
| A | 数据契约 + Engine 骨架 | ✅ 22/22 tests,clippy clean |
| B | —— 编号空缺(从未实施)—— | — |
| C | 索引 + FTS5 + trigram | ✅ 50/50 tests,parity 0.00%(1760 题) |
| D | 间隔调度 + 缓存 | ✅ 70/70 tests,CLI `tick`/`cache`/`run` |
| E | Dioxus 0.7 GUI | ✅ 78/78 tests,macOS `.app` 启动 OK |
| F | 80% 覆盖率 + 报表归档 | ✅ 纯逻辑 ~85% 覆盖,R-001..R-009 + R-COVERAGE |
| G | 问答卡片 + CardsPage 真实数据 | ✅ 89/89 tests,R-002 关闭(部分) |
| H | SettingsPage 数据根 + Rebuild now + 真实数据 E2E | ✅ 89/89 tests,smoke PASS,R-011 |
| I | 性能基线(cargo bench + 5 项 median 数字) | ✅ 89/89 tests,R-PERF 阈值表 |
| I+ | SettingsPage 主题实时预览(select.onchange 即时 apply + `↻` cycle 按钮) | ✅ 89/89 tests,clippy clean,R-012 |
| I++ | SettingsPage 主题色板预览条(5 个 var(--*) 色块零事件跟随) | ✅ 89/89 tests,clippy clean,R-013 |
| I+++ | notify watcher 模块(engine 侧 primitive) | ✅ 89/89 tests |
| J | UX 打磨(DB 徽章 + 全局 ?/ 快捷键 + 主题同步 + 首启动 hint / loading-empty-error 三态) | ✅ 89/89 tests,R-010 关闭 |
| K | 文档(architecture / port-from-python / engine-design) | ✅ 3 篇落档 |
| L | audit round(3 reviewer 并行 → 12 fix steps) | ✅ 91 → 112 tests |
| M | watcher → AppState 接入(SettingsPage 自动重建 toggle) | ✅ 112 → 121 tests |
| M+ | `sync_watcher`:data_root 变化时强制重启 watcher | ✅ 121 → 123 tests |
| N | `Scheduler::tick` 失败 source 指数退避(防雪崩,R-007 配套) | ✅ 123 → 127 tests |
| O | watcher.rs `unused_mut` 清理(clippy --all-targets 0 error) | ✅ 127 tests |
| P | SettingsPage dioxus-ssr 结构化 render test(防 UI 回归) | ✅ 127 → 129 tests |
| Q | `dump_parity.sh` 实际生成 — 题目级 parity 0.00%(R-015) | ✅ 129 tests |
| **2026-11-04 审计修复轮** | P0-1 宽容反序列化 / P0-2 parity 分母 / P1-1 `q_fts` 扇出 / P1-2 LIKE ESCAPE / P1-3 watcher 代际隔离 / P1-4 重建移出 UI 线程 / P2-3 DB 迁移路径 / P2-7 cache status 读取时计算 | ✅ **153 tests**(engine 130 / app 23 / cli 0),clippy 0 error |

## 报表索引(`docs/reports/`)

> 状态列 = 该报表完成时的快照;当前总数见文首状态行。
> 报表 ID **无 R-014**(编号空缺,与 Phase B 同理)。

| ID | 议题 | 状态 |
|---|---|---|
| R-001 | 中文 trigram / 分词降级(CJK 加空格 + FTS5 unicode61) | ✅ Phase C |
| R-002 | PDF / 文档解析(问答卡片已落地,PDF/HTML 延后) | ✅ Phase G 关闭(部分) |
| R-003 | Dioxus desktop 首次 build 耗时 | ✅ 接受(增量 < 15s) |
| R-004 | Dioxus 版本锁定 | ⚠ **决策未落地** —— Cargo.toml 是 caret `0.7.9`,`Cargo.lock` 实解 **0.7.10**(详见报表) |
| R-005 | inventory 在 wasm 端(不启用,desktop only) | ✅ Phase F 范围内降级 |
| R-006 | localStorage → 文件持久化(`dirs::config_dir`) | ✅ Phase E 落地 |
| R-007 | 反爬 / 限流 / 网络 IO 失败(写 cache notes + 指数退避) | ✅ Phase D 落地,Phase N 补退避 |
| R-008 | 大文件 docs/ 索引启动慢(GUI 不阻塞) | ✅ 当前数据量不触发 |
| R-009 | 搜索排序与 Python `solve.py::lookup` 差异 | ⚠ 已知偏差,bm25 排序 |
| R-010 | Phase J UX 打磨(首启动 + 全局键 + 主题同步) | ✅ 89/89 tests,clippy clean |
| R-011 | Phase H Settings 数据根 + Rebuild now + 真实数据 E2E | ✅ 21 banks / 1760 题 / 1278 cards / 0 errors |
| R-012 | Phase I+ 主题实时预览(select onchange 即时 apply + cycle 按钮) | ✅ 89/89 tests,clippy clean |
| R-013 | Phase I++ 主题色板预览条(5 色块零事件跟随根 data-theme) | ✅ 89/89 tests,clippy clean |
| R-015 | `dump_parity.sh` 题目级 parity(Python vs Rust dump) | ✅ 0/1768 不一致(0.00%),门禁能变红 |
| R-016 | 全量代码审计(2026-11-04,P0×2 / P1×4 / P2×11 + 环境 1) | ✅ 10 项建议修复全部落地 → 153 tests |
| R-COVERAGE | Phase F 80% 覆盖率验收 | ✅ 纯逻辑 ~85%(Phase F 快照) |
| R-PERF | Phase I 性能基线 + 容忍上限 | ✅ build_index ≤ 35ms / search ≤ 2.5ms |
| R-FINAL | v1.0 封盘报告(2026-10-04) | ✅ 封盘,后续见 HANDOFF |
