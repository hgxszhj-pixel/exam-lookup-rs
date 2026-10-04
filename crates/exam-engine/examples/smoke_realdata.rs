//! 真实数据 smoke test — Phase H E2E 验证。
//!
//! 用法:
//!   cargo run --example smoke_realdata -- \
//!     /Users/hgxszhj/x_videos-dowloads/exam-lookup \
//!     /tmp/exam-lookup-rs-smoke.db
//!
//! 无参数 → 默认上面两条路径。
//!
//! 流程:rebuild → 打印 → 跑 3 条样本查询 → 退出码 0/1。

use std::env;
use std::path::PathBuf;
use std::time::Instant;

use exam_engine::index::IndexBuilder;
use exam_engine::search::{SearchQuery, search, search_cards};

fn main() {
    let args: Vec<String> = env::args().collect();
    let data_root = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/Users/hgxszhj/x_videos-dowloads/exam-lookup"));
    let db_path = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/exam-lookup-rs-smoke.db"));

    eprintln!("[smoke] data_root = {}", data_root.display());
    eprintln!("[smoke] db_path   = {}", db_path.display());

    // 1. rebuild
    let t0 = Instant::now();
    let mut builder = IndexBuilder::open(&db_path).expect("open IndexBuilder");
    let stats = builder
        .rebuild(&data_root)
        .expect("rebuild against real data_root");
    let dt = t0.elapsed();
    eprintln!(
        "[smoke] rebuild OK in {:.2}s — banks={} questions={} cards={} errors={}",
        dt.as_secs_f64(),
        stats.banks,
        stats.questions,
        stats.cards,
        stats.errors
    );

    if stats.questions == 0 {
        eprintln!("[smoke] FAIL — 0 questions parsed (data_root 路径或 banks 格式异常?)");
        std::process::exit(1);
    }

    // 2. 用 builder.connection() 跑查询(已持有 conn,免再 open)
    let conn = builder.connection();

    let samples: &[(&str, &str)] = &[
        ("ask_关键词_中文", "保密"),
        ("ask_关键词_英文", "security"),
        ("cards_关键词",    "安全"),
    ];

    let mut all_ok = true;
    for (tag, kw) in samples {
        let q = SearchQuery::new(*kw).with_limit(3);
        let t = Instant::now();
        match search(conn, &q) {
            Ok(hits) => {
                eprintln!(
                    "[smoke] {tag} kw={kw:?} hits={} ({:.0}ms)",
                    hits.len(),
                    t.elapsed().as_secs_f64() * 1000.0
                );
                for (i, h) in hits.iter().take(3).enumerate() {
                    let stem: String = h.question.chars().take(40).collect();
                    eprintln!(
                        "    #{i} id={} src={} score={:.2} stem={stem}…",
                        h.id, h.src, h.score
                    );
                }
            }
            Err(e) => {
                eprintln!("[smoke] {tag} FAIL: {e:?}");
                all_ok = false;
            }
        }
    }

    // cards 路径
    let q = SearchQuery::new("安全").with_limit(2);
    match search_cards(conn, &q) {
        Ok(cards) => {
            eprintln!("[smoke] cards hits={}", cards.len());
            for c in cards.iter().take(2) {
                let title: String = c.title.chars().take(30).collect();
                eprintln!("    card id={} title={title}…", c.id);
            }
        }
        Err(e) => {
            eprintln!("[smoke] cards FAIL: {e:?}");
            all_ok = false;
        }
    }

    if all_ok {
        eprintln!("[smoke] PASS");
        std::process::exit(0);
    } else {
        eprintln!("[smoke] FAIL — 见上方错误");
        std::process::exit(1);
    }
}