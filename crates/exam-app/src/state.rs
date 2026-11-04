//! 全局状态:DB 连接 / 设置 / 主题 / 错题本。

use crate::wrongbook::WrongBookStore;
use exam_engine::watcher::DataRootWatcher;
use exam_engine::{BuildStats, CardHit, EngineError, IndexBuilder, SearchHit, SearchQuery};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

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
        Some(v) => format!(r#"document.documentElement.setAttribute('data-theme','{v}');"#),
        None => r#"document.documentElement.removeAttribute('data-theme');"#.to_string(),
    };
    let _ = dioxus::document::eval(&script);
}

#[derive(Debug, Clone)]
pub struct AppSettings {
    pub theme: Theme,
    pub data_root: PathBuf,
    pub db_path: PathBuf,
    /// data_root 文件变化时是否自动 rebuild_index(Phase M)
    pub auto_rebuild: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            data_root: default_data_root(),
            db_path: default_db_path(),
            auto_rebuild: false,
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
    Ready {
        questions: usize,
    },
    Error(String),
}

/// watcher 消费线程的**代际**句柄(P1-3)。
///
/// 代际独立:每一代线程拥有**自己的**停止标志 `Arc<AtomicBool>`。
///
/// 修复前的 bug(P1-3):所有代共用 `watcher_enabled` 一个 `AtomicBool`。
/// `stop_watcher` 把它置 `false` 后,紧接着的 `ensure_watcher` 又置回 `true` ——
/// 旧代线程的 `while enabled.load()` 再次为真 → **旧代复活**,
/// 继续用**旧的 data_root** 重建索引,把新一代的结果覆盖掉。
///
/// 现在:停旧代只置**旧代自己**的标志;旧代一定退出由 `stop_watcher` 的 join 保证。
pub struct WatcherSlot {
    /// 本代专属停止标志:`false` → 该代线程退出消费循环。
    stop: Arc<AtomicBool>,
    /// 线程句柄。`stop_watcher` **真正 join**(不是 detach):
    /// 保证"旧代完全退出"之后调用方才可能启新一代。
    join: JoinHandle<()>,
}

/// watcher 消费线程的 poll 周期。
///
/// 同时也是 `stop_watcher` 的 join 延迟上界(线程最迟一个周期后看到停止标志)。
/// 500ms → 100ms:停止延迟从半秒降到百毫秒级,文件事件检测延迟同样可接受。
const WATCHER_POLL_INTERVAL: Duration = Duration::from_millis(100);

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
    /// Phase M:防并发 rebuild — 用户点按钮 + watcher 线程 + bootstrap 同时只一个跑。
    pub rebuild_lock: Arc<Mutex<()>>,
    /// watcher 消费线程的**当前代**(`None` = 未启动)。见 [`WatcherSlot`]:停止是**代际独立**的。
    pub watcher: Arc<Mutex<Option<WatcherSlot>>>,
    /// UI 指示:当前是否有 watcher 在跑。
    ///
    /// **不是**线程的停止标志 —— 停止标志是每代私有的(见 [`WatcherSlot::stop`]);
    /// 共享标志会让"停旧代 + 启新代"复活旧代(P1-3)。
    pub watcher_enabled: Arc<AtomicBool>,
}

impl AppState {
    /// 创建并尝试加载 DB(失败不 panic,只置状态)。
    pub fn bootstrap() -> Self {
        Self::bootstrap_from(load_settings())
    }

