//! 间隔分档调度器:对齐 newsnow 的 `Time.{Test, Realtime, Fast, Default, Common, Slow}`。
//!
//! 设计:
//! - **纯函数**:`due_indices()` 根据 `Interval` + 上次 run 时间决定哪些 source 应该触发
//!   (无需真实时间,便于 unit test)
//! - **副作用**:`Scheduler::tick()` 调 `source.load()` + `cache.upsert()`
//! - **运行循环**:`run_loop_for(duration, stop)` 在 std 线程内阻塞 tick,适合 CLI 后台跑
//!
//! 关键不变量:
//! - 每个 source 自己的 `Interval` 决定 tick 频率,不强行统一
//! - `Expired` 状态强制重跑,`Fresh` 状态跳过
//! - 不重试:load 失败 → 仍写 cache(notes 标 error),防雪崩(对齐 R-007)

use crate::cache::{CacheEntry, CacheStatus, CacheStore, compute_status, should_refresh};
use crate::source::{Interval, LoadResult, Source};
#[cfg(test)]
use crate::error::EngineError;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// 失败 source 退避的初始秒数。后续 attempts 指数倍增,封顶 [`MAX_BACKOFF_SECS`]。
///
/// 选 5s(而不是 Interval::Test 的 1s)是希望至少跳过 5 个 tick 才重试,既给下游
/// 留恢复时间,又不让单测时间窗口过大。`MAX_BACKOFF_SECS` 兜底防长尾 source
/// 永久冻结(对齐 newsnow 的 `backoff_cap`)。
const BASE_BACKOFF_SECS: i64 = 5;
const MAX_BACKOFF_SECS: i64 = 300;

/// 指数退避:`attempts` 从 1 起算 → 5, 10, 20, 40, 80, 160, 300。
fn next_backoff_secs(attempts: u32) -> i64 {
    let shift = attempts.saturating_sub(1).min(20); // 防 shift overflow
    let exp = BASE_BACKOFF_SECS.saturating_mul(1i64 << shift);
    exp.min(MAX_BACKOFF_SECS)
}

/// 单 source 的调度状态。
struct ScheduleEntry {
    source: &'static dyn Source,
    last_run: Option<i64>,
    /// 连续失败次数;成功后重置为 0。
    attempts: u32,
    /// 允许再次 `load` 的最早时间(epoch secs);`None` 表示无退避。
    next_retry_at: Option<i64>,
}

/// 一次 tick 的结果(便于测试 + CLI 输出)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickOutcome {
    pub source_id: String,
    pub interval: Interval,
    pub status: CacheStatus,
    pub banks: usize,
    pub questions: usize,
    pub skipped: bool,
    pub error: Option<String>,
}

/// Scheduler 配置。
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// 单次 tick 后 thread sleep 时间(决定循环颗粒度)。
    pub tick_resolution: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            tick_resolution: Duration::from_millis(500),
        }
    }
}

/// 调度器:持有一组 source + 一个 CacheStore。
///
/// 持有 `Arc<Mutex<CacheStore>>` 而非裸 `Arc<CacheStore>`:这是 Step 0.1 后的
/// thread-safety 正确表达。`Mutex` 把跨线程访问显式化(如果未来 scheduler
/// 多线程化),也满足 clippy `arc_with_non_send_sync` lint。
pub struct Scheduler {
    entries: Vec<ScheduleEntry>,
    cache: Arc<Mutex<CacheStore>>,
    config: SchedulerConfig,
}

impl Scheduler {
    pub fn new(cache: Arc<Mutex<CacheStore>>) -> Self {
        Self {
            entries: Vec::new(),
            cache,
            config: SchedulerConfig::default(),
        }
    }

    pub fn with_config(mut self, config: SchedulerConfig) -> Self {
        self.config = config;
        self
    }

    pub fn register(&mut self, source: &'static dyn Source) {
        self.entries.push(ScheduleEntry {
            source,
            last_run: None,
            attempts: 0,
            next_retry_at: None,
        });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 纯函数:返回应该触发的 entry 索引。
    ///
    /// 不实际 load,只读 cache + `last_run`,便于单测。
    pub fn due_indices(&self, now: i64) -> Vec<usize> {
        let mut due = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            // 0. 退避判断(最优先):失败后未到 next_retry_at → 跳过,防雪崩
            if let Some(retry_at) = e.next_retry_at {
                if now < retry_at {
                    continue;
                }
            }
            let cached = self
                .cache
                .lock()
                .expect("cache mutex poisoned")
                .get(e.source.id())
                .ok()
                .flatten();
            // 1. 缓存判断(优先):过期 / 缺失 → 必跑
            // 2. last_run 判断(冗余保护):即使 cache 缺失但 last_run 存在 → 按 interval 算
            let cache_due = should_refresh(cached.as_ref(), e.source.interval(), now);
            let lr_due = e
                .last_run
                .map(|t| (now - t) >= e.source.interval().as_secs() as i64)
                .unwrap_or(true);
            if cache_due || lr_due {
                due.push(i);
            }
        }
        due
    }

