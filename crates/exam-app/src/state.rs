//! 全局状态:DB 连接 / 设置 / 主题 / 错题本。

use crate::wrongbook::WrongBookStore;
use exam_engine::{BuildStats, CardHit, EngineError, IndexBuilder, SearchHit, SearchQuery};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

/// 当前 Unix 秒数。
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    Light,
    Dark,
    #[default]
    System,
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }

    /// 解析 Theme 到 `data-theme` 实际取值(`System` → 跟随 prefers-color-scheme,
    /// 渲染时由 CSS `@media (prefers-color-scheme: dark)` 兜底,attr 留空即可)。
    pub fn data_attr(self) -> Option<&'static str> {
        match self {
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
            Self::System => None,
        }
    }
}

/// 把当前 Theme 写入 `document.documentElement[data-theme]`。
/// Desktop 启动时 + Settings 保存后各调一次。
pub fn apply_theme_to_root(theme: Theme) {
    let script = match theme.data_attr() {
        Some(v) => format!(
            r#"document.documentElement.setAttribute('data-theme','{v}');"#
        ),
        None => r#"document.documentElement.removeAttribute('data-theme');"#.to_string(),
    };
    let _ = dioxus::document::eval(&script);
}

#[derive(Debug, Clone)]
pub struct AppSettings {
    pub theme: Theme,
    pub data_root: PathBuf,
    pub db_path: PathBuf,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            data_root: default_data_root(),
            db_path: default_db_path(),
        }
    }
}

impl AppSettings {
    pub fn theme_from_str(s: &str) -> Theme {
        match s {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }
}

fn default_data_root() -> PathBuf {
    PathBuf::from("/Users/hgxszhj/x_videos-dowloads/exam-lookup")
}

fn default_db_path() -> PathBuf {
    PathBuf::from("questions.db")
}

fn default_settings_path() -> PathBuf {
    if let Some(dir) = dirs::config_dir() {
        dir.join("exam-lookup-rs").join("settings.json")
    } else {
        PathBuf::from("exam-lookup-rs.settings.json")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DbStatus {
    #[default]
    Unloaded,
    Ready { questions: usize },
    Error(String),
}

/// 全局应用状态 — 通过 `provide_context` 注入子树。
///
/// `db` 用 `Arc<Mutex<Option<Connection>>>` 包装:
/// - `Connection` 是 `Send` 但非 `Sync`(rusqlite 保守做法)
/// - `Mutex<Option<Connection>>` 是 `Send + Sync`
/// - `Arc<...>` 提供 Clone
///   这样既满足 `use_context_provider(T: Clone)`,又能在多组件共享。
#[derive(Clone)]
#[allow(dead_code)] // db_status / with_conn 留给 Phase E+ 扩展(Settings 显示 DB 状态等)
pub struct AppState {
    pub settings: Arc<RwLock<AppSettings>>,
    pub db: Arc<Mutex<Option<Connection>>>,
    pub wrong_book: WrongBookStore,
    pub db_status: Arc<RwLock<DbStatus>>,
}

impl AppState {
    /// 创建并尝试加载 DB(失败不 panic,只置状态)。
    pub fn bootstrap() -> Self {
        let settings = load_settings();
        let wrong_book = WrongBookStore::load_or_default();
        let db_path = settings.db_path.clone();
        let db: Arc<Mutex<Option<Connection>>> = Arc::new(Mutex::new(None));
        let status = match Connection::open(&db_path) {
            Ok(c) => {
                let s = match count_questions(&c) {
                    Ok(n) => DbStatus::Ready { questions: n as usize },
                    Err(e) => DbStatus::Error(format!("读取 DB 失败:{e}")),
                };
                if let Ok(mut g) = db.lock() {
                    *g = Some(c);
                }
                s
            }
            Err(e) => DbStatus::Error(format!("打开 DB 失败:{} ({e})", db_path.display())),
        };
        Self {
            settings: Arc::new(RwLock::new(settings)),
            db,
            wrong_book,
            db_status: Arc::new(RwLock::new(status)),
        }
    }

    pub fn search(&self, query: &str, bank: Option<&str>, limit: usize) -> Result<Vec<SearchHit>, String> {
        let guard = self.db.lock().map_err(|e| e.to_string())?;
        let conn = guard.as_ref().ok_or_else(|| "DB 未就绪".to_string())?;
        let mut q = SearchQuery::new(query).with_limit(limit);
        if let Some(b) = bank {
            q = q.with_bank(b);
        }
        exam_engine::search::search(conn, &q).map_err(|e| match e {
            EngineError::Other(s) => s,
            other => format!("{other:?}"),
        })
    }

    /// 卡片搜索(Phase G):查 `card` / `card_fts`。
    pub fn search_cards(&self, query: &str, limit: usize) -> Result<Vec<CardHit>, String> {
        let guard = self.db.lock().map_err(|e| e.to_string())?;
        let conn = guard.as_ref().ok_or_else(|| "DB 未就绪".to_string())?;
        let q = SearchQuery::new(query).with_limit(limit);
        exam_engine::search::search_cards(conn, &q).map_err(|e| match e {
            EngineError::Other(s) => s,
            other => format!("{other:?}"),
        })
    }

    #[allow(dead_code)] // 留作未来 GUI:Settings 显示 DB ready/loading
    pub fn with_conn<F, R>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&Connection) -> R,
    {
        let guard = self.db.lock().map_err(|e| e.to_string())?;
        let conn = guard.as_ref().ok_or_else(|| "DB 未就绪".to_string())?;
        Ok(f(conn))
    }

    /// 不可变访问 Connection(&Connection)。失败 → String。
    pub fn conn(&self) -> Result<std::sync::MutexGuard<'_, Option<Connection>>, String> {
        self.db.lock().map_err(|e| e.to_string())
    }

    #[allow(dead_code)]
    pub fn db_status_snapshot(&self) -> DbStatus {
        self.db_status.read().map(|s| s.clone()).unwrap_or(DbStatus::Unloaded)
    }

    /// 从 `data_root`(bank JSON + crawl 目录)重建索引到 `db_path`。
    ///
    /// 完成后原子替换 `self.db` 中的 Connection,刷新 `db_status`。
    /// Phase H:SettingsPage "Rebuild now" 按钮 + 真实数据 E2E 入口。
    pub fn rebuild_index(
        &self,
        data_root: &Path,
        db_path: &Path,
    ) -> Result<BuildStats, String> {
        // 1. 标记 Loading 状态(UI 看到错误时给反馈)
        if let Ok(mut s) = self.db_status.write() {
            *s = DbStatus::Unloaded;
        }

        // 2. 一次性 build 到指定 db_path(独立连接,不影响 self.db)
        let mut builder = IndexBuilder::open(db_path)
            .map_err(|e| format!("打开索引文件失败:{} ({e})", db_path.display()))?;
        let stats = builder
            .rebuild(data_root)
            .map_err(|e| format!("重建索引失败:{} (data_root={})", e, data_root.display()))?;

        // 3. 打开新的 readonly 连接 + 写回 self.db
        let new_conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| format!("重建后打开只读连接失败:{e}"))?;

        {
            let mut guard = self.db.lock().map_err(|e| e.to_string())?;
            *guard = Some(new_conn);
        }

        // 4. 写 db_status = Ready{N}
        if let Ok(mut s) = self.db_status.write() {
            *s = DbStatus::Ready { questions: stats.questions };
        }
        Ok(stats)
    }
}