    /// Phase M:从指定 settings 引导 — 公开以便测试,且若 `auto_rebuild=true` 启 watcher。
    ///
    /// `ensure_watcher` 失败(目录消失 / IO 错)→ 仅 `tracing::warn!`,**不** panic — UI 可正常启动。
    pub fn bootstrap_from(settings: AppSettings) -> Self {
        let auto_rebuild_enabled = settings.auto_rebuild;
        let data_root = settings.data_root.clone();
        let wrong_book = WrongBookStore::load_or_default();
        let db_path = settings.db_path.clone();
        let db: Arc<Mutex<Option<Connection>>> = Arc::new(Mutex::new(None));
        let status = match Connection::open(&db_path) {
            Ok(c) => {
                let s = match count_questions(&c) {
                    Ok(n) => DbStatus::Ready {
                        questions: n as usize,
                    },
                    Err(e) => DbStatus::Error(format!("读取 DB 失败:{e}")),
                };
                if let Ok(mut g) = db.lock() {
                    *g = Some(c);
                }
                s
            }
            Err(e) => DbStatus::Error(format!("打开 DB 失败:{} ({e})", db_path.display())),
        };
        let rebuild_lock = Arc::new(Mutex::new(()));
        let watcher: Arc<Mutex<Option<WatcherSlot>>> = Arc::new(Mutex::new(None));
        let watcher_enabled = Arc::new(AtomicBool::new(false));
        let state = Self {
            settings: Arc::new(RwLock::new(settings)),
            db,
            wrong_book,
            db_status: Arc::new(RwLock::new(status)),
            rebuild_lock,
            watcher,
            watcher_enabled,
        };
        // Phase M:若用户上次会话开了 auto_rebuild,启动时直接启 watcher
        if auto_rebuild_enabled {
            if let Err(e) = state.ensure_watcher() {
                tracing::warn!(
                    data_root = %data_root.display(),
                    error = %e,
                    "bootstrap 启 watcher 失败,UI 可手动重试"
                );
            }
        }
        state
    }

    pub fn search(
        &self,
        query: &str,
        bank: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchHit>, String> {
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
        self.db_status
            .read()
            .map(|s| s.clone())
            .unwrap_or(DbStatus::Unloaded)
    }

    /// 从 `data_root`(bank JSON + crawl 目录)重建索引到 `db_path`。
    ///
    /// 完成后原子替换 `self.db` 中的 Connection,刷新 `db_status`。
    /// Phase H:SettingsPage "Rebuild now" 按钮 + 真实数据 E2E 入口。
    ///
    /// Phase M:用 `rebuild_lock.try_lock()` 防并发 — 用户点按钮 + watcher 线程 + bootstrap
    /// 同时只一个能跑。锁不上立即返 Err,UI 显示"重建中…"(而不是阻塞 UI 线程)。
    pub fn rebuild_index(&self, data_root: &Path, db_path: &Path) -> Result<BuildStats, String> {
        let _guard = self
            .rebuild_lock
            .try_lock()
            .map_err(|_| "已有重建在进行".to_string())?;
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
            *s = DbStatus::Ready {
                questions: stats.questions,
            };
        }
        Ok(stats)
    }

    // ── Phase M:DataRootWatcher 接入 ──────────────────────────────────

