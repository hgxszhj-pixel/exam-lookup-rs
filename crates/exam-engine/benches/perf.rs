//! Performance baselines **+ 可执行的 perf 预算门禁**。
//!
//! Run: `cargo bench --bench perf`
//!
//! Six benches lock the regression line:
//!   1. build_index         — IndexBuilder over banks + crawl fixtures
//!   2. search_fts          — FTS5 bm25 query (single CJK keyword)
//!   3. search_like         — LIKE fallback path (全表 `LIKE '%needle%'`)
//!   4. search_cards_fts    — card FTS5 query
//!   5. load_json_banks     — JsonBanksSource::load
//!   6. load_crawl_cards    — CrawlCardsSource::load (问答 split)
//!
//! **每个 bench 除了出数字,还过一道预算门禁**(阈值见
//! `exam_engine::perf_budget`,来源 `docs/reports/R-PERF.md`):
//! 实测耗时(P25)超上限 → 门禁判红 → **进程退出码 1**。
//!
//! 2026-11-04 审计 P2-6:此前这 6 个 bench **零断言**,R-PERF.md 的阈值只活在
//! Markdown 里 —— 一道永远不会触发的假门禁。
//!
//! **三条不能省的规矩**(都是踩过的坑):
//!
//! 1. **不能在 bench 里 panic** —— divan 0.1.21 会 `catch_unwind` 掉 bench 函数里
//!    的 panic 并继续跑下一个 bench,进程退出码**仍然是 0**
//!    (`divan/src/thread_pool.rs::broadcast_task`:catch 到的 `Err` 被直接 `drop`)。
//!    所以门禁用「记失败标志 + `main` 统一 `exit(1)`」,不用 panic。
//! 2. **每个 bench 要自证测的是它名字声称的那条路径** —— 否则数字在,门禁也在,
//!    量的却是别的东西(旧 `search_like` 就是这么名不副实的:多字 CJK 查询会被
//!    FTS 的「按字 OR-join」抢先命中,LIKE 兜底根本没跑)。
//! 3. **判定用 P25,不用中位数** —— 机器繁忙时整个分布会平移(实测中位在
//!    18.8~30.4 ms 之间跳,差 60%),P25 几乎不动(±3%)。中位量的是「机器当下
//!    多忙」,P25 量的是「这份代码能跑多快」。详见 `perf_budget::GATE_QUANTILE`。

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;
use std::time::Instant;

use divan::Bencher;
use exam_engine::perf_budget::{
    check, median, percentile, scale_from_env, PerfBudget, BUILD_INDEX, GATE_QUANTILE,
    LOAD_CRAWL_CARDS, LOAD_JSON_BANKS, SEARCH_CARDS_FTS, SEARCH_FTS, SEARCH_LIKE,
};
use exam_engine::source::crawl_cards::CrawlCardsSource;
use exam_engine::source::json_banks::JsonBanksSource;
use exam_engine::source::LoadResult;
use exam_engine::{
    open_readonly, search, search_cards, BuildStats, CardHit, IndexBuilder, MatchKind, SearchHit,
    SearchQuery, Source,
};
use tempfile::TempDir;

// -------- fixture builder --------

const BANK_QUESTION_COUNT: usize = 200;
const CARD_QUESTION_COUNT: usize = 200;
const CJK_BANK_TITLE: &str = "高频考点综合题";
const CJK_CARD_TITLE: &str = "考点解析-问答";

/// 专供 LIKE 兜底路径命中的那道题。
///
/// 题干里的 `、` 是关键:`norm()` 会把标点删掉 → `nq` 里是连续的 "AB";
/// 而 FTS 侧 unicode61 按标点断开 → 索引里只有 "A" "B" 两个 token,没有 "AB"。
/// 于是查 "AB" 时 **FTS 必然落空**(OR-join 的每个单字 token 都命中不了),
/// 才会真正走到 LIKE 兜底。
const LATIN_TOKEN_QUESTION: &str = "作业许可A、B的办理流程，下列说法正确的是";