    /// 执行一次 tick:对所有 due source 调 load() + 写 cache。
    #[tracing::instrument(skip_all, fields(data_root = %data_root.display(), now))]
    pub fn tick(&mut self, data_root: &Path, now: i64) -> Vec<TickOutcome> {
        let started = std::time::Instant::now();
        let due = self.due_indices(now);
        let mut out = Vec::with_capacity(due.len());
        for i in due {
            let e = &mut self.entries[i];
            let interval = e.source.interval();
            let ttl_secs = interval.as_secs();
            let source_id = e.source.id();

            // 先调 load(失败也要记 status)
            let load_result = e.source.load(data_root);

            match load_result {
                Ok(results) => {
                    let (banks, questions) = summarize(&results);
                    let status = compute_status(now, ttl_secs, now);
                    let entry = CacheEntry {
                        source_id: source_id.to_string(),
                        fetched_at: now,
                        ttl_secs,
                        status,
                        banks,
                        questions,
                        notes: None,
                    };
                    // upsert 失败不中断(本地 cache 写失败应容忍)
                    let _ = self
                        .cache
                        .lock()
                        .expect("cache mutex poisoned")
                        .upsert(&entry);
                    e.last_run = Some(now);
                    // 成功 → 清除退避,下个 tick 重新按 interval 调度
                    e.attempts = 0;
                    e.next_retry_at = None;
                    out.push(TickOutcome {
                        source_id: source_id.to_string(),
                        interval,
                        status,
                        banks,
                        questions,
                        skipped: false,
                        error: None,
                    });
                }
                Err(err) => {
                    let notes = Some(err.to_string());
                    // 失败仍写 cache(notes 标 error),让 UI 能看到 R-007 的失败
                    let entry = CacheEntry {
                        source_id: source_id.to_string(),
                        fetched_at: now,
                        ttl_secs,
                        status: CacheStatus::Expired,
                        banks: 0,
                        questions: 0,
                        notes,
                    };
                    let _ = self
                        .cache
                        .lock()
                        .expect("cache mutex poisoned")
                        .upsert(&entry);
                    e.last_run = Some(now);
                    // 失败 → 累加 attempts + 推 next_retry_at;后续 tick 在 due_indices 跳过
                    e.attempts = e.attempts.saturating_add(1);
                    e.next_retry_at = Some(now.saturating_add(next_backoff_secs(e.attempts)));
                    out.push(TickOutcome {
                        source_id: source_id.to_string(),
                        interval,
                        status: CacheStatus::Expired,
                        banks: 0,
                        questions: 0,
                        skipped: false,
                        error: Some(err.to_string()),
                    });
                }
            }
        }
        tracing::debug!(
            due_count = out.len(),
            error_count = out.iter().filter(|o| o.error.is_some()).count(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "scheduler tick complete"
        );
        out
    }

    /// 阻塞运行循环,直到 `stop` 置 true 或 `duration` 到时。
    ///
    /// Phase D 集成测试用:`run_loop_for(Duration::from_millis(2500), stop)` + `Interval::Test` source
    /// → 期望 ≥ 2 次触发。
    pub fn run_loop_for(
        &mut self,
        data_root: &Path,
        duration: Duration,
        stop: Arc<AtomicBool>,
    ) -> Vec<TickOutcome> {
        let start = Instant::now();
        let mut all = Vec::new();
        let base_now = crate::cache::now_secs();
        while !stop.load(Ordering::Relaxed) && start.elapsed() < duration {
            // 推进 now_secs(每次 tick 用真实时间,保证 cache age 计算正确)
            let now = crate::cache::now_secs();
            let outcomes = self.tick(data_root, now);
            if !outcomes.is_empty() {
                all.extend(outcomes);
            }
            std::thread::sleep(self.config.tick_resolution);
            // 防止 base_now 永不读
            let _ = base_now;
        }
        all
    }
}

fn summarize(results: &[LoadResult]) -> (usize, usize) {
    let banks = results.len();
    let questions = results.iter().map(|r| r.questions.len()).sum();
    (banks, questions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::now_secs;
    use std::path::PathBuf;

    /// 测试用 source:每次 load 计数 + 返回固定 bank。
    /// 返回 `&'static` 因为 `Source: 'static` 约束 + Scheduler 持 `&'static dyn Source`。
    fn counting_source(
        id: &'static str,
        interval: Interval,
        counter: &'static std::sync::atomic::AtomicUsize,
    ) -> &'static CountingSource {
        Box::leak(Box::new(CountingSource {
            id,
            display: id,
            interval,
            counter,
        }))
    }

    struct CountingSource {
        id: &'static str,
        display: &'static str,
        interval: Interval,
        counter: &'static std::sync::atomic::AtomicUsize,
    }

    impl Source for CountingSource {
        fn id(&self) -> &'static str { self.id }
        fn display_name(&self) -> &'static str { self.display }
        fn interval(&self) -> Interval { self.interval }
        fn load(&self, _data_root: &Path) -> crate::Result<Vec<LoadResult>> {
            self.counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![LoadResult {
                bank_id: crate::types::BankId::new("b1"),
                meta: None,
                questions: vec![],
                cards: vec![],
            }])
        }
    }