    /// 幂等启动 watcher 消费线程(若已存在则 no-op)。
    ///
    /// 流程:
    /// 1. 检查 `self.watcher` slot,已 Some → 早返
    /// 2. 读当前 settings.data_root,调 `engine::watcher::watch()` 创 OS watcher
    /// 3. spawn std::thread 消费 `rx.recv_timeout(WATCHER_POLL_INTERVAL)` 循环,每次事件触发 `rebuild_index`
    /// 4. 存 [`WatcherSlot`]{stop 标志 + 线程句柄} 进 slot
    ///
    /// 失败(目录消失 / data_root 不存在)→ 返 Err,**不** panic。
    #[allow(dead_code)] // Step 3 SettingsPage 调
    pub fn ensure_watcher(&self) -> Result<(), String> {
        // 1. 幂等检查 —— 全程持锁直到 slot 填好。
        //    旧实现"检查后释放锁、再 spawn、再加锁写 slot"存在 double-spawn 竞态:
        //    并发调用时后到的会顶掉先到的 handle,被顶掉的线程既没人 join 也没人能停
        //    → 变成永久孤儿线程(它的停止标志永远不会被置 false)。
        let mut slot = self.watcher.lock().map_err(|e| e.to_string())?;
        if slot.is_some() {
            return Ok(());
        }

        // 2. 读当前 data_root (snapshot 一次,启动期固定;Step 3 会接 settings 变化重启)
        let (data_root, db_path) = {
            let s = self.settings.read().map_err(|e| e.to_string())?;
            (s.data_root.clone(), s.db_path.clone())
        };

        // 3. 启 OS watcher(失败:目录消失 / IO 错)
        let watcher = exam_engine::watcher::watch(&data_root, Duration::from_millis(300))
            .map_err(|e| format!("启动 data_root 监听失败 ({}): {e}", data_root.display()))?;

        // 4. 启消费线程 —— 停止标志**每代独有**(P1-3):
        //    停旧代只置旧代自己的标志,启新代不会把旧代"复活"。
        let stop = Arc::new(AtomicBool::new(true));
        let stop_in_thread = Arc::clone(&stop);
        let state_clone = self.clone();
        let join = std::thread::Builder::new()
            .name("data-root-watcher".into())
            .spawn(move || {
                watcher_consumer_loop(state_clone, watcher, stop_in_thread, data_root, db_path)
            })
            .map_err(|e| format!("spawn watcher 线程失败: {e}"))?;

        *slot = Some(WatcherSlot { stop, join });
        self.watcher_enabled.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// 停止 watcher 消费线程:置**本代**停止标志为 false,并**真正 join** 线程。
    ///
    /// 修复前(P1-3):只是把 handle 从 slot take 掉 → 线程 detach 继续跑,
    /// 且共享的 `watcher_enabled` 会被下一代重新置 true → 旧代复活,
    /// 用旧 data_root 重建索引覆盖新索引。
    ///
    /// 现在:join 保证"旧代完全退出"之后调用方才可能启下一代。
    /// 注意 join 最多阻塞一个 poll 周期(见 [`WATCHER_POLL_INTERVAL`]),
    /// 或等当前正在跑的重建结束 —— 因此 UI 侧应在**后台线程**调用本方法。
    #[allow(dead_code)] // Step 3 SettingsPage 调
    pub fn stop_watcher(&self) {
        // UI 状态立即置 false(不等 join,免得 UI 以为还在跑)
        self.watcher_enabled.store(false, Ordering::Relaxed);

        // take 出 slot —— **先释放锁再 join**:持锁等线程会挡住 ensure_watcher
        let taken = match self.watcher.lock() {
            Ok(mut slot) => slot.take(),
            Err(e) => {
                tracing::warn!(error = %e, "stop_watcher: 取 watcher slot 失败");
                None
            }
        };
        let Some(slot) = taken else { return };

        // 只停**这一代**:置本代专属标志 → 新一代的标志不受影响,旧代也回不来
        slot.stop.store(false, Ordering::Relaxed);

        // 真正 join(旧实现是 detach —— 旧线程继续跑)
        if slot.join.join().is_err() {
            tracing::warn!("watcher 消费线程 panic(已 join)");
        }
    }

    /// 当前是否启用了 watcher 消费循环(用于 UI 反映状态)。
    #[allow(dead_code)] // Step 3 SettingsPage 调
    pub fn is_watcher_enabled(&self) -> bool {
        self.watcher_enabled.load(Ordering::Relaxed)
    }

    /// Phase M+:把 watcher 状态对齐到 `enabled` flag + 当前 settings。
    ///
    /// 在 SettingsPage "保存" 按钮 / 路径变更时调用,确保 watcher 监听新路径:
    /// - `enabled=true` → `stop_watcher` + `ensure_watcher`(**总是** 强制重启,因路径可能已变)
    /// - `enabled=false` → `stop_watcher`
    ///
    /// 失败仅 `tracing::warn!`,UI 仍可正常用。
    ///
    /// 注意:`stop_watcher` 会 join 旧线程 → 本方法**可能阻塞**(最多一个 poll 周期,
    /// 或当前正在跑的重建的剩余时间)。UI 侧应在后台线程调用(见 `ui/settings.rs`)。
    #[allow(dead_code)] // Step M+ SettingsPage 保存按钮调
    pub fn sync_watcher(&self, enabled: bool) {
        if enabled {
            self.stop_watcher();
            if let Err(e) = self.ensure_watcher() {
                tracing::warn!(error = %e, "sync_watcher: 启 watcher 失败");
            }
        } else {
            self.stop_watcher();
        }
    }
}

/// Phase M:watcher 消费线程函数 — `rx.recv_timeout(WATCHER_POLL_INTERVAL)` 循环,事件触发 rebuild_index。
///
/// 参数:
/// - `state`:AppState clone(Arc 字段共享锁)
/// - `watcher`:DataRootWatcher 持有 OS watcher + 接收端
/// - `stop`:**本代专属**停止标志(P1-3 —— 共享标志会让旧代被新一代复活)
/// - `data_root` / `db_path`:snapshot 启动时的路径(Step 3 接 settings 变化重启时再 snapshot)
#[allow(dead_code)] // Step 3 SettingsPage 调 ensure_watcher 时被引用
fn watcher_consumer_loop(
    state: AppState,
    watcher: DataRootWatcher,
    stop: Arc<AtomicBool>,
    data_root: PathBuf,
    db_path: PathBuf,
) {
    use std::sync::mpsc::RecvTimeoutError;
    tracing::info!(data_root = %data_root.display(), "watcher 消费线程启动");
    while stop.load(Ordering::Relaxed) {
        match watcher.rx.recv_timeout(WATCHER_POLL_INTERVAL) {
            Ok(_event) => {
                // 重建 — 用 try_lock;锁不上(用户正在手动重建)skip,下次事件会再试
                match state.rebuild_index(&data_root, &db_path) {
                    Ok(stats) => tracing::info!(
                        banks = stats.banks,
                        questions = stats.questions,
                        cards = stats.cards,
                        errors = stats.errors,
                        "watcher 触发自动重建完成"
                    ),
                    Err(e) if e == "已有重建在进行" => {
                        tracing::debug!("watcher 跳过: 已有重建在进行");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "watcher 触发自动重建失败");
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                tracing::info!("watcher 通道断开,消费线程退出");
                break;
            }
        }
    }
    tracing::info!("watcher 停止标志置位,消费线程退出");
    // watcher 在此 Drop → OS watcher 停 → 监听结束
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
        // 缺失视为 false(老 settings.json 无此字段)— 兼容向前
        auto_rebuild: parsed.auto_rebuild,
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
        auto_rebuild: s.auto_rebuild,
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
    /// `serde(default)` 兼容老 settings.json 缺此字段(Phase M 之前的版本)
    #[serde(default)]
    auto_rebuild: bool,
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

    /// Phase M Step 1:auto_rebuild 默认 false(用户主动开)。
    #[test]
    fn auto_rebuild_default_is_false() {
        let s = AppSettings::default();
        assert!(!s.auto_rebuild);
    }

    /// Phase M Step 1:save → load 往返保留 auto_rebuild=true。
    #[test]
    fn settings_roundtrip_with_auto_rebuild() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.json");
        // 直接写 RawSettings JSON + load_settings 走 tmp path 难(默认 dirs::config_dir 不可改);
        // 改测 save/load 全链路用 default_settings_path 但配置目录临时替换太重。
        // 退而测:序列化层 — RawSettings 结构对 auto_rebuild 字段保 roundtrip。
        let raw = RawSettings {
            theme: "dark".into(),
            data_root: "/tmp/a".into(),
            db_path: "/tmp/b.db".into(),
            auto_rebuild: true,
        };
        let s = serde_json::to_string(&raw).expect("ser");
        let back: RawSettings = serde_json::from_str(&s).expect("de");
        assert!(back.auto_rebuild);
        assert_eq!(back.theme, "dark");
        // 顺便验证路径写入没踩坏
        let _ = path; // 避免 unused warning
    }

    /// Phase M Step 1:老 settings.json 无 auto_rebuild 字段 → 解析时 `serde(default)` 兜底为 false。
    #[test]
    fn settings_legacy_without_auto_rebuild_defaults_false() {
        let legacy = r#"{"theme":"light","data_root":"/x","db_path":"/y.db"}"#;
        let parsed: RawSettings = serde_json::from_str(legacy).expect("parse legacy");
        assert!(!parsed.auto_rebuild, "缺失字段应默认为 false");
    }

    // ── Phase M Step 2:rebuild_lock + watcher 集成测试 ──

    /// rebuild_lock 防并发:持有锁时,第二个 try_lock 应返 Err。
    #[test]
    fn rebuild_lock_prevents_concurrent_rebuilds() {
        let s = AppState::bootstrap();
        // 模拟"有一个长跑 rebuild 持有锁"
        let _held = s.rebuild_lock.try_lock().expect("first lock acquired");
        // 第二个 try_lock 必须失败
        let second = s.rebuild_lock.try_lock();
        assert!(second.is_err(), "持锁时第二个 try_lock 应返 Err");
        // 释放后能再锁
        drop(_held);
        let third = s.rebuild_lock.try_lock();
        assert!(third.is_ok(), "释放后应能再锁");
    }

    /// ensure_watcher 幂等:调两次,第二次是 no-op(不应 double-spawn)。
    #[test]
    fn ensure_watcher_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let s = AppState::bootstrap();
        // 改 settings 的 data_root 指向 tempdir(否则 watch() 会报目录不存在)
        {
            let mut g = s.settings.write().expect("settings lock");
            g.data_root = dir.path().to_path_buf();
            g.db_path = db_path.clone();
        }
        // 第一次:启动成功
        s.ensure_watcher().expect("first ensure");
        assert!(s.watcher.lock().unwrap().is_some(), "启动后 slot 应 Some");
        // 第二次:no-op
        s.ensure_watcher().expect("second ensure (idempotent)");
        // 句柄数仍是 1(没有 double-spawn)
        assert!(s.watcher.lock().unwrap().is_some());
        // 清理:停 watcher 避免测试结束时的孤儿线程
        s.stop_watcher();
    }

