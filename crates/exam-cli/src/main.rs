//! exam-cli:题库导出 / 校验 / 统计工具(Phase A 骨架 + Phase C 索引/搜索)。
//!
//! 子命令:
//! - `sources`        :列出已注册的 source
//! - `load`           :从 data_root 加载并打印摘要
//! - `index`          :把 source 数据写入 SQLite(FTS5 索引)
//! - `search`         :在 SQLite 上跑查询
//! - `dump`           :导出 source 数据为 JSON(对照 Python `qbank.py::dump` 做 parity)

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use exam_engine::{
    all_sources, find_source, parse, search as engine_search, CacheStore, EngineError,
    IndexBuilder, Scheduler, SearchQuery,
};
use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "exam-cli", version, about = "exam-lookup-rs 命令行工具")]
struct Cli {
    #[arg(long, global = true, value_name = "DIR")]
    data_root: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 列出已注册的 source。
    Sources,

    /// 从 data_root 加载指定 source 并打印 bank 元信息摘要。
    Load {
        #[arg(long)]
        source: String,
        #[arg(long)]
        filter: Option<String>,
    },

    /// 构建 SQLite 索引(FTS5 + CJK 字切分)。
    Index {
        /// 索引输出路径,默认 `questions.db`
        #[arg(long, default_value = "questions.db")]
        out: PathBuf,
        /// 强制全量重建
        #[arg(long)]
        rebuild: bool,
    },

    /// 在 SQLite 索引上搜索。
    Search {
        /// 索引文件路径
        #[arg(long, default_value = "questions.db")]
        db: PathBuf,
        /// 查询文本
        query: String,
        /// 限定 bank(src)过滤
        #[arg(long)]
        bank: Option<String>,
        /// 返回数量上限
        #[arg(long, default_value_t = 16)]
        limit: usize,
        /// 输出 JSON 格式
        #[arg(long)]
        json: bool,
    },

    /// 把 source 数据 dump 成 JSON(对齐 Python `qbank.py::dump`)— parity 比对用。
    Dump {
        /// 目标 JSON 文件
        out: PathBuf,
    },

    /// 列出 cache.db 中所有 source 的缓存条目。
    Cache {
        /// cache.db 路径
        #[arg(long, default_value = "cache.db")]
        db: PathBuf,
    },

    /// 单次执行 scheduler tick(只跑 due sources,不进入循环)。
    Tick {
        /// cache.db 路径
        #[arg(long, default_value = "cache.db")]
        cache: PathBuf,
    },

    /// 跑 scheduler 循环(默认 60s,可被 SIGINT 提前终止)— Phase D 演示用。
    Run {
        /// cache.db 路径
        #[arg(long, default_value = "cache.db")]
        cache: PathBuf,
        /// 持续秒数
        #[arg(long, default_value_t = 60)]
        seconds: u64,
    },
}

fn main() -> Result<()> {
    // Step 1.1 (2026-10-12): initialize tracing subscriber with RUST_LOG filter.
    // `.ok()` swallows "already initialized" errors (e.g. test harnesses).
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init()
        .ok();

    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Sources => list_sources(),
        Cmd::Load { source, filter } => {
            let data_root = resolve_data_root(&cli.data_root, "load")?;
            load_source(&source, &data_root, filter.as_deref())
        }
        Cmd::Index { out, rebuild } => {
            let data_root = resolve_data_root(&cli.data_root, "index")?;
            build_index(&data_root, &out, rebuild)
        }
        Cmd::Search {
            db,
            query,
            bank,
            limit,
            json,
        } => run_search(&db, &query, bank.as_deref(), limit, json),
        Cmd::Dump { out } => {
            let data_root = resolve_data_root(&cli.data_root, "dump")?;
            run_dump(&data_root, &out)
        }
        Cmd::Cache { db } => run_cache_list(&db),
        Cmd::Tick { cache } => {
            let data_root = resolve_data_root(&cli.data_root, "tick")?;
            run_tick(&cache, &data_root)
        }
        Cmd::Run { cache, seconds } => {
            let data_root = resolve_data_root(&cli.data_root, "run")?;
            run_scheduler(&cache, &data_root, seconds)
        }
    }
}

