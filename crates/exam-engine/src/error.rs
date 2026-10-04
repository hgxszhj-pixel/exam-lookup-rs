//! 统一错误类型。所有 exam-engine 内部错误用 `EngineError`,
//! 顶层入口可用 `anyhow::Error` 包装。

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    /// 文件 IO 错误(读题库 JSON、写 SQLite cache 等)
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// JSON 解析错误
    #[error("json parse error at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// 数据 schema 校验失败
    #[error("schema invalid at {path}: {reason}")]
    Schema { path: PathBuf, reason: String },

    /// 注册表查询失败
    #[error("source not registered: {0}")]
    SourceNotFound(String),

    /// 间隔调度 / 缓存 / 索引的更广义错误
    #[error("engine error: {0}")]
    Other(String),

    /// 文件系统 watcher 启动失败(路径非目录 / OS 资源耗尽 / 等)
    #[error("watcher init failed: {0}")]
    WatcherInit(String),
}

impl EngineError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.into(),
            source,
        }
    }

    pub fn schema(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::Schema {
            path: path.into(),
            reason: reason.into(),
        }
    }

    /// 通用的「其他引擎错误」构造器 — 主要给 SQLite / FTS5 等 wrap 用。
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;

// `anyhow::Error` 已经 blanket impl<E: StdError> From<E>,所以无需手写 impl。
// 注意:不要手写 `impl From<EngineError> for anyhow::Error { anyhow::anyhow!("{}", e) }`,
// 那种写法会 strip 掉 typed error,导致 downcast_ref::<EngineError>() 永远返回 None。