    fn dummy_root() -> PathBuf {
        PathBuf::from("/tmp")
    }

    #[test]
    fn new_scheduler_is_empty() {
        let store = Arc::new(Mutex::new(CacheStore::open_in_memory().unwrap()));
        let s = Scheduler::new(store);
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn due_indices_first_run_due_for_all() {
        let store = Arc::new(Mutex::new(CacheStore::open_in_memory().unwrap()));
        let mut s = Scheduler::new(store);
        let counter = Box::leak(Box::new(std::sync::atomic::AtomicUsize::new(0)));
        s.register(counting_source("a", Interval::Test, counter));
        s.register(counting_source("b", Interval::Slow, counter));
        assert_eq!(s.due_indices(now_secs()).len(), 2);
    }

    #[test]
    fn tick_runs_due_sources_and_writes_cache() {
        let store = Arc::new(Mutex::new(CacheStore::open_in_memory().unwrap()));
        let mut s = Scheduler::new(store.clone());
        let counter = Box::leak(Box::new(std::sync::atomic::AtomicUsize::new(0)));
        s.register(counting_source("json_banks", Interval::Test, counter));

        let outcomes = s.tick(&dummy_root(), now_secs());
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].source_id, "json_banks");
        assert_eq!(outcomes[0].banks, 1);
        assert_eq!(outcomes[0].questions, 0);
        assert_eq!(outcomes[0].status, CacheStatus::Fresh);
        assert_eq!(outcomes[0].error, None);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);

        let cached = store
            .lock()
            .expect("cache mutex poisoned")
            .get("json_banks")
            .unwrap()
            .unwrap();
        assert_eq!(cached.banks, 1);
        assert_eq!(cached.status, CacheStatus::Fresh);
    }

    #[test]
    fn due_indices_skips_fresh_source_within_ttl() {
        let store = Arc::new(Mutex::new(CacheStore::open_in_memory().unwrap()));
        let mut s = Scheduler::new(store.clone());
        let counter = Box::leak(Box::new(std::sync::atomic::AtomicUsize::new(0)));
        s.register(counting_source("slow_source", Interval::Slow, counter));

        // 先 tick 一次,记录 last_run + cache
        s.tick(&dummy_root(), now_secs());
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);

        // 立刻再 due_indices → 应该为空(Fresh,last_run 也是刚才)
        let due = s.due_indices(now_secs());
        assert!(due.is_empty(), "fresh source should not be due");
    }

    #[test]
    fn due_indices_due_after_ttl_elapsed() {
        let store = Arc::new(Mutex::new(CacheStore::open_in_memory().unwrap()));
        let mut s = Scheduler::new(store.clone());
        let counter = Box::leak(Box::new(std::sync::atomic::AtomicUsize::new(0)));
        s.register(counting_source("test_src", Interval::Test, counter));

        // 注入一个 Expired cache entry,模拟 1 小时前
        store
            .lock()
            .expect("cache mutex poisoned")
            .upsert(&CacheEntry {
                source_id: "test_src".into(),
                fetched_at: now_secs() - 7200,
                ttl_secs: 60,
                status: CacheStatus::Expired,
                banks: 0,
                questions: 0,
                notes: None,
            })
            .unwrap();
        let due = s.due_indices(now_secs());
        assert_eq!(due.len(), 1, "expired cache should trigger refresh");
    }

    #[test]
    fn tick_records_error_in_cache_notes() {
        let store = Arc::new(Mutex::new(CacheStore::open_in_memory().unwrap()));
        let mut s = Scheduler::new(store.clone());

        // Failing source: load 返回错误
        struct FailSource;
        impl Source for FailSource {
            fn id(&self) -> &'static str { "fail" }
            fn display_name(&self) -> &'static str { "Fail" }
            fn interval(&self) -> Interval { Interval::Test }
            fn load(&self, _: &Path) -> crate::Result<Vec<LoadResult>> {
                Err(EngineError::Other("simulated io failure".into()))
            }
        }
        s.register(&FailSource);

        let outcomes = s.tick(&dummy_root(), now_secs());
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].error.is_some());
        assert_eq!(outcomes[0].status, CacheStatus::Expired);

        let cached = store
            .lock()
            .expect("cache mutex poisoned")
            .get("fail")
            .unwrap()
            .unwrap();
        assert_eq!(cached.status, CacheStatus::Expired);
        assert!(cached.notes.unwrap().contains("simulated"));
    }

    /// Phase D 计划验收:单 source 每 2s 触发一次集成测试绿。
    /// 用 Interval::Test (1s) + run_loop_for 2500ms → 期望 ≥ 2 次触发。
    #[test]
    fn run_loop_for_fires_at_least_twice_within_2_5s() {
        let store = Arc::new(Mutex::new(CacheStore::open_in_memory().unwrap()));
        let mut s = Scheduler::new(store).with_config(SchedulerConfig {
            tick_resolution: Duration::from_millis(200),
        });
        let counter = Box::leak(Box::new(std::sync::atomic::AtomicUsize::new(0)));
        s.register(counting_source("live", Interval::Test, counter));

        let stop = Arc::new(AtomicBool::new(false));
        let outcomes = s.run_loop_for(
            &dummy_root(),
            Duration::from_millis(2500),
            stop.clone(),
        );

        let fires = counter.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            fires >= 2,
            "Interval::Test should fire ≥ 2 times in 2.5s (got {fires})"
        );
        assert!(outcomes.len() >= fires);
    }

    /// 验证 stop 信号能中断 run_loop。
    #[test]
    fn run_loop_for_respects_stop_signal() {
        let store = Arc::new(Mutex::new(CacheStore::open_in_memory().unwrap()));
        let mut s = Scheduler::new(store);
        let counter = Box::leak(Box::new(std::sync::atomic::AtomicUsize::new(0)));
        s.register(counting_source("stoppable", Interval::Test, counter));

        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            stop_clone.store(true, Ordering::Relaxed);
        });

        let start = Instant::now();
        let _ = s.run_loop_for(&dummy_root(), Duration::from_secs(10), stop);
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "stop signal should interrupt within ~600ms (elapsed {elapsed:?})"
        );
    }

    /// 失败 source 在 backoff 窗口内不应被反复 load(防雪崩,R-007 配套)。
    ///
    /// 场景:`CountingFailSource` 永远 Err;连续 3 次 tick(间隔 < backoff 窗口)→ `load`
    /// 只能调 1 次(首次失败后进入退避,后续 tick 跳过)。当前实现每次 tick 都重试
    /// (counter=3),故此测试应 FAIL 直到 backoff 落地。
    #[test]
    fn tick_does_not_retry_failed_source_within_backoff_window() {
        let store = Arc::new(Mutex::new(CacheStore::open_in_memory().unwrap()));
        let mut s = Scheduler::new(store);

        // 用 AtomicUsize 计数 load 调用次数;leak source 以满足 `&'static dyn Source`
        // (与 counting_source() 同样套路)
        let load_count = Box::leak(Box::new(std::sync::atomic::AtomicUsize::new(0)));
        struct CountingFailSource {
            counter: &'static std::sync::atomic::AtomicUsize,
        }
        impl Source for CountingFailSource {
            fn id(&self) -> &'static str { "fail_counted" }
            fn display_name(&self) -> &'static str { "FailCounted" }
            fn interval(&self) -> Interval { Interval::Test }
            fn load(&self, _: &Path) -> crate::Result<Vec<LoadResult>> {
                self.counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(EngineError::Other("simulated".into()))
            }
        }
        let fail_src: &'static CountingFailSource =
            Box::leak(Box::new(CountingFailSource { counter: load_count }));
        s.register(fail_src);

        // Tick #1: 首次失败,应触发 1 次 load
        let t0 = now_secs();
        s.tick(&dummy_root(), t0);
        assert_eq!(
            load_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "first tick must trigger exactly 1 load"
        );

        // Tick #2 / #3: 在 backoff 窗口内,load_count 应保持 1
        s.tick(&dummy_root(), t0 + 1);
        s.tick(&dummy_root(), t0 + 2);

        assert_eq!(
            load_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "consecutive ticks within backoff window must NOT retry failed source"
        );
    }

    /// 失败 source 退避窗口过期后应被重新尝试(不会永久冻结)。
    #[test]
    fn tick_retries_failed_source_after_backoff_window_expires() {
        let store = Arc::new(Mutex::new(CacheStore::open_in_memory().unwrap()));
        let mut s = Scheduler::new(store);

        let load_count = Box::leak(Box::new(std::sync::atomic::AtomicUsize::new(0)));
        struct CountingFailSource {
            counter: &'static std::sync::atomic::AtomicUsize,
        }
        impl Source for CountingFailSource {
            fn id(&self) -> &'static str { "fail_expire" }
            fn display_name(&self) -> &'static str { "FailExpire" }
            fn interval(&self) -> Interval { Interval::Test }
            fn load(&self, _: &Path) -> crate::Result<Vec<LoadResult>> {
                self.counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(EngineError::Other("simulated".into()))
            }
        }
        let fail_src: &'static CountingFailSource =
            Box::leak(Box::new(CountingFailSource { counter: load_count }));
        s.register(fail_src);

        let t0 = now_secs();
        s.tick(&dummy_root(), t0);
        assert_eq!(load_count.load(std::sync::atomic::Ordering::SeqCst), 1);

        // 退避窗口过期后(> BASE_BACKOFF_SECS=5)再 tick → 应重新尝试
        s.tick(&dummy_root(), t0 + 6);
        assert_eq!(
            load_count.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "tick after backoff window should retry the failed source"
        );
    }

    /// 成功 load 立即清除退避状态(attempts 归零、next_retry_at 清空)。
    #[test]
    fn tick_success_clears_backoff_state() {
        let store = Arc::new(Mutex::new(CacheStore::open_in_memory().unwrap()));
        let mut s = Scheduler::new(store);

        // 注册一个会"先失败后成功"的 source:用 AtomicBool 控制
        let mode = Box::leak(Box::new(std::sync::atomic::AtomicBool::new(false)));
        struct FlakySource {
            mode: &'static std::sync::atomic::AtomicBool,
        }
        impl Source for FlakySource {
            fn id(&self) -> &'static str { "flaky" }
            fn display_name(&self) -> &'static str { "Flaky" }
            fn interval(&self) -> Interval { Interval::Test }
            fn load(&self, _: &Path) -> crate::Result<Vec<LoadResult>> {
                if self.mode.load(std::sync::atomic::Ordering::SeqCst) {
                    Ok(vec![LoadResult {
                        bank_id: crate::types::BankId::new("b1"),
                        meta: None,
                        questions: vec![],
                        cards: vec![],
                    }])
                } else {
                    Err(EngineError::Other("flaky fail".into()))
                }
            }
        }
        let flaky: &'static FlakySource = Box::leak(Box::new(FlakySource { mode }));
        s.register(flaky);

        let t0 = now_secs();
        s.tick(&dummy_root(), t0); // 失败 → attempts=1, next_retry_at=t0+5
        assert_eq!(s.entries[0].attempts, 1);
        assert!(s.entries[0].next_retry_at.is_some());

        // 切到 OK 模式 + 跳到 backoff 窗口外
        mode.store(true, std::sync::atomic::Ordering::SeqCst);
        s.tick(&dummy_root(), t0 + 6);
        assert_eq!(s.entries[0].attempts, 0, "success must reset attempts to 0");
        assert!(
            s.entries[0].next_retry_at.is_none(),
            "success must clear next_retry_at"
        );
    }

    /// 纯函数 `next_backoff_secs` 单测:验证指数递增 + MAX 封顶。
    #[test]
    fn next_backoff_secs_grows_then_caps() {
        assert_eq!(next_backoff_secs(1), 5);
        assert_eq!(next_backoff_secs(2), 10);
        assert_eq!(next_backoff_secs(3), 20);
        assert_eq!(next_backoff_secs(4), 40);
        // 7 次失败 = 5 * 2^6 = 320,但应被 MAX_BACKOFF_SECS=300 封顶
        assert_eq!(next_backoff_secs(7), 300);
        // 极端 attempts(>20) 不应 overflow
        assert_eq!(next_backoff_secs(1000), 300);
    }
}