/// Step 1.4: 校验 `--data-root` 存在 + canonicalize。
///
/// 返回绝对路径,失败立刻报错(用户输入早 fail,不下到 engine 才暴露)。
fn resolve_data_root(opt: &Option<PathBuf>, sub: &str) -> Result<PathBuf> {
    let p = opt
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("--data-root <DIR> is required for `{sub}`"))?;
    exam_engine::source::canonicalize_path(p)
        .with_context(|| format!("--data-root 不可访问: {}", p.display()))
}

fn list_sources() -> Result<()> {
    let sources = all_sources();
    println!("已注册 source ({}):", sources.len());
    for s in sources {
        println!(
            "  - {:<20} {:<20} interval={}",
            s.id(),
            s.display_name(),
            s.interval()
        );
    }
    Ok(())
}

fn load_source(source_id: &str, data_root: &Path, filter: Option<&str>) -> Result<()> {
    let src = find_source(source_id)
        .ok_or_else(|| anyhow::anyhow!(EngineError::SourceNotFound(source_id.to_string())))?;

    let filter_lower = filter.map(|f| f.to_ascii_lowercase());
    let results = src.load(data_root)?;

    println!(
        "{}: 加载 {} 个 bank(来自 {})",
        src.id(),
        results.len(),
        data_root.display()
    );

    let mut total = 0usize;
    for r in &results {
        let bank_str = r.bank_id.as_str();
        let title = r.meta.as_ref().map(|m| m.title.as_str()).unwrap_or("");
        let hit = filter_lower
            .as_ref()
            .map(|f| {
                bank_str.to_ascii_lowercase().contains(f)
                    || title.to_ascii_lowercase().contains(f)
            })
            .unwrap_or(true);

        if !hit {
            continue;
        }

        println!(
            "  - {:<30} {:<10} title={}",
            bank_str,
            format!("{} 题", r.questions.len()),
            title
        );
        total += r.questions.len();
    }

    println!("\n合计 {} 题(过滤后)", total);
    Ok(())
}

fn build_index(data_root: &Path, out: &Path, rebuild: bool) -> Result<()> {
    if out.exists() && !rebuild {
        return Err(anyhow::anyhow!(
            "{} 已存在,需要 --rebuild 才能覆盖",
            out.display()
        ));
    }
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).context("create db parent dir")?;
        }
    }
    let mut builder = IndexBuilder::open(out)?;
    let stats = builder.rebuild(data_root)?;
    println!(
        "已构建索引:{} ({} bank, {} 题, {} 错误)",
        out.display(),
        stats.banks,
        stats.questions,
        stats.errors
    );
    Ok(())
}

fn run_search(
    db: &Path,
    query: &str,
    bank: Option<&str>,
    limit: usize,
    as_json: bool,
) -> Result<()> {
    // Step 1.2: defense-in-depth query cap (engine also caps, but reject early with
    // a clear CLI-side error so users see what changed).
    if query.len() > exam_engine::search::MAX_QUERY_BYTES {
        return Err(anyhow::anyhow!(
            "query 太长:{} 字节 (上限 {})",
            query.len(),
            exam_engine::search::MAX_QUERY_BYTES
        ));
    }
    if !db.exists() {
        return Err(anyhow::anyhow!(
            "DB 不存在:{} — 先跑 `exam-cli index --data-root ...`",
            db.display()
        ));
    }
    let conn = rusqlite::Connection::open(db).context("open db")?;
    let q = SearchQuery::new(query).with_limit(limit);
    let q = match bank {
        Some(b) => q.with_bank(b),
        None => q,
    };
    let hits = engine_search::search(&conn, &q)?;
    if as_json {
        let json = serde_json::to_string_pretty(&hits).context("serialize hits")?;
        println!("{}", json);
    } else {
        if hits.is_empty() {
            println!("(无命中)");
            return Ok(());
        }
        for h in &hits {
            let snippet: String = h.question.chars().take(60).collect();
            println!(
                "  [{:?}] {:>5.3} {:<40} {:<30} A={}",
                h.match_kind, h.score, h.src, snippet, h.answer
            );
        }
        println!("\n{} hits", hits.len());
    }
    Ok(())
}

