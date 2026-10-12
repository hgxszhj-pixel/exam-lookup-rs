//! Source 注册表:借鉴 newsnow 的 `glob:./sources/{*.ts,**/index.ts}` 自动注册模式。
//!
//! Rust 实现:每个 `source/*.rs` 用 `inventory::submit!` 注册一个 `&'static dyn Source`,
//! 通过 `all_sources()` 一行拿到所有注册项。
//!
//! 关键不变量:
//! - `Source` trait 必须对象安全(`dyn Source` 可用)
//! - 每个 source 必须 `Send + Sync + 'static`,允许跨线程 / 异步使用
//! - `Interval` 6 档对齐 newsnow:Test / Realtime / Fast / Default / Common / Slow

use crate::error::EngineError;
use crate::types::{BankId, BankMeta, CardItem, Question};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub mod crawl_cards;
pub mod json_banks;

/// 间隔分档调度:对应 newsnow `shared/consts.ts` 的 `Time.{Test, Realtime, Fast, Default, Common, Slow}`。
///
/// 默认 Refresh 间隔:
/// - `Test`:1s(单元测试用)
/// - `Realtime`:2min(实时/微博)
/// - `Fast`:5min(快讯)
/// - `Default`:10min(默认)
/// - `Common`:30min(慢更新)
/// - `Slow`:60min(离线 cache)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interval {
    Test,
    Realtime,
    Fast,
    Default,
    Common,
    Slow,
}

impl Interval {
    pub const fn as_secs(self) -> u64 {
        match self {
            Self::Test => 1,
            Self::Realtime => 120,
            Self::Fast => 300,
            Self::Default => 600,
            Self::Common => 1800,
            Self::Slow => 3600,
        }
    }

    pub const fn as_duration(self) -> Duration {
        Duration::from_secs(self.as_secs())
    }
}

impl std::fmt::Display for Interval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Test => "test (1s)",
            Self::Realtime => "realtime (2min)",
            Self::Fast => "fast (5min)",
            Self::Default => "default (10min)",
            Self::Common => "common (30min)",
            Self::Slow => "slow (60min)",
        };
        f.write_str(s)
    }
}

/// 一个 source 的加载结果:一个 source 可能产出多个 bank(对应 newsnow 的 `defineSource` 可返回 array)。
///
/// Phase G 扩展:同一 source 也可产出「问答材料卡片」(`cards`),用于 cards 页 / doc_index。
/// 题库 source 默认 `cards = vec![]`,问答 source 默认 `questions = vec![]`。
#[derive(Debug)]
pub struct LoadResult {
    pub bank_id: BankId,
    pub meta: Option<BankMeta>,
    pub questions: Vec<Question>,
    pub cards: Vec<CardItem>,
}

/// Source trait:每个 source 实现 `load()` 从 `data_root` 加载自身数据。
///
/// Phase A 不涉及远程 fetch;后续 Phase 可加 `async fn fetch()` 走 HTTP。
pub trait Source: Sync + Send + 'static {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn interval(&self) -> Interval;

    /// 从 data_root 加载该 source 的全部数据。
    /// - `data_root`:指向 `exam-lookup` 项目根(包含 `banks/`、`crawl/`、`docs/` 等子目录)
    fn load(&self, data_root: &Path) -> Result<Vec<LoadResult>, EngineError>;
}

// ================ inventory 注册表 ================

inventory::collect!(SourceEntry);

/// 注册表条目:包装 `&'static dyn Source`。
///
/// `&'static dyn Trait` 是 Sized + 'static,符合 inventory 要求。
pub struct SourceEntry {
    pub source: &'static dyn Source,
}

// 在每个 source 文件中直接使用:
// ```
// static FOO_INSTANCE: FooSource = FooSource;
// inventory::submit!(crate::source::SourceEntry { source: &FOO_INSTANCE });
// ```
// inventory::submit! 必须是 const,不能塞到普通 fn 里。

/// 列出所有注册的 source。
pub fn all_sources() -> Vec<&'static dyn Source> {
    inventory::iter::<SourceEntry>().map(|e| e.source).collect()
}

/// 按 id 查找 source。
pub fn find_source(id: &str) -> Option<&'static dyn Source> {
    all_sources().into_iter().find(|s| s.id() == id)
}

/// 单 JSON 文件 size 上限(64 MiB)。
///
/// 防御:4 GiB banks file → 进程 OOM(Step 1.3,2026-10-12)。
/// 当前 21 banks 总量 16 MB,64 MiB 留 4× 余量,正常题库不会触顶。
pub const MAX_JSON_BYTES: u64 = 64 * 1024 * 1024;

/// 读 JSON 文件 + 提前 size check + 包装 IO 错误。
///
/// 先 `fs::metadata().len()` 拒绝超 cap,避免 `fs::read` 一次性分配过大 buffer。
/// 写文件 / 已存在但 < cap 的常规路径走 `fs::read`,不重复 stat。
pub fn load_json_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>, EngineError> {
    let meta = std::fs::metadata(path).map_err(|e| EngineError::io(path, e))?;
    if meta.len() > max_bytes {
        return Err(EngineError::schema(
            path,
            format!(
                "JSON 文件过大: {} 字节,上限 {} 字节(MAX_JSON_BYTES)",
                meta.len(),
                max_bytes
            ),
        ));
    }
    std::fs::read(path).map_err(|e| EngineError::io(path, e))
}

