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
                // Step 1.1: log OS file event at debug (per-event; high-volume risk).
                tracing::debug!(
                    path = %ev.path.display(),
                    kind = ?ev.kind,
                    "watcher event"
                );
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

        let watcher = watch(dir.path(), Duration::from_millis(150)).expect("watch");

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

    /// Step 3.3 (2026-10-12):创建新文件触发事件
    #[test]
    fn fires_on_create() {
        let dir = tempfile::tempdir().expect("tempdir");
        let watcher = watch(dir.path(), Duration::from_millis(150)).expect("watch");
        std::thread::sleep(Duration::from_millis(250));

        // 新建文件
        let new = dir.path().join("new.json");
        std::fs::write(&new, "{}").expect("create");

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = false;
        while Instant::now() < deadline {
            if let Ok(_ev) = watcher.rx.recv_timeout(Duration::from_millis(200)) {
                got = true;
                break;
            }
        }
        assert!(got, "watcher 在 3s 内未触发 create 事件");
    }

    /// Step 3.3:删除文件触发事件
    #[test]
    fn fires_on_delete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("doomed.json");
        std::fs::write(&target, "{}").expect("seed");

        let watcher = watch(dir.path(), Duration::from_millis(150)).expect("watch");
        std::thread::sleep(Duration::from_millis(250));

        std::fs::remove_file(&target).expect("delete");

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = false;
        while Instant::now() < deadline {
            if let Ok(_ev) = watcher.rx.recv_timeout(Duration::from_millis(200)) {
                got = true;
                break;
            }
        }
        assert!(got, "watcher 在 3s 内未触发 delete 事件");
    }

    /// Step 3.3:递归子目录里的修改也能捕获
    #[test]
    fn fires_on_recursive_subdir_modify() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).expect("mkdir sub");
        let target = sub.join("nested.json");
        std::fs::write(&target, "{}").expect("seed");

        let watcher = watch(dir.path(), Duration::from_millis(150)).expect("watch");
        std::thread::sleep(Duration::from_millis(250));

        std::fs::write(&target, "{\"x\":2}").expect("modify nested");

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = false;
        while Instant::now() < deadline {
            if let Ok(_ev) = watcher.rx.recv_timeout(Duration::from_millis(200)) {
                got = true;
                break;
            }
        }
        assert!(got, "watcher 在 3s 内未触发 nested modify 事件");
    }

    /// Step 3.3:快速 burst(150ms 内 5 次 modify)→ debounce 合并为 1 个事件
    #[test]
    fn debounces_rapid_burst() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("burst.json");
        std::fs::write(&target, "{}").expect("seed");

        // debounce 窗口 300ms,burst 应合并
        let watcher = watch(dir.path(), Duration::from_millis(300)).expect("watch");
        std::thread::sleep(Duration::from_millis(250));

        // 150ms 内连续写 5 次
        let burst_start = Instant::now();
        while burst_start.elapsed() < Duration::from_millis(150) {
            std::fs::write(&target, "burst").expect("burst write");
            std::thread::sleep(Duration::from_millis(20));
        }

        // 收集 burst 完后再收到的事件(1s 内应见 1 个,但不应 ≥ 5)
        std::thread::sleep(Duration::from_millis(400));
        let mut count = 0usize;
        let collect_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < collect_deadline {
            if let Ok(_ev) = watcher.rx.recv_timeout(Duration::from_millis(100)) {
                count += 1;
            } else {
                // 连续 200ms 没新事件 → debounce 窗口已过,停止收集
                if count > 0 {
                    break;
                }
            }
        }
        // 严格说可能 1 或 2(初始 seed 也可能触发),但绝不应 ≥ 5
        assert!(
            count < 5,
            "debounce 应合并 5 次 burst,实际收到 {count} 个事件"
        );
        assert!(count >= 1, "应至少收到 1 个事件,got {count}");
    }
}