fn write_bank_json(dir: &Path) {
    let banks = dir.join("banks");
    fs::create_dir_all(&banks).unwrap();
    let path = banks.join("demo.json");

    let mut questions = Vec::with_capacity(BANK_QUESTION_COUNT + 1);
    for i in 0..BANK_QUESTION_COUNT {
        questions.push(format!(
            r#"{{"id":"q{i:04}","type":"single","typeLabel":"单选","question":"高频考点 {i} 安全生产 高空作业 现场处置","options":[{{"key":"A","text":"正确"}},{{"key":"B","text":"错误"}},{{"key":"C","text":"不适用"}},{{"key":"D","text":"待定"}}],"answerKeys":["A"],"answerText":["正确"],"topic":"第{i}章 高空作业","srcBank":"综合维护"}}"#
        ));
    }
    // 多一道带 ASCII 标点的题(见 `LATIN_TOKEN_QUESTION` 的注释)。
    questions.push(format!(
        r#"{{"id":"q-latin","type":"single","typeLabel":"单选","question":"{stem}","options":[{{"key":"A","text":"正确"}},{{"key":"B","text":"错误"}}],"answerKeys":["A"],"answerText":["正确"],"topic":"许可管理","srcBank":"综合维护"}}"#,
        stem = LATIN_TOKEN_QUESTION
    ));
    let body = questions.join(",\n");
    let file = format!(
        r#"{{"meta":{{"title":"{title}","sourceNote":"perf fixture","chapterCount":3}},"questions":[{body}]}}"#,
        title = CJK_BANK_TITLE
    );
    fs::write(&path, file).unwrap();
}

fn write_crawl_json(dir: &Path) {
    let crawl = dir.join("crawl");
    fs::create_dir_all(&crawl).unwrap();
    let path = crawl.join("demo-问答.json");

    let mut questions = Vec::with_capacity(CARD_QUESTION_COUNT);
    for i in 0..CARD_QUESTION_COUNT {
        questions.push(format!(
            r#"{{"chapter":"第{i}章 高空作业","num":{i},"page":null,"q":"高风险与特殊场景作业 {i} 是指什么？","a":"（一）高风险与特殊场景作业包括：高空作业 {i}、动火作业、受限空间作业。","_book":"中国电信综合维护及装维作业安全生产管理办法（试行）","_src":"语料库定义句","_tested":""}}"#
        ));
    }
    let body = questions.join(",\n");
    let file = format!(
        r#"{{"meta":{{"title":"{title}","sourceNote":"perf fixture"}},"questions":[{body}]}}"#,
        title = CJK_CARD_TITLE
    );
    fs::write(&path, file).unwrap();
}

struct Fixture {
    // `root` + `db_path` are leaked to 'static so they can be captured into
    // the `Send` closure passed to `bencher.bench(|| ...)`. The accompanying
    // TempDir is also leaked so the leaked paths stay valid for the whole
    // process. Bounded: benches only run during `cargo bench`.
    root: &'static Path,
    db_path: &'static Path,
    _tmp: &'static TempDir,
}

impl Fixture {
    fn new() -> Self {
        let tmp: &'static TempDir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let root: &'static Path = Box::leak(tmp.path().to_path_buf().into_boxed_path());
        let db_path: &'static Path = Box::leak(root.join("index.sqlite").into_boxed_path());
        write_bank_json(root);
        write_crawl_json(root);
        Self {
            root,
            db_path,
            _tmp: tmp,
        }
    }

    /// 建好索引(供 search bench 的一次性准备)。
    fn build(&self) {
        let mut builder = IndexBuilder::open(self.db_path).unwrap();
        builder.rebuild(self.root).unwrap();
    }
}

// -------- 被测操作 --------
//
// bench 与门禁**共用同一组函数** —— 门禁量的是「和 bench 同一个操作」,
// 不是另写一份「差不多」的代码(否则门禁和数字各说各话,又是一道假门禁)。

fn op_build_index(fx: &Fixture) -> BuildStats {
    let mut builder = IndexBuilder::open(fx.db_path).unwrap();
    builder.rebuild(fx.root).unwrap()
}

/// `Connection` 是 `!Send`,不能跨 divan 的线程边界捕获 → 每次调用现开一条。
/// 开销 ~5 µs,相对查询本身(~1.5 ms)可忽略。
fn op_search(fx: &Fixture, q: &SearchQuery) -> Vec<SearchHit> {
    let conn = open_readonly(fx.db_path).unwrap();
    search(&conn, q).unwrap()
}

fn op_search_cards(fx: &Fixture, q: &SearchQuery) -> Vec<CardHit> {
    let conn = open_readonly(fx.db_path).unwrap();
    search_cards(&conn, q).unwrap()
}

fn op_load_json_banks(root: &Path) -> Vec<LoadResult> {
    JsonBanksSource.load(root).unwrap().results
}

fn op_load_crawl_cards(root: &Path) -> Vec<LoadResult> {
    CrawlCardsSource.load(root).unwrap().results
}

// -------- 门禁 --------

