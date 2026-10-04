//! 三态缓存层:对齐 newsnow `SourceResponse.status: "success" \| "cache"`。
//!
//! 核心概念:
//! - `CacheStatus`: `Fresh` (ttl 内) / `Stale` (1-2×ttl 内,可用但提示刷新) / `Expired` (>2×ttl 或缺)
//! - `CacheEntry`:单 source 的缓存元数据 + 摘要统计(banks / questions / 备注)
//! - `CacheStore`:SQLite-backed 持久化(独立 cache.db,跟 questions.db 解耦)
//!
//! 纯函数:
//! - `should_refresh()`:决定 scheduler 是否需要再次 load
//! - `compute_status()`:根据 age 推导 status
//!
//! 副作用只在 `CacheStore`;`Scheduler` 通过 `should_refresh` 读 cache 决定时机,
//! 写回用 `CacheStore::upsert`。

use crate::error::{EngineError, Result};
use crate::source::Interval;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// 缓存三态(对应 newsnow `SourceResponse.status`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    /// 在 ttl 内,可直接用。
    Fresh,
    /// 1-2×ttl,仍可用但建议刷新(避免下一周期到 Expired)。
    Stale,
    /// >2×ttl 或缺失,必须重新加载。
    Expired,
}

impl CacheStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Expired => "expired",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "fresh" => Ok(Self::Fresh),
            "stale" => Ok(Self::Stale),
            "expired" => Ok(Self::Expired),
            _ => Err(EngineError::schema(
                "<cache status>",
                format!("unknown cache status: {s}"),
            )),
        }
    }
}

impl std::fmt::Display for CacheStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 单 source 的缓存条目。
///
/// 注意:`banks` / `questions` 是上一轮 load 的摘要,用于 CLI 快速打印与
/// scheduler 决定是否 stale 升级;不存原始 payload(避免 cache.db 膨胀)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    pub source_id: String,
    pub fetched_at: i64,
    pub ttl_secs: u64,
    pub status: CacheStatus,
    pub banks: usize,
    pub questions: usize,
    pub notes: Option<String>,
}

impl CacheEntry {
    pub fn age_secs(&self, now: i64) -> i64 {
        (now - self.fetched_at).max(0)
    }
}

/// 当前 Unix 秒数。
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 决定 scheduler 是否要再次 load 该 source。
///
/// 规则:
/// - 无 entry → true(首次必须加载)
/// - `status == Expired` → true
/// - `age >= ttl_secs` → true(到点了)
/// - 否则 false
pub fn should_refresh(entry: Option<&CacheEntry>, _interval: Interval, now: i64) -> bool {
    let Some(e) = entry else { return true };
    if e.status == CacheStatus::Expired {
        return true;
    }
    e.age_secs(now) >= e.ttl_secs as i64
}

/// 根据 age 推导当前 status(纯函数)。
///
/// - `age < ttl` → Fresh
/// - `ttl ≤ age < 2×ttl` → Stale
/// - `age ≥ 2×ttl` → Expired
pub fn compute_status(fetched_at: i64, ttl_secs: u64, now: i64) -> CacheStatus {
    let age = (now - fetched_at).max(0) as u64;
    if age < ttl_secs {
        CacheStatus::Fresh
    } else if age < ttl_secs.saturating_mul(2) {
        CacheStatus::Stale
    } else {
        CacheStatus::Expired
    }
}

// ================ SQLite 持久化 ================

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS cache_meta (
    source_id   TEXT PRIMARY KEY,
    fetched_at  INTEGER NOT NULL,
    ttl_secs    INTEGER NOT NULL,
    status      TEXT NOT NULL,
    banks       INTEGER NOT NULL,
    questions   INTEGER NOT NULL,
    notes       TEXT
);
CREATE INDEX IF NOT EXISTS idx_cache_meta_status ON cache_meta(status);
";

/// SQLite-backed cache store(独立 cache.db)。
pub struct CacheStore {
    conn: Connection,
}

// SAFETY: rusqlite 的 `Connection` 内部使用 SQLite 自带的 mutex 保证线程安全,
//但 `Connection` 类型本身只 impl `Send` 不 impl `Sync`(rusqlite 的保守做法)。
//我们在 exam-engine 里只在单一线程持有 `CacheStore`(Scheduler tick 是单线程循环),
//不需要跨线程共享,这里给 `Sync` 是为了让 `Arc<CacheStore>` 编译通过。
//若后续要把 Scheduler 改成多线程并行,需要改成 `Arc<Mutex<CacheStore>>`。
unsafe impl Sync for CacheStore {}