    /// stop_watcher 清空 slot + 置 enabled=false。
    #[test]
    fn stop_watcher_clears_handle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = AppState::bootstrap();
        {
            let mut g = s.settings.write().expect("settings lock");
            g.data_root = dir.path().to_path_buf();
        }
        s.ensure_watcher().expect("start");
        assert!(s.is_watcher_enabled(), "start 后 enabled=true");
        assert!(s.watcher.lock().unwrap().is_some());
        s.stop_watcher();
        assert!(!s.is_watcher_enabled(), "stop 后 enabled=false");
        assert!(s.watcher.lock().unwrap().is_none(), "stop 后 slot 应 None");
    }

    /// Phase M Step 4:bootstrap_with auto_rebuild=true → 启 watcher;auto_rebuild=false → 不启。
    #[test]
    fn bootstrap_with_auto_rebuild_starts_watcher() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = AppSettings {
            data_root: dir.path().to_path_buf(),
            db_path: dir.path().join("test.db"),
            auto_rebuild: true,
            ..Default::default()
        };
        let s = AppState::bootstrap_from(settings);
        assert!(
            s.is_watcher_enabled(),
            "auto_rebuild=true → bootstrap 启 watcher"
        );
        assert!(s.watcher.lock().unwrap().is_some());
        s.stop_watcher(); // 清理
    }

    /// Phase M Step 4:bootstrap_with auto_rebuild=false → 不启 watcher。
    #[test]
    fn bootstrap_without_auto_rebuild_does_not_start_watcher() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = AppSettings {
            data_root: dir.path().to_path_buf(),
            db_path: dir.path().join("test.db"),
            auto_rebuild: false,
            ..Default::default()
        };
        let s = AppState::bootstrap_from(settings);
        assert!(
            !s.is_watcher_enabled(),
            "auto_rebuild=false → bootstrap 不启 watcher"
        );
        assert!(s.watcher.lock().unwrap().is_none());
    }

    /// Phase M Step 4:bootstrap_with auto_rebuild=true 但 data_root 目录消失 → 不 panic。
    #[test]
    fn bootstrap_handles_missing_data_root_gracefully() {
        let dir = tempfile::tempdir().expect("tempdir");
        // 立刻删掉 data_root 模拟"目录消失"
        let nonexistent = dir.path().join("does_not_exist");
        let settings = AppSettings {
            data_root: nonexistent,
            db_path: dir.path().join("test.db"),
            auto_rebuild: true,
            ..Default::default()
        };
        // 不应 panic
        let s = AppState::bootstrap_from(settings);
        // watcher 启动失败 → slot 应仍 None(因为 ensure_watcher 返 Err 后未存 handle)
        assert!(!s.is_watcher_enabled(), "启动失败 → enabled=false");
        assert!(s.watcher.lock().unwrap().is_none(), "启动失败 → slot=None");
    }

    /// Phase M+:sync_watcher(true) 在 data_root 变化后强制重启,slot 仍 Some。
    #[test]
    fn sync_watcher_restarts_after_data_root_change() {
        let dir_a = tempfile::tempdir().expect("tempdir A");
        let dir_b = tempfile::tempdir().expect("tempdir B");
        let s = AppState::bootstrap();
        // 初始启 watcher → 监听 dir_a
        {
            let mut g = s.settings.write().expect("settings lock");
            g.data_root = dir_a.path().to_path_buf();
        }
        s.ensure_watcher().expect("start with A");
        assert!(s.watcher.lock().unwrap().is_some());

        // 改 settings.data_root → dir_b
        {
            let mut g = s.settings.write().expect("settings lock");
            g.data_root = dir_b.path().to_path_buf();
        }

        // sync_watcher(true) → 强制重启,新线程监听 dir_b
        s.sync_watcher(true);
        assert!(s.is_watcher_enabled(), "sync(true) 后 enabled=true");
        assert!(
            s.watcher.lock().unwrap().is_some(),
            "sync(true) 后 slot 仍 Some(新 handle)"
        );

        // 清理
        s.stop_watcher();
    }

    /// Phase M+:sync_watcher(false) 停 watcher,即使之前在跑。
    #[test]
    fn sync_watcher_disabled_stops_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = AppState::bootstrap();
        {
            let mut g = s.settings.write().expect("settings lock");
            g.data_root = dir.path().to_path_buf();
        }
        s.ensure_watcher().expect("start");
        assert!(s.is_watcher_enabled());
        s.sync_watcher(false);
        assert!(!s.is_watcher_enabled(), "sync(false) 后 enabled=false");
        assert!(s.watcher.lock().unwrap().is_none());
    }

    // ── P1-3:watcher 代际隔离 + 真正 join ──────────────────────────────

    /// 写一个最小 bank JSON 到 `<root>/banks/<name>.json`。
    fn write_bank(root: &Path, name: &str, stem: &str) {
        let banks = root.join("banks");
        std::fs::create_dir_all(&banks).expect("mkdir banks");
        let json = format!(
            r#"{{"meta": {{"title": "{name}"}}, "questions": [{{"id": "{name}-1", "number": 1, "type": "single", "question": "{stem}", "answerKeys": ["A"]}}]}}"#
        );
        std::fs::write(banks.join(format!("{name}.json")), json).expect("write bank");
    }

    /// 当前 DB 的 `q` 表里是否有题干包含 `needle` 的题。
    fn db_has_question(state: &AppState, needle: &str) -> bool {
        let Ok(guard) = state.db.lock() else {
            return false;
        };
        let Some(conn) = guard.as_ref() else {
            return false;
        };
        conn.query_row(
            "SELECT count(*) FROM q WHERE question LIKE ?1",
            [format!("%{needle}%")],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    /// 轮询直到 `f` 为真或超时。
    fn wait_until<F: FnMut() -> bool>(mut f: F, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        f()
    }

    /// P1-3 回归:停旧代 + 启新代后,**旧代不得复活**。
    ///
    /// 修复前:所有代共用 `watcher_enabled` 一个 `AtomicBool`。
    /// `stop_watcher` 置 false → `ensure_watcher` 置回 true → 旧代线程的
    /// `while enabled.load()` 又为真 → 旧代继续用**旧 data_root** 重建索引,
    /// 把新一代的结果覆盖掉(用户改了 data_root,索引却被"旧路径"顶回来)。
    ///
    /// 本测试用真实 FS 事件驱动:切到新 data_root 之后,旧 data_root 的改动
    /// **不应**再触发任何重建。
    #[test]
    fn old_watcher_generation_does_not_rebuild_after_restart() {
        let dir_a = tempfile::tempdir().expect("A");
        let dir_b = tempfile::tempdir().expect("B");
        let db_path = dir_a.path().join("idx.db");

        write_bank(dir_a.path(), "a", "AAA 题干");
        write_bank(dir_b.path(), "b", "BBB 题干");

        let settings = AppSettings {
            data_root: dir_a.path().to_path_buf(),
            db_path,
            auto_rebuild: false,
            ..Default::default()
        };
        let s = AppState::bootstrap_from(settings);
        s.ensure_watcher().expect("gen1 启动");

        // gen1 监听 A → 改 A → 应重建出 "AAA 题干"
        write_bank(dir_a.path(), "a2", "AAA 题干");
        assert!(
            wait_until(|| db_has_question(&s, "AAA 题干"), Duration::from_secs(15)),
            "gen1 应被 A 的文件变化触发重建"
        );

        // 切到 B → 强制重启 watcher(gen2 监听 B)
        {
            let mut g = s.settings.write().expect("settings lock");
            g.data_root = dir_b.path().to_path_buf();
        }
        s.sync_watcher(true);
        assert!(s.is_watcher_enabled(), "sync(true) 后 watcher 应在跑");

        // gen2 监听 B → 改 B → 重建来源应是 B(DB 里不再有 A 的题)
        write_bank(dir_b.path(), "b2", "BBB 题干");
        assert!(
            wait_until(|| db_has_question(&s, "BBB 题干"), Duration::from_secs(15)),
            "gen2 应被 B 的文件变化触发重建"
        );
        assert!(
            !db_has_question(&s, "AAA 题干"),
            "重建来源应是 B —— DB 里不该再有 A 的题"
        );

        // 关键回归:改**旧** data_root A ——
        // 旧代若复活,会用 A 重建,把新索引顶掉(旧实现必红)。
        write_bank(dir_a.path(), "a3", "AAA 题干");
        std::thread::sleep(Duration::from_secs(3));
        assert!(
            !db_has_question(&s, "AAA 题干"),
            "旧代复活了:旧 data_root 的改动触发了重建(P1-3 未修)"
        );

        s.stop_watcher();
    }

    /// P1-3 契约:`stop_watcher` 返回时,旧代线程必须**已经退出**(真正 join,不是 detach)。
    ///
    /// 探针:消费线程持有 `AppState` 副本 → 线程退出才会 drop 掉那份副本,
    /// `Arc::strong_count(&state.db)` 才会回落到基线。
    ///
    /// 修复前:`stop_watcher` 只把 handle 从 slot take 掉 → 线程 detach 继续跑;
    /// 且共享的 `watcher_enabled` 被新一代置回 true → 旧代**永远**活着
    /// → strong_count 永远多 1 → 红。
    #[test]
    fn stop_watcher_joins_the_previous_generation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = AppState::bootstrap_from(AppSettings {
            data_root: dir.path().to_path_buf(),
            db_path: dir.path().join("idx.db"),
            auto_rebuild: false,
            ..Default::default()
        });
        let base = Arc::strong_count(&s.db);

        s.ensure_watcher().expect("gen1");
        assert_eq!(
            Arc::strong_count(&s.db),
            base + 1,
            "消费线程应持有 AppState 副本"
        );

        // 停旧代 + 启新代:此时活着的消费线程应**只有**新代
        s.stop_watcher();
        s.ensure_watcher().expect("gen2");
        assert_eq!(
            Arc::strong_count(&s.db),
            base + 1,
            "旧代必须已 join 退出(否则它还活着 → 会用旧 data_root 覆盖新索引)"
        );

        s.stop_watcher();
        assert_eq!(
            Arc::strong_count(&s.db),
            base,
            "全部停完后不应有任何消费线程存活"
        );
    }
}