/// 门禁是否已失败。
///
/// **为什么不用 panic**:divan 0.1.21 在 `thread_pool.rs::broadcast_task` 里
/// `catch_unwind` 掉 bench 函数的 panic,然后把 catch 到的 `Err` **直接 drop**
/// —— panic 不会传播,进程退出码仍是 0。用 panic 当门禁 = 又一道假门禁。
/// 所以这里只记标志位,由 `main` 统一决定退出码。
static GATE_FAILED: AtomicBool = AtomicBool::new(false);

/// 门禁采样次数(奇数 → 取真中位)。
///
/// 51 而不是 21:`build_index` 一次 ~20 ms,多采几次只多花 ~1 s,
/// 换来的是中位数明显更稳(这台机器上 21 次采样的 run 间中位能差 ±20%)。
const GATE_SAMPLES: usize = 51;
/// 预热次数(不计入中位)。
const GATE_WARMUP: usize = 3;

/// 一次测量的结果 —— P25(判定用)+ 中位/min/max(诊断用)。
///
/// **判定用 P25 而不是中位**:机器繁忙时整个分布会平移(实测中位在
/// 18.8~30.4 ms 之间跳,差 60%),而 P25 几乎不动(±3%)。
/// 中位量的是「机器当下多忙」,P25 量的是「这份代码能跑多快」——
/// 后者才是回归检测要的量。理由与数据见 `exam_engine::perf_budget::GATE_QUANTILE`。
#[derive(Debug, Clone, Copy)]
struct Measurement {
    p25: f64,
    median: f64,
    min: f64,
    max: f64,
}

/// 实测耗时(ms)。
fn measure_ms<T>(mut op: impl FnMut() -> T) -> Measurement {
    for _ in 0..GATE_WARMUP {
        divan::black_box(op());
    }
    let mut samples = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        let t0 = Instant::now();
        divan::black_box(op());
        samples.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("耗时不会是 NaN"));
    Measurement {
        p25: percentile(&samples, GATE_QUANTILE).expect("GATE_SAMPLES > 0 → 样本非空"),
        median: median(&samples).expect("GATE_SAMPLES > 0 → 样本非空"),
        min: sorted[0],
        max: sorted[sorted.len() - 1],
    }
}

/// 对一次账:超限 → 记失败(不 panic,理由见 `GATE_FAILED`)。
fn gate(budget: &PerfBudget, m: Measurement) {
    let scale = scale_from_env();
    let effective = budget.scaled(scale);
    match check(&effective, m.p25) {
        Ok(()) => println!(
            "  ✓ perf gate {:<18} p25 {:>8.3} ms (median {:>8.3} / min {:>7.3} / max {:>8.3}) ≤ {:>8.3} ms (scale {scale})",
            effective.name, m.p25, m.median, m.min, m.max, effective.limit_ms
        ),
        Err(e) => {
            eprintln!(
                "  ✗ perf gate {:<18} {e} (median {:.3} / min {:.3} / max {:.3})",
                e.name, m.median, m.min, m.max
            );
            GATE_FAILED.store(true, Ordering::Relaxed);
        }
    }
}

/// 记录一次「bench 没测到它声称的路径 / fixture 变形」类失败。
fn fail(msg: impl std::fmt::Display) {
    eprintln!("  ✗ {msg}");
    GATE_FAILED.store(true, Ordering::Relaxed);
}

// -------- benches --------
// All benches run single-threaded (`threads = 1`) because rusqlite::Connection
// is `!Send` — sending it across divan's default thread pool triggers
// `RefCell<...> cannot be shared between threads safely`. Single-thread is
// also the realistic baseline: SQL is serialised under one writer anyway.
//
// `Once` 保证每个门禁**恰好跑一次** —— 不依赖 divan 内部调几次 bench 函数。

static BUILD_INDEX_GATE: Once = Once::new();
static SEARCH_FTS_GATE: Once = Once::new();
static SEARCH_LIKE_GATE: Once = Once::new();
static SEARCH_CARDS_FTS_GATE: Once = Once::new();
static LOAD_JSON_BANKS_GATE: Once = Once::new();
static LOAD_CRAWL_CARDS_GATE: Once = Once::new();

#[divan::bench(threads = 1)]
fn build_index(bencher: Bencher) {
    let fx = Fixture::new();
    bencher.bench(|| divan::black_box(op_build_index(&fx)));
    BUILD_INDEX_GATE.call_once(|| {
        gate(&BUILD_INDEX, measure_ms(|| op_build_index(&fx)));
    });
}