/// canonicalize 一个用户提供的路径,失败包装成 `EngineError::io`。
///
/// Step 1.4 (2026-10-12):防止 symlink 漂移 + 路径不一致(CLI `--data-root`
/// 与 settings.json 里写的是不同形式时,下游 cache key 错配)。
///
/// `std::fs::canonicalize` 在 macOS 已解析 `/var` ↔ `/private/var`,
/// 不需要额外的 dunce 包装(那是反向 un-canonicalize 用)。
pub fn canonicalize_path(path: &Path) -> Result<PathBuf, EngineError> {
    std::fs::canonicalize(path).map_err(|e| EngineError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_durations() {
        assert_eq!(Interval::Test.as_secs(), 1);
        assert_eq!(Interval::Realtime.as_secs(), 120);
        assert_eq!(Interval::Fast.as_secs(), 300);
        assert_eq!(Interval::Default.as_secs(), 600);
        assert_eq!(Interval::Common.as_secs(), 1800);
        assert_eq!(Interval::Slow.as_secs(), 3600);
    }

    #[test]
    fn interval_display() {
        assert_eq!(format!("{}", Interval::Realtime), "realtime (2min)");
        assert_eq!(format!("{}", Interval::Slow), "slow (60min)");
    }

    #[test]
    fn interval_as_duration_matches_secs() {
        for interval in [
            Interval::Test,
            Interval::Realtime,
            Interval::Fast,
            Interval::Default,
            Interval::Common,
            Interval::Slow,
        ] {
            assert_eq!(interval.as_duration().as_secs(), interval.as_secs());
        }
    }

    #[test]
    fn inventory_registration_via_macro() {
        // 测试 inventory 注册流程本身:写一个 mock source 并注册
        // 注意:必须 'static,无副作用
        struct MockSource;
        impl Source for MockSource {
            fn id(&self) -> &'static str {
                "mock"
            }
            fn display_name(&self) -> &'static str {
                "Mock"
            }
            fn interval(&self) -> Interval {
                Interval::Test
            }
            fn load(&self, _: &Path) -> Result<Vec<LoadResult>, EngineError> {
                Ok(vec![])
            }
        }
        static MOCK_INSTANCE: MockSource = MockSource;
        inventory::submit!(SourceEntry {
            source: &MOCK_INSTANCE
        });

        let all = all_sources();
        assert!(all.iter().any(|s| s.id() == "mock"));
        assert!(find_source("mock").is_some());
        assert!(find_source("nonexistent").is_none());
    }

    /// Step 1.3: 小于 cap 的文件正常读出
    #[test]
    fn load_json_bounded_reads_small_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("small.json");
        std::fs::write(&p, br#"{"hello":"world"}"#).expect("write");
        let bytes = load_json_bounded(&p, MAX_JSON_BYTES).expect("read small");
        assert_eq!(bytes, br#"{"hello":"world"}"#);
    }

    /// Step 1.3:超过 cap 返回 Schema 错误,且不读文件
    /// (用 `seek` 写到超 cap 大小 — 不真分配 64+ MiB heap,只在 metadata 里骗 stat)
    #[test]
    fn load_json_bounded_rejects_oversize() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("huge.json");
        // 写 16 字节,但声明 cap 8 字节 — 应该被拒
        std::fs::write(&p, b"0123456789ABCDEF").expect("write");
        let err = load_json_bounded(&p, 8).expect_err("must error on oversize");
        assert!(
            matches!(err, EngineError::Schema { .. }),
            "want Schema error, got: {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("JSON 文件过大"), "msg: {msg}");
        assert!(msg.contains('8'), "msg should mention cap: {msg}");
    }

    /// Step 1.3: 等于 cap 正好通过(metadata.len() > cap 才是超)
    #[test]
    fn load_json_bounded_accepts_exact_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("exact.json");
        std::fs::write(&p, b"12345678").expect("write 8 bytes");
        let bytes = load_json_bounded(&p, 8).expect("read at cap");
        assert_eq!(bytes.len(), 8);
    }

    /// Step 1.4: 相对路径 canonicalize 成绝对路径
    #[test]
    fn canonicalize_path_resolves_relative() {
        let dir = tempfile::tempdir().expect("tempdir");
        let canon = canonicalize_path(dir.path()).expect("canon");
        assert!(canon.is_absolute(), "expected absolute, got: {canon:?}");
        assert_eq!(canon, std::fs::canonicalize(dir.path()).unwrap());
    }

    /// Step 1.4: 不存在路径 canonicalize 失败,包成 Io error
    #[test]
    fn canonicalize_path_rejects_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("nope");
        let err = canonicalize_path(&p).expect_err("must error");
        assert!(matches!(err, EngineError::Io { .. }), "got: {err:?}");
    }
}