impl CacheStore {
    /// 打开或创建 cache.db,自动 init schema。
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| EngineError::schema(path, e.to_string()))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| EngineError::schema(path, e.to_string()))?;
        Ok(Self { conn })
    }

    /// 内存 cache(测试用)。
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|e| {
            EngineError::schema("<in-memory cache>", e.to_string())
        })?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| EngineError::schema("<in-memory schema>", e.to_string()))?;
        Ok(Self { conn })
    }

    pub fn upsert(&self, entry: &CacheEntry) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO cache_meta(source_id, fetched_at, ttl_secs, status, banks, questions, notes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(source_id) DO UPDATE SET
                    fetched_at = excluded.fetched_at,
                    ttl_secs = excluded.ttl_secs,
                    status = excluded.status,
                    banks = excluded.banks,
                    questions = excluded.questions,
                    notes = excluded.notes",
                params![
                    entry.source_id,
                    entry.fetched_at,
                    entry.ttl_secs as i64,
                    entry.status.as_str(),
                    entry.banks as i64,
                    entry.questions as i64,
                    entry.notes.as_deref(),
                ],
            )
            .map_err(|e| EngineError::schema("<cache upsert>", e.to_string()))?;
        Ok(())
    }

    pub fn get(&self, source_id: &str) -> Result<Option<CacheEntry>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT source_id, fetched_at, ttl_secs, status, banks, questions, notes
                 FROM cache_meta WHERE source_id = ?1",
            )
            .map_err(|e| EngineError::schema("<cache get>", e.to_string()))?;
        let row = stmt
            .query_row(params![source_id], |r| {
                let status_s: String = r.get(3)?;
                Ok(CacheEntry {
                    source_id: r.get(0)?,
                    fetched_at: r.get(1)?,
                    ttl_secs: r.get::<_, i64>(2)? as u64,
                    status: CacheStatus::parse(&status_s).ok().unwrap_or(CacheStatus::Expired),
                    banks: r.get::<_, i64>(4)? as usize,
                    questions: r.get::<_, i64>(5)? as usize,
                    notes: r.get(6)?,
                })
            })
            .optional()
            .map_err(|e| EngineError::schema("<cache get>", e.to_string()))?;
        Ok(row)
    }

    pub fn list(&self) -> Result<Vec<CacheEntry>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT source_id, fetched_at, ttl_secs, status, banks, questions, notes
                 FROM cache_meta ORDER BY source_id",
            )
            .map_err(|e| EngineError::schema("<cache list>", e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                let status_s: String = r.get(3)?;
                Ok(CacheEntry {
                    source_id: r.get(0)?,
                    fetched_at: r.get(1)?,
                    ttl_secs: r.get::<_, i64>(2)? as u64,
                    status: CacheStatus::parse(&status_s).ok().unwrap_or(CacheStatus::Expired),
                    banks: r.get::<_, i64>(4)? as usize,
                    questions: r.get::<_, i64>(5)? as usize,
                    notes: r.get(6)?,
                })
            })
            .map_err(|e| EngineError::schema("<cache list>", e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| EngineError::schema("<cache list>", e.to_string()))?);
        }
        Ok(out)
    }

    pub fn delete(&self, source_id: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM cache_meta WHERE source_id = ?1",
                params![source_id],
            )
            .map_err(|e| EngineError::schema("<cache delete>", e.to_string()))?;
        Ok(())
    }

    /// 测试/管理用:清空所有条目。
    pub fn clear(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM cache_meta", [])
            .map_err(|e| EngineError::schema("<cache clear>", e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_status_as_str_and_parse_roundtrip() {
        for s in [CacheStatus::Fresh, CacheStatus::Stale, CacheStatus::Expired] {
            assert_eq!(CacheStatus::parse(s.as_str()).unwrap(), s);
        }
        assert!(CacheStatus::parse("bogus").is_err());
    }

    #[test]
    fn compute_status_three_bands() {
        // ttl = 60
        // age < 60     → Fresh
        // 60..119      → Stale
        // ≥ 120        → Expired
        assert_eq!(compute_status(0, 60, 30), CacheStatus::Fresh);
        assert_eq!(compute_status(0, 60, 60), CacheStatus::Stale);
        assert_eq!(compute_status(0, 60, 119), CacheStatus::Stale);
        assert_eq!(compute_status(0, 60, 120), CacheStatus::Expired);
        assert_eq!(compute_status(0, 60, 9999), CacheStatus::Expired);
    }

    #[test]
    fn compute_status_clamps_negative_age() {
        // now < fetched_at(clock skew)→ 当作 age=0,不 Expired
        assert_eq!(compute_status(100, 60, 50), CacheStatus::Fresh);
    }

    #[test]
    fn should_refresh_first_run_is_due() {
        assert!(should_refresh(None, Interval::Slow, 1000));
    }

    #[test]
    fn should_refresh_expired_status_is_due() {
        let e = CacheEntry {
            source_id: "x".into(),
            fetched_at: 0,
            ttl_secs: 60,
            status: CacheStatus::Expired,
            banks: 0,
            questions: 0,
            notes: None,
        };
        assert!(should_refresh(Some(&e), Interval::Slow, 10));
    }

    #[test]
    fn should_refresh_within_ttl_not_due() {
        let e = CacheEntry {
            source_id: "x".into(),
            fetched_at: 0,
            ttl_secs: 60,
            status: CacheStatus::Fresh,
            banks: 0,
            questions: 0,
            notes: None,
        };
        assert!(!should_refresh(Some(&e), Interval::Slow, 30));
    }

    #[test]
    fn should_refresh_at_ttl_boundary_due() {
        let e = CacheEntry {
            source_id: "x".into(),
            fetched_at: 0,
            ttl_secs: 60,
            status: CacheStatus::Stale,
            banks: 0,
            questions: 0,
            notes: None,
        };
        assert!(should_refresh(Some(&e), Interval::Slow, 60));
        assert!(should_refresh(Some(&e), Interval::Slow, 61));
    }

    #[test]
    fn cache_store_roundtrip_in_memory() {
        let store = CacheStore::open_in_memory().unwrap();
        let entry = CacheEntry {
            source_id: "json_banks".into(),
            fetched_at: 1000,
            ttl_secs: 3600,
            status: CacheStatus::Fresh,
            banks: 16,
            questions: 1760,
            notes: Some("unit-test".into()),
        };
        store.upsert(&entry).unwrap();
        let got = store.get("json_banks").unwrap().unwrap();
        assert_eq!(got, entry);

        // 更新覆盖
        let mut updated = entry.clone();
        updated.fetched_at = 2000;
        updated.status = CacheStatus::Stale;
        updated.questions = 1761;
        store.upsert(&updated).unwrap();
        assert_eq!(store.get("json_banks").unwrap().unwrap(), updated);

        // list 排序
        store.upsert(&CacheEntry {
            source_id: "other".into(),
            fetched_at: 500,
            ttl_secs: 60,
            status: CacheStatus::Expired,
            banks: 1,
            questions: 5,
            notes: None,
        }).unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].source_id, "json_banks");
        assert_eq!(list[1].source_id, "other");
    }

    #[test]
    fn cache_store_persists_to_disk() {
        let tmp = std::env::temp_dir().join(format!(
            "exam-cache-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);

        let store = CacheStore::open(&tmp).unwrap();
        store
            .upsert(&CacheEntry {
                source_id: "json_banks".into(),
                fetched_at: 100,
                ttl_secs: 60,
                status: CacheStatus::Stale,
                banks: 16,
                questions: 1760,
                notes: None,
            })
            .unwrap();
        drop(store);

        let reopened = CacheStore::open(&tmp).unwrap();
        let got = reopened.get("json_banks").unwrap().unwrap();
        assert_eq!(got.fetched_at, 100);
        assert_eq!(got.banks, 16);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn cache_store_delete_and_clear() {
        let store = CacheStore::open_in_memory().unwrap();
        store.upsert(&CacheEntry {
            source_id: "a".into(),
            fetched_at: 0,
            ttl_secs: 60,
            status: CacheStatus::Fresh,
            banks: 0,
            questions: 0,
            notes: None,
        }).unwrap();
        store.upsert(&CacheEntry {
            source_id: "b".into(),
            fetched_at: 0,
            ttl_secs: 60,
            status: CacheStatus::Fresh,
            banks: 0,
            questions: 0,
            notes: None,
        }).unwrap();

        store.delete("a").unwrap();
        assert!(store.get("a").unwrap().is_none());
        assert!(store.get("b").unwrap().is_some());

        store.clear().unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn cache_store_get_missing_returns_none() {
        let store = CacheStore::open_in_memory().unwrap();
        assert!(store.get("nonexistent").unwrap().is_none());
    }

    #[test]
    fn cache_entry_age_clamps_negative() {
        let e = CacheEntry {
            source_id: "x".into(),
            fetched_at: 100,
            ttl_secs: 60,
            status: CacheStatus::Fresh,
            banks: 0,
            questions: 0,
            notes: None,
        };
        assert_eq!(e.age_secs(50), 0);
        assert_eq!(e.age_secs(200), 100);
    }
}