#[divan::bench(threads = 1)]
fn search_fts(bencher: Bencher) {
    let fx = Fixture::new();
    // one-time build before timing starts
    fx.build();
    let q = SearchQuery::new("高空作业");
    bencher.bench(|| {
        // open conn per iteration — `Connection` is `!Send`, can't be captured
        // across divan's closure boundary. Cost is amortised: rusqlite open is
        // ~5 µs on macOS, dwarfed by FTS5 query itself (~hundreds of µs).
        divan::black_box(op_search(&fx, &q))
    });
    SEARCH_FTS_GATE.call_once(|| {
        // 自证路径:这个 bench 必须真的走 FTS —— 否则它量的和名字对不上。
        let hits = op_search(&fx, &q);
        if hits.is_empty() {
            fail("search_fts 没命中任何题 —— fixture 或索引坏了");
        } else if !hits.iter().all(|h| h.match_kind == MatchKind::Fts) {
            fail("search_fts 出现了非 FTS 命中 —— 这个 bench 量错了东西");
        }
        gate(&SEARCH_FTS, measure_ms(|| op_search(&fx, &q)));
    });
}

#[divan::bench(threads = 1)]
fn search_like(bencher: Bencher) {
    let fx = Fixture::new();
    fx.build();
    // LIKE 兜底路径专用查询:见 `LATIN_TOKEN_QUESTION` —— FTS 侧没有 "AB" token,
    // 查 "AB" 必然落到 LIKE 兜底。
    let q = SearchQuery::new("AB");
    bencher.bench(|| divan::black_box(op_search(&fx, &q)));
    SEARCH_LIKE_GATE.call_once(|| {
        // 自证路径:恰好命中那道带 'A、B' 的题,且 kind 是 Like。
        // (若 FTS 抢先命中,会返回一堆含 'A'/'B' 单字 token 的题,数量对不上。)
        let hits = op_search(&fx, &q);
        if hits.len() != 1 {
            fail(format!(
                "search_like 期望恰好 1 条 LIKE 兜底命中,实际 {} 条 —— 路径可能变了",
                hits.len()
            ));
        } else if hits[0].match_kind != MatchKind::Like {
            fail("search_like 的命中不是 MatchKind::Like —— 路径可能变了");
        } else if !hits[0].question.contains("A、B") {
            fail("search_like 命中的不是那道带 'A、B' 的题 —— fixture 变了");
        }
        gate(&SEARCH_LIKE, measure_ms(|| op_search(&fx, &q)));
    });
}

#[divan::bench(threads = 1)]
fn search_cards_fts(bencher: Bencher) {
    let fx = Fixture::new();
    fx.build();
    let q = SearchQuery::new("高风险");
    bencher.bench(|| divan::black_box(op_search_cards(&fx, &q)));
    SEARCH_CARDS_FTS_GATE.call_once(|| {
        let hits = op_search_cards(&fx, &q);
        if hits.is_empty() {
            fail("search_cards_fts 没命中任何卡片 —— fixture 或索引坏了");
        } else if !hits.iter().all(|h| h.match_kind == MatchKind::Fts) {
            fail("search_cards_fts 出现了非 FTS 命中 —— 这个 bench 量错了东西");
        }
        gate(&SEARCH_CARDS_FTS, measure_ms(|| op_search_cards(&fx, &q)));
    });
}

#[divan::bench(threads = 1)]
fn load_json_banks(bencher: Bencher) {
    let fx = Fixture::new();
    bencher.bench(|| divan::black_box(op_load_json_banks(fx.root)));
    LOAD_JSON_BANKS_GATE.call_once(|| {
        let results = op_load_json_banks(fx.root);
        if results.is_empty() {
            fail("load_json_banks 没加载到任何 bank —— fixture 坏了");
        }
        gate(&LOAD_JSON_BANKS, measure_ms(|| op_load_json_banks(fx.root)));
    });
}

#[divan::bench(threads = 1)]
fn load_crawl_cards(bencher: Bencher) {
    let fx = Fixture::new();
    bencher.bench(|| divan::black_box(op_load_crawl_cards(fx.root)));
    LOAD_CRAWL_CARDS_GATE.call_once(|| {
        let results = op_load_crawl_cards(fx.root);
        if results.is_empty() {
            fail("load_crawl_cards 没加载到任何问答文件 —— fixture 坏了");
        }
        gate(
            &LOAD_CRAWL_CARDS,
            measure_ms(|| op_load_crawl_cards(fx.root)),
        );
    });
}

fn main() {
    divan::main();
    if GATE_FAILED.load(Ordering::Relaxed) {
        eprintln!("\nperf 预算门禁未通过(见上 ✗ 行) —— 退出码 1");
        std::process::exit(1);
    }
}
