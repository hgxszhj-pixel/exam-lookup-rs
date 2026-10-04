# exam-lookup-rs

把 [`x_videos-dowloads/exam-lookup`](../x_videos-dowloads/exam-lookup) 的 Python 备考台(题库 + 知识库 + Web UI)用 Rust 重写,借鉴 [`newsnext/newsnow`](https://github.com/newsnext/newsnow) 的引擎设计模式。

> **状态**:Phase I++ 完成(SettingsPage 主题色板预览条 — 5 个 var(--*) 色块零事件跟随根 data-theme 自动重渲)。
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
│ exam-app  (Dioxus 0.7.9 GUI)  │  ← Phase E
│ exam-cli (CLI 工具,已可用)   │
└──────────────────────────────┘
```

### Crate 列表

| Crate | 角色 | 当前状态 |
|---|---|---|
| `exam-engine` | 纯数据 lib:types / error / source 注册表 + JsonBanksSource / CrawlCardsSource | ✅ Phase G |
| `exam-cli` | CLI:`sources` 列表 + `load` 加载题库摘要 | ✅ Phase A |
| `exam-app` | Dioxus 0.7.9 desktop GUI + CardsPage 真实卡片数据 | ✅ Phase G |

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
cargo clippy --workspace -- -D warnings

# CLI 用法
cargo run --bin exam-cli -- sources
# 已注册 source (1):
#   - json_banks           本地 JSON 题库           interval=slow (60min)

cargo run --bin exam-cli -- --data-root /path/to/exam-lookup load --source json_banks
# json_banks: 加载 N 个 bank(来自 /path/to/exam-lookup)
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

| Phase | 内容 | 状态 |
|---|---|---|
| A | 数据契约 + Engine 骨架 | ✅ 22/22 tests,clippy clean |
| C | 索引 + FTS5 + trigram | ✅ 50/50 tests,parity 0.00%(1760 题) |
| D | 间隔调度 + 缓存 | ✅ 70/70 tests,CLI `tick`/`cache`/`run` |
| E | Dioxus 0.7.9 GUI | ✅ 78/78 tests,macOS `.app` 启动 OK |
| F | 80% 覆盖率 + 报表归档 | ✅ 纯逻辑 ~85% 覆盖,R-001..R-009 + R-COVERAGE |
| G | 问答卡片 + CardsPage 真实数据 | ✅ 89/89 tests,R-002 关闭(部分) |
| I | 性能基线(cargo bench + 5 项 median 数字) | ✅ 89/89 tests,R-PERF 阈值表 |
| J | UX 打磨(loading / empty / error 三态 + 快捷键 + 主题) | ✅ 89/89 tests,R-010 关闭 |
| K | 文档(architecture / port-from-python / engine-design) | ✅ 3 篇落档 |
| J | UX 打磨(DB 徽章 / 全局 ?/ / 主题同步 / 首启动 hint) | ✅ 89/89 tests,R-010 |
| H | SettingsPage 数据根 + Rebuild now + 真实数据 E2E | ✅ 89/89 tests,smoke PASS,R-011 |
| I+ | SettingsPage 主题实时预览(select.onchange 即时 apply + `↻` cycle 按钮) | ✅ 89/89 tests,clippy clean,R-012 |
| I++ | SettingsPage 主题色板预览条(5 个 var(--*) 色块零事件跟随) | ✅ 89/89 tests,clippy clean,R-013 |

## 报表索引(`docs/reports/`)

| ID | 议题 | 状态 |
|---|---|---|
| R-001 | 中文 trigram / 分词降级(CJK 加空格 + FTS5 unicode61) | ✅ Phase C |
| R-002 | PDF / 文档解析(问答卡片已落地,PDF/HTML 延后) | ✅ Phase G 关闭(部分) |
| R-003 | Dioxus desktop 首次 build 耗时 | ✅ 接受(增量 < 15s) |
| R-004 | Dioxus 0.7.9 版本锁定(`=0.7.9`) | ✅ Cargo.toml 锁版本 |
| R-005 | inventory 在 wasm 端(不启用,desktop only) | ✅ Phase F 范围内降级 |
| R-006 | localStorage → 文件持久化(`dirs::config_dir`) | ✅ Phase E 落地 |
| R-007 | 反爬 / 限流 / 网络 IO 失败(写 cache notes,不重试) | ✅ Phase D 落地 |
| R-008 | 大文件 docs/ 索引启动慢(GUI 不阻塞) | ✅ 当前数据量不触发 |
| R-009 | 搜索排序与 Python `solve.py::lookup` 差异 | ⚠ 已知偏差,bm25 排序 |
| R-COVERAGE | Phase F 80% 覆盖率验收 | ✅ 纯逻辑 ~85% |
| R-PERF | Phase I 性能基线 + 容忍上限 | ✅ build_index ≤ 35ms / search ≤ 2.5ms |
| R-010 | Phase J UX 打磨(首启动 + 全局键 + 主题同步) | ✅ 89/89 tests,clippy clean |
| R-011 | Phase H Settings 数据根 + Rebuild now + 真实数据 E2E | ✅ 21 banks / 1760 题 / 1278 cards / 0 errors |
| R-012 | Phase I+ 主题实时预览(select onchange 即时 apply + cycle 按钮) | ✅ 89/89 tests,clippy clean |
| R-013 | Phase I++ 主题色板预览条(5 色块零事件跟随根 data-theme) | ✅ 89/89 tests,clippy clean |
