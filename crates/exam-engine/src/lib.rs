//! exam-engine:题库 / 问答材料 数据引擎。
//!
//! 借鉴 `newsnext/newsnow` 的核心模式:
//! - **glob 自动注册**:`source/*.rs` 通过 `inventory` crate 自动注册到全局注册表
//! - **类型即注册表**:`BankId` / `QuestionId` newtype 编译期保证 ID 不混
//! - **间隔分档调度**:`Interval` 6 档(对应 newsnow `Time.{Test, Realtime, Fast, Default, Common, Slow}`)
//! - **cache-first**:`CacheStatus { Fresh, Stale, Expired }`(对应 newsnow `SourceResponse.status`)
//!
//! 数据契约:
//! - 输入:题库 JSON + 答案 JSON + 问答材料 JSON(原 Python `exam-lookup` 项目)
//! - 输出:`Vec<Question>` / `Vec<CardItem>` / SQLite 索引(后续 Phase)

pub mod cache;
pub mod error;
pub mod index;
pub mod parse;
pub mod scheduler;
pub mod search;
pub mod types;
pub mod watcher;
pub mod source;

pub use cache::{CacheEntry, CacheStatus, CacheStore, compute_status, now_secs, should_refresh};
pub use error::{EngineError, Result};
pub use index::{BuildStats, IndexBuilder, build_with_source, open_readonly};
pub use scheduler::{Scheduler, SchedulerConfig, TickOutcome};
pub use search::{CardHit, MatchKind, SearchHit, SearchQuery, search, search_cards};
pub use source::{Interval, LoadResult, Source, all_sources, find_source};
pub use types::{
    BankId, BankMeta, BankOption, CardItem, Question, QuestionId, QuestionType,
};

// 显式暴露内置 source
pub use source::json_banks::JsonBanksSource;
pub use watcher::{DataRootWatcher, watch};