fn count_questions(c: &Connection) -> rusqlite::Result<i64> {
    c.query_row("SELECT count(*) FROM q", [], |r| r.get(0))
}

fn load_settings() -> AppSettings {
    let path = default_settings_path();
    if !path.exists() {
        return AppSettings::default();
    }
    let Ok(s) = std::fs::read_to_string(&path) else {
        return AppSettings::default();
    };
    let Ok(parsed) = serde_json::from_str::<RawSettings>(&s) else {
        return AppSettings::default();
    };
    AppSettings {
        theme: AppSettings::theme_from_str(&parsed.theme),
        data_root: PathBuf::from(parsed.data_root),
        db_path: PathBuf::from(parsed.db_path),
    }
}

pub fn save_settings(s: &AppSettings) -> Result<(), String> {
    let path = default_settings_path();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    let raw = RawSettings {
        theme: s.theme.as_str().to_string(),
        data_root: s.data_root.to_string_lossy().to_string(),
        db_path: s.db_path.to_string_lossy().to_string(),
    };
    let json = serde_json::to_string_pretty(&raw).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct RawSettings {
    theme: String,
    data_root: String,
    db_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wrongbook::WrongBook;

    #[test]
    fn theme_roundtrip() {
        for t in [Theme::Light, Theme::Dark, Theme::System] {
            assert_eq!(AppSettings::theme_from_str(t.as_str()), t);
        }
        assert_eq!(AppSettings::theme_from_str("bogus"), Theme::System);
    }

    #[test]
    fn wrong_book_default_is_empty() {
        let mut wb = WrongBook::default();
        assert!(wb.entries().is_empty());
        assert!(wb.add("q1", "src", "Q?", "A").is_ok());
        assert_eq!(wb.entries().len(), 1);
    }

    #[test]
    fn wrong_book_dedupes_by_question_id() {
        let mut wb = WrongBook::default();
        wb.add("q1", "src", "Q?", "A").unwrap();
        wb.add("q1", "src", "Q?", "B").unwrap();
        assert_eq!(wb.entries().len(), 1);
        assert_eq!(wb.entries()[0].my_answer, "B");
    }

    #[test]
    fn wrong_book_remove() {
        let mut wb = WrongBook::default();
        wb.add("q1", "src", "Q?", "A").unwrap();
        wb.remove("q1");
        assert!(wb.entries().is_empty());
    }

    #[test]
    fn wrong_book_export_json() {
        let mut wb = WrongBook::default();
        wb.add("q1", "src1", "Q?", "A").unwrap();
        wb.add("q2", "src2", "Q2?", "B").unwrap();
        let json = wb.export_json().unwrap();
        assert!(json.contains("q1"));
        assert!(json.contains("src1"));
        assert!(json.contains("q2"));
    }

    #[test]
    fn app_settings_default_data_root_is_known() {
        let s = AppSettings::default();
        assert!(s.db_path.to_string_lossy().contains("questions"));
    }
}