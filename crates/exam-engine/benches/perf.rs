//! Performance baselines — exam-engine core paths.
//!
//! Run: `cargo bench --bench perf`
//!
//! Six benches lock the regression line:
//!   1. build_index         — IndexBuilder over banks + crawl fixtures
//!   2. search_fts          — FTS5 bm25 query (single CJK keyword)
//!   3. search_like         — LIKE fallback path (multi-CJK phrase OR-join)
//!   4. search_cards        — card FTS5 query
//!   5. load_json_banks     — JsonBanksSource::load
//!   6. load_crawl_cards    — CrawlCardsSource::load (问答 split)

use std::fs;
use std::path::Path;

use divan::Bencher;
use exam_engine::source::LoadResult;
use exam_engine::source::crawl_cards::CrawlCardsSource;
use exam_engine::source::json_banks::JsonBanksSource;
use exam_engine::{
    IndexBuilder, SearchQuery, Source, open_readonly, search, search_cards,
};
use tempfile::TempDir;

// -------- fixture builder --------

const BANK_QUESTION_COUNT: usize = 200;
const CARD_QUESTION_COUNT: usize = 200;
const CJK_BANK_TITLE: &str = "高频考点综合题";
const CJK_CARD_TITLE: &str = "考点解析-问答";

fn write_bank_json(dir: &Path) {
    let banks = dir.join("banks");
    fs::create_dir_all(&banks).unwrap();
    let path = banks.join("demo.json");

    let mut questions = Vec::with_capacity(BANK_QUESTION_COUNT);
    for i in 0..BANK_QUESTION_COUNT {
        questions.push(format!(
            r#"{{"id":"q{i:04}","type":"single","typeLabel":"单选","question":"高频考点 {i} 安全生产 高空作业 现场处置","options":[{{"key":"A","text":"正确"}},{{"key":"B","text":"错误"}},{{"key":"C","text":"不适用"}},{{"key":"D","text":"待定"}}],"answerKeys":["A"],"answerText":["正确"],"topic":"第{i}章 高空作业","srcBank":"综合维护"}}"#
        ));
    }
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
        let db_path: &'static Path =
            Box::leak(root.join("index.sqlite").into_boxed_path());
        write_bank_json(root);
        write_crawl_json(root);
        Self { root, db_path, _tmp: tmp }
    }
}

// -------- benches --------
// All benches run single-threaded (`threads = 1`) because rusqlite::Connection
// is `!Send` — sending it across divan's default thread pool triggers
// `RefCell<...> cannot be shared between threads safely`. Single-thread is
// also the realistic baseline: SQL is serialised under one writer anyway.

#[divan::bench(threads = 1)]
fn build_index(bencher: Bencher) {
    let fx = Fixture::new();
    bencher.bench(|| {
        let mut builder = IndexBuilder::open(fx.db_path).unwrap();
        divan::black_box(builder.rebuild(fx.root).unwrap())
    });
}

#[divan::bench(threads = 1)]
fn search_fts(bencher: Bencher) {
    let fx = Fixture::new();
    // one-time build before timing starts
    let mut builder = IndexBuilder::open(fx.db_path).unwrap();
    let _ = builder.rebuild(fx.root).unwrap();
    let q = SearchQuery::new("高空作业");
    bencher.bench(|| {
        // open conn per iteration — `Connection` is `!Send`, can't be captured
        // across divan's closure boundary. Cost is amortised: rusqlite open is
        // ~5 µs on macOS, dwarfed by FTS5 query itself (~hundreds of µs).
        let conn = open_readonly(fx.db_path).unwrap();
        divan::black_box(search(&conn, &q).unwrap())
    });
}

#[divan::bench(threads = 1)]
fn search_like(bencher: Bencher) {
    let fx = Fixture::new();
    let mut builder = IndexBuilder::open(fx.db_path).unwrap();
    let _ = builder.rebuild(fx.root).unwrap();
    // multi-CJK phrase — LIKE fallback path OR-joins single chars
    let q = SearchQuery::new("高空作业现场处置");
    bencher.bench(|| {
        let conn = open_readonly(fx.db_path).unwrap();
        divan::black_box(search(&conn, &q).unwrap())
    });
}

#[divan::bench(threads = 1)]
fn search_cards_fts(bencher: Bencher) {
    let fx = Fixture::new();
    let mut builder = IndexBuilder::open(fx.db_path).unwrap();
    let _ = builder.rebuild(fx.root).unwrap();
    let q = SearchQuery::new("高风险");
    bencher.bench(|| {
        let conn = open_readonly(fx.db_path).unwrap();
        divan::black_box(search_cards(&conn, &q).unwrap())
    });
}

#[divan::bench(threads = 1)]
fn load_json_banks(bencher: Bencher) {
    let fx = Fixture::new();
    bencher.bench(|| {
        let src = JsonBanksSource;
        let results: Vec<LoadResult> = src.load(fx.root).unwrap();
        divan::black_box(results)
    });
}

#[divan::bench(threads = 1)]
fn load_crawl_cards(bencher: Bencher) {
    let fx = Fixture::new();
    bencher.bench(|| {
        let src = CrawlCardsSource;
        let results: Vec<LoadResult> = src.load(fx.root).unwrap();
        divan::black_box(results)
    });
}

fn main() {
    divan::main();
}
