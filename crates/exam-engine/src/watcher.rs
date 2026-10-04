//! `DataRootWatcher` — debounced file-system watcher primitive.
//!
//! 封装 `notify-debouncer-mini`(OS-native:FSEvents / ReadDirectoryChangesW / inotify),
//! 在指定目录下任何 JSON / 子目录变化后,经 `debounce` 窗口合并,通过 `mpsc::Receiver`
//! 投递 `DebouncedEvent`。
//!
//! **职责边界(Phase I+ 锁定)**:
//! - 此模块只暴露可复用 primitive: `watch(root, debounce) -> DataRootWatcher`
//! - 何时调用 rebuild_index / 重建节流策略 / 与 GUI 启动挂钩 → 由 app 层决定,本模块不过问
//! - 不引入异步运行时;`rx.recv()` 是阻塞 API,符合现有 engine「纯逻辑 lib」约束

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use notify_debouncer_mini::{new_debouncer, DebounceEventResult, DebouncedEvent, Debouncer};
use notify::{RecommendedWatcher, RecursiveMode};

use crate::error::{EngineError, Result};

/// `DataRootWatcher` 的 drop 即停 watch;无需手动 stop。
///
/// `_debouncer` 持有 OS watcher + 后台线程;Drop 时两者一起终止,后台线程因 `tx` 关闭而退出。
pub struct DataRootWatcher {
    _debouncer: Debouncer<RecommendedWatcher>,
    /// caller 在自己的线程 / 任务里 `rx.recv()`,收到事件即触发 rebuild
    pub rx: mpsc::Receiver<DebouncedEvent>,
}

/// 启动对 `root` 的递归监听,`debounce` 窗口内合并事件。
///
/// 失败原因:
/// - 路径不存在 / 不是目录 → `EngineError::WatcherInit`
/// - OS 资源耗尽 → 同上包装
pub fn watch(root: &Path, debounce: Duration) -> Result<DataRootWatcher> {
    if !root.is_dir() {
        return Err(EngineError::WatcherInit(format!(
            "data_root 不是一个目录: {}",
            root.display()
        )));
    }
    let (tx, rx) = mpsc::channel();
    let mut debouncer = new_debouncer(debounce, move |res: DebounceEventResult| {
        if let Ok(events) = res {
            for ev in events {
                let _ = tx.send(ev);
            }
        }
    })
    .map_err(|e| EngineError::WatcherInit(e.to_string()))?;
    debouncer
        .watcher()
        .watch(root, RecursiveMode::Recursive)
        .map_err(|e| EngineError::WatcherInit(e.to_string()))?;
    Ok(DataRootWatcher {
        _debouncer: debouncer,
        rx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn fires_on_modify() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("foo.json");
        std::fs::write(&target, "{}").expect("seed");

        let mut watcher = watch(dir.path(), Duration::from_millis(150)).expect("watch");

        // 给 watcher 注册时间(OS 异步)
        std::thread::sleep(Duration::from_millis(250));

        // 触发 modify
        std::fs::write(&target, "{\"x\":1}").expect("modify");

        // 3s 内必须收到至少一个事件
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = false;
        while Instant::now() < deadline {
            if let Ok(_ev) = watcher.rx.recv_timeout(Duration::from_millis(200)) {
                got = true;
                break;
            }
        }
        assert!(got, "watcher 在 3s 内未触发 modify 事件");
    }

    #[test]
    fn rejects_non_dir() {
        let p = tempfile::NamedTempFile::new().expect("tmpfile");
        let err = watch(p.path(), Duration::from_millis(100))
            .err()
            .expect("non-dir must error");
        assert!(matches!(err, EngineError::WatcherInit(_)), "got: {err:?}");
    }
}