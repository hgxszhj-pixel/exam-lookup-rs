# newsnow → Rust 引擎模式适配

> 借鉴 [`newsnext/newsnow`](https://github.com/newsnext/newsnow) 的设计:Rust 工程化的最小落地。

## 关键映射

| newsnow (TS) | Rust 实现 | 机制 |
|--------------|-----------|------|
| `glob:./sources/{*.ts,**/index.ts}` | `inventory::submit!()` 每 source 文件一行 | `inventory = "0.3"` |
| `SourceID` 模板字面量 | `BankId(String)` / `QuestionId(String)` newtype + `FromStr` | std + 类型系统 |
| `Time.Test/Realtime/Fast/Default/Common/Slow` | `enum Interval { Test, Realtime(Duration), Fast, Default, Common, Slow }` | std |
| `defineSource()` 包装 | `pub trait Source { fn load(...) }` | trait,无 async-trait |
| `SourceResponse.status: success/cache` | `enum CacheStatus { Fresh, Stale, Expired }` | `serde` |
| `Cache.cache` 持久化 | `cache.rs` + SQLite | `rusqlite` |
| 调度循环 (`runOnce` / `loop`) | `scheduler.rs::run_loop` async | `tokio` |
| 静态 `sources.json` 驱动 UI + 抓取 | `inventory::iter::<&dyn Source>()` 同时供 CLI/GUI/scheduler | `inventory` |

## 设计取舍

### 1. `inventory` vs `linkme` vs 手写

| 选项 | 决策 |
|------|------|
| `inventory` | ✅ 选用,链接时静态注册,运行时零开销 |
| `linkme` | 拒绝,Rust 1.80 与 linkme 0.3 兼容性问题 |
| 手写 `OnceCell<Vec<...>>` | 拒绝,失去"加文件自动生效"的便利 |

**约束** (R-005):inventory **不**支持 wasm,desktop only。

### 2. `async-trait` vs native AFIT

`async fn in trait` 在 Rust 1.75+ stable 已可用,选 native:

```rust
pub trait Source: Sync {
    fn load(&self, data_root: &Path) -> Result<Vec<LoadResult>>;
}
```

`Source::load` 是**同步**的 — 因为 `exam-engine` 不发网络,只读本地 JSON,无 IO 阻塞。`fetch.rs` 留给后续远程源(R-007 暂不启用)。

### 3. FTS5 + trigram vs Tantivy

| 选项 | 决策 |
|------|------|
| SQLite FTS5 (unicode61 + 自加空格 CJK) | ✅ 选用,零依赖,bundle 内置 |
| Tantivy | 拒绝,体积 +50 MB,desktop GUI 不接受 |
| MeiliSearch | 拒绝,需要独立进程 |

trigram 中文分词(R-001):Rust 无现成 crate,降级方案是 "CJK 字符间加空格 + FTS5 unicode61 + LIKE OR-join 兜底"。bm25 排序与 Python `kb.py` 的 LIKE 排序有偏差(R-009)。

### 4. `thiserror` + `anyhow` vs 仅 `anyhow`

- **库边界** = `thiserror` 枚举(`crate::error::EngineError`)
- **应用层** = `anyhow::Result` 转接
- 公开 API `From<EngineError> for anyhow::Error` 留类型;**禁止** `impl From<X> for anyhow::Error { anyhow::anyhow!(...) }` strip typed(踩坑记录)

## 当前注册表状态

```bash
$ cargo run --bin exam-cli -- sources
已注册 source (1):
  - json_banks           本地 JSON 题库           interval=slow (60min)
```

第二 source `crawl_cards` (问答 split) 已落 `source/crawl_cards.rs`,Phase G 启用。Phase 后可继续扩 `doc_index` (PDF/HTML,R-002 延后)。

## 引擎不变式

1. **`Source::load` 不可副作用**:写 SQLite 是 caller 责任(目前 `IndexBuilder::rebuild` 调),不污染 source 层
2. **inventory 注册 0 成本**:链接期完成,运行时只读
3. **CacheStatus::Expired 才触发 `fetch`**:Fresh/Stale 直接返回,scheduler 不打网络
4. **Scheduler 单源独立 panic**:`tokio::spawn` + `JoinHandle` 隔离,一源挂不影响他源

## 增量扩展示例

加一个新 source (假设 `doc_index`):

```rust
// crates/exam-engine/src/source/doc_index.rs
pub struct DocIndexSource;
impl Source for DocIndexSource {
    fn id(&self) -> &'static str { "doc_index" }
    fn interval(&self) -> Interval { Interval::Slow }
    fn load(&self, data_root: &Path) -> Result<Vec<LoadResult>> {
        // 读 docs/*.pdf, *.html
        // 产出 LoadResult { questions: vec![], cards: vec![], docs: vec![...] }
    }
}
inventory::submit!(SourceEntry { source: &DocIndexSource as &dyn Source });
```

无任何注册表代码改动 — `inventory::iter` 自动发现。CLI / GUI / scheduler 同步可见。