fn run_dump(data_root: &Path, out: &Path) -> Result<()> {
    let sources = all_sources();
    if sources.is_empty() {
        return Err(anyhow::anyhow!("no source registered"));
    }
    let mut dump = serde_json::Map::new();
    let mut total_q = 0usize;
    let src_count = sources.len();
    for s in &sources {
        let results = s.load(data_root)?;
        let mut banks = serde_json::Map::new();
        for r in results {
            let entry = serde_json::json!({
                "meta": r.meta,
                "questions": r.questions.iter().map(|q| {
                    serde_json::json!({
                        "id": q.id,
                        "number": q.number,
                        "year": q.year,
                        "type": q.qtype.as_str(),
                        "typeLabel": q.type_label,
                        "question": q.stem,
                        "nq": parse::norm(&q.stem),
                        "nqstem": parse::norm(&parse::stem_of(&q.stem)),
                        "options": q.options.iter().map(|o| serde_json::json!({"key": o.key, "text": o.text})).collect::<Vec<_>>(),
                        "answerKeys": q.answer_keys,
                        "answerText": parse::join_answer_text(&q.answer_text),
                        "analysis": q.analysis,
                        "topic": q.topic,
                        "srcBank": q.src_bank,
                    })
                }).collect::<Vec<_>>(),
            });
            banks.insert(r.bank_id.as_str().to_string(), entry);
            total_q += r.questions.len();
        }
        dump.insert(s.id().to_string(), serde_json::Value::Object(banks));
    }
    dump.insert(
        "_meta".to_string(),
        serde_json::json!({"total_questions": total_q, "schema": "v1"}),
    );

    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).context("create dump parent dir")?;
        }
    }
    let json = serde_json::to_string_pretty(&dump).context("serialize dump")?;
    std::fs::write(out, json).context("write dump")?;
    println!(
        "dump 写入 {} ({} 题, 来自 {} sources)",
        out.display(),
        total_q,
        src_count
    );
    Ok(())
}

fn run_cache_list(db: &Path) -> Result<()> {
    if !db.exists() {
        return Err(anyhow::anyhow!(
            "cache.db 不存在:{} — 先跑 `exam-cli tick` 或 `run`",
            db.display()
        ));
    }
    let store = CacheStore::open(db)?;
    let entries = store.list()?;
    if entries.is_empty() {
        println!("(cache 空)");
        return Ok(());
    }
    println!(
        "{:<20} {:<10} {:<8} {:>5} {:>6} fetched_at",
        "source_id", "status", "ttl", "banks", "qs"
    );
    for e in &entries {
        println!(
            "{:<20} {:<10} {:<8} {:>5} {:>6} {}",
            e.source_id,
            e.status,
            format!("{ttl_secs}s", ttl_secs = e.ttl_secs),
            e.banks,
            e.questions,
            e.fetched_at
        );
        if let Some(n) = &e.notes {
            println!("    notes: {n}");
        }
    }
    Ok(())
}

fn build_scheduler(cache: &Path) -> Result<Scheduler> {
    if let Some(parent) = cache.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).context("create cache parent dir")?;
        }
    }
    let store = std::sync::Arc::new(std::sync::Mutex::new(CacheStore::open(cache)?));
    Ok(Scheduler::new(store))
}

fn run_tick(cache: &Path, data_root: &Path) -> Result<()> {
    let mut sched = build_scheduler(cache)?;
    for s in all_sources() {
        sched.register(s);
    }
    let now = exam_engine::now_secs();
    let outcomes = sched.tick(data_root, now);
    if outcomes.is_empty() {
        println!("(no sources due)");
        return Ok(());
    }
    println!("source_id           interval       status    banks    qs  skipped  error");
    for o in &outcomes {
        println!(
            "{:<19} {:<14} {:<9} {:>5} {:>5} {:<8} {}",
            o.source_id,
            o.interval,
            o.status,
            o.banks,
            o.questions,
            o.skipped,
            o.error.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

fn run_scheduler(cache: &Path, data_root: &Path, seconds: u64) -> Result<()> {
    let mut sched = build_scheduler(cache)?;
    for s in all_sources() {
        sched.register(s);
    }
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_clone = stop.clone();
    // 简易 SIGINT 等价:另一个 thread 等 N 秒后置 stop
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        stop_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let outcomes = sched.run_loop_for(
        data_root,
        std::time::Duration::from_secs(seconds),
        stop,
    );
    println!("run 完成:{} 次 tick outcome", outcomes.len());
    Ok(())
}
