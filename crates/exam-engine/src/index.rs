//! JSON 题库 → SQLite 一次性构建(Phase C)。
//!
//! Schema 对齐 Python `solve.py::db_init`:
//! - `q(id TEXT PK, num INT, type TEXT, question TEXT, nq TEXT, nqstem TEXT,`
//!   `answer TEXT, answerText TEXT, analysis TEXT, src TEXT, chapter TEXT)`
//! - 索引:`idx_nq` on `q(nq)`
//! - 全文:`q_fts`(FTS5 virtual,content=`q`、tokenize=`unicode61 remove_diacritics 2`)
//!
//! **R-001 降级**:本实现不启用 trigram tokenizer(CJK trigram 需 SQLite 扩展,
//! `rusqlite` 的 `bundled` feature 不含);改用 `unicode61` + 字符 n-gram LIKE 兜底
//! (见 `search.rs`)。文档化与 Python `kb.py::docs_fts` 的差异。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags, Transaction};

use crate::error::{EngineError, Result};
use crate::parse::{cjk_expand, join_answer_text, norm, stem_of};
use crate::source::{all_sources, LoadIssue, LoadResult, Source};

/// 当前 schema 版本。
///
/// - v1/v2:初版 —— `q.id TEXT PRIMARY KEY` + 独立的 `q_fts(id UNINDEXED, …)` 表。
/// - **v3**:P1-1 —— `q` 改复合主键 `(src, id)`;`q_fts` 去掉 `id` 列,改为 **rowid 映射**
///   (`q_fts.rowid == q.rowid`),从结构上保证 FTS 与 `q` 一一对应。
///
/// 迁移入口:`IndexBuilder::init_schema`。判定依据是**实际表结构**而不是版本号 ——
/// Python 侧 `kb_db.py::ensure_schema` 踩过这个坑(「旧脚本会把 user_version 写成 2,
/// 而 questions 表并不存在,纯按版本号就会漏建」)。
const SCHEMA_VERSION: i32 = 3;

/// `q` 表 —— 主键 `(src, id)`。
///
/// **为什么是复合主键**:`id` 在跨 bank 时会重复(实测真实数据 1768 题里 129 个 id 撞车,
/// 如 `BM-a17e665c` 同时出现在多个 bank)。旧的 `id TEXT PRIMARY KEY` + `INSERT OR REPLACE`
/// 会**静默丢掉 129 题**。Python 侧契约同样是 `questions(bank, qid, PRIMARY KEY(bank, qid))`
/// (`kb_db.py:55-61`),这里对齐。
const CREATE_Q: &str = r#"
CREATE TABLE IF NOT EXISTS q (
    id          TEXT NOT NULL,
    num         INTEGER,
    type        TEXT,
    question    TEXT,
    nq          TEXT,
    nqstem      TEXT,
    answer      TEXT,
    answerText  TEXT,
    analysis    TEXT,
    src         TEXT NOT NULL,
    chapter     TEXT,
    PRIMARY KEY (src, id)
)"#;

/// 迁移期间用的临时表(结构同 `q`,表名不同,迁移完 rename 回去)。
const CREATE_Q_MIGRATE: &str = r#"
CREATE TABLE IF NOT EXISTS q_migrate (
    id          TEXT NOT NULL,
    num         INTEGER,
    type        TEXT,
    question    TEXT,
    nq          TEXT,
    nqstem      TEXT,
    answer      TEXT,
    answerText  TEXT,
    analysis    TEXT,
    src         TEXT NOT NULL,
    chapter     TEXT,
    PRIMARY KEY (src, id)
)"#;

const CREATE_IDX_NQ: &str = "CREATE INDEX IF NOT EXISTS idx_nq ON q(nq)";
/// `nqstem` 索引 —— 搜索第一步就是 `WHERE nqstem = ?`(对齐 Python `nqstem` 精确命中)。
/// 修复前这个查询每次都全表扫描(`EXPLAIN QUERY PLAN` → `SCAN q`),而 UI 每次按键都调。
const CREATE_IDX_NQSTEM: &str = "CREATE INDEX IF NOT EXISTS idx_nqstem ON q(nqstem)";
const CREATE_IDX_SRC: &str = "CREATE INDEX IF NOT EXISTS idx_src ON q(src)";

/// `q_fts` —— **只存被索引的文本**,不带 `id` 列。
///
/// `q_fts.rowid` 与 `q.rowid` 一一对应(写入时显式指定 rowid,见 `insert_load_result`),
/// 于是 `q_fts JOIN q ON q.rowid = q_fts.rowid` **结构上不可能扇出**。
///
/// 旧结构(`id UNINDEXED` + 普通 `INSERT`)有两个独立缺陷:
/// 1. `q` 用 `INSERT OR REPLACE` 去重、`q_fts` 用普通 `INSERT` 不去重 → 行数不等 →
///    join 一对多 → 同一条命中重复返回,`LIMIT 16` 实际只给 8 个不同问题。
/// 2. 同一个库调两次 `populate`(它是 public、注释写明「不清空,增量插入」)→ 同样重复。
const CREATE_Q_FTS: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS q_fts USING fts5(
    question,
    analysis,
    tokenize = 'unicode61 remove_diacritics 2'
)"#;

const CREATE_CARD: &str = r#"
CREATE TABLE IF NOT EXISTS card (
    id       TEXT NOT NULL,
    title    TEXT,
    body     TEXT,
    src      TEXT,
    tags     TEXT,
    PRIMARY KEY (src, id)
)"#;

/// `card_fts` —— 同 `q_fts`:`card_fts.rowid == card.rowid`。
const CREATE_CARD_FTS: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS card_fts USING fts5(
    title,
    body,
    tokenize = 'unicode61 remove_diacritics 2'
)"#;

const CREATE_META: &str = "CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT)";

/// 索引构建器 — `db_path` 一般落在 `~/Library/Application Support/exam-lookup-rs/questions.db`
/// (桌面端,Phase E 接入)。
pub struct IndexBuilder {
    conn: Connection,
    db_path: PathBuf,
}

impl IndexBuilder {
    pub fn open(db_path: impl Into<PathBuf>) -> Result<Self> {
        let db_path = db_path.into();
        let conn = Connection::open(&db_path)
            .map_err(|e| EngineError::other(format!("open {}: {e}", db_path.display())))?;
        // WAL 模式:并发读 + 单写;desktop GUI 多 Tab 友好
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| EngineError::other(format!("PRAGMA journal_mode: {e}")))?;
        Ok(Self { conn, db_path })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| EngineError::other(format!("open_in_memory: {e}")))?;
        Ok(Self {
            conn,
            db_path: PathBuf::from(":memory:"),
        })
    }

    /// 幂等创建表 / 索引 / FTS 虚拟表;必要时先把旧 schema 迁移到当前版本。
    ///
    /// P2-3:之前这里只有 `CREATE TABLE IF NOT EXISTS` —— 存量库永远停在旧结构,
    /// 代码里加了字段、库里没有,而且**没有任何升级手段**。现在:
    /// 1. 先建 `meta`(版本记录要用)
    /// 2. 按**实际表结构**判定要不要迁移(不是只看 `user_version`)
    /// 3. 幂等建表 / 建索引
    /// 4. 双写版本号:`PRAGMA user_version`(给 SQLite 工具看)+ `meta.schema_version`(给应用看)
    pub fn init_schema(&self) -> Result<()> {
        // 1. meta 表先建
        self.conn
            .execute_batch(CREATE_META)
            .map_err(|e| EngineError::other(format!("create meta: {e}")))?;

        // 2. 旧 schema → 当前 schema
        if self.needs_migration()? {
            self.migrate_to_current()?;
        }

        // 3. 幂等建表 / 建索引
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| EngineError::other(format!("begin tx: {e}")))?;
        tx.execute(CREATE_Q, [])
            .map_err(|e| EngineError::other(format!("create q: {e}")))?;
        tx.execute(CREATE_IDX_NQ, [])
            .map_err(|e| EngineError::other(format!("idx_nq: {e}")))?;
        tx.execute(CREATE_IDX_NQSTEM, [])
            .map_err(|e| EngineError::other(format!("idx_nqstem: {e}")))?;
        tx.execute(CREATE_IDX_SRC, [])
            .map_err(|e| EngineError::other(format!("idx_src: {e}")))?;
        tx.execute(CREATE_Q_FTS, [])
            .map_err(|e| EngineError::other(format!("create q_fts: {e}")))?;
        tx.execute(CREATE_CARD, [])
            .map_err(|e| EngineError::other(format!("create card: {e}")))?;
        tx.execute(CREATE_CARD_FTS, [])
            .map_err(|e| EngineError::other(format!("create card_fts: {e}")))?;
        upsert_meta(&tx, "schema_version", &SCHEMA_VERSION.to_string())?;
        tx.commit()
            .map_err(|e| EngineError::other(format!("commit init: {e}")))?;

        // 4. SQLite 原生版本标记
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|e| EngineError::other(format!("set user_version: {e}")))?;
        Ok(())
    }

    /// 存量库是否需要迁移到当前 schema。
    ///
    /// **判定依据是实际表结构,不是 `user_version`** —— Python 侧 `kb_db.py::ensure_schema`
    /// 的注释记着这个坑:「旧脚本会把 user_version 写成 2,而 questions 表并不存在,
    /// 纯按版本号就会漏建(踩过一次)」。所以这里每一项都直接看表结构。
    fn needs_migration(&self) -> Result<bool> {
        let conn = &self.conn;
        // q / card:主键必须是 (src, id)
        if table_exists(conn, "q")? && !pk_is(conn, "q", &["src", "id"])? {
            return Ok(true);
        }
        if table_exists(conn, "card")? && !pk_is(conn, "card", &["src", "id"])? {
            return Ok(true);
        }
        // q_fts / card_fts:旧结构带 `id` 列,新结构只索引文本列
        if table_exists(conn, "q_fts")? && has_column(conn, "q_fts", "id")? {
            return Ok(true);
        }
        if table_exists(conn, "card_fts")? && has_column(conn, "card_fts", "id")? {
            return Ok(true);
        }
        Ok(false)
    }

    /// 把存量库迁移到当前 schema(单事务)。
    fn migrate_to_current(&self) -> Result<()> {
        let from = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap_or(0);
        tracing::info!(from, to = SCHEMA_VERSION, "检测到旧 schema,开始迁移");
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| EngineError::other(format!("begin migrate tx: {e}")))?;
        migrate_q_table(&tx)?;
        migrate_card_table(&tx)?;
        rebuild_fts_tables(&tx)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|e| EngineError::other(format!("set user_version: {e}")))?;
        upsert_meta(&tx, "schema_version", &SCHEMA_VERSION.to_string())?;
        tx.commit()
            .map_err(|e| EngineError::other(format!("commit migrate: {e}")))?;
        tracing::info!(to = SCHEMA_VERSION, "schema 迁移完成");
        Ok(())
    }

    /// 清空 `q` + `q_fts` + `card` + `card_fts`(幂等重建的常规步骤)。
    pub fn truncate(&self) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| EngineError::other(format!("begin tx: {e}")))?;
        tx.execute("DELETE FROM q", [])
            .map_err(|e| EngineError::other(format!("delete q: {e}")))?;
        tx.execute("DELETE FROM q_fts", [])
            .map_err(|e| EngineError::other(format!("delete q_fts: {e}")))?;
        tx.execute("DELETE FROM card", [])
            .map_err(|e| EngineError::other(format!("delete card: {e}")))?;
        tx.execute("DELETE FROM card_fts", [])
            .map_err(|e| EngineError::other(format!("delete card_fts: {e}")))?;
        tx.commit()
            .map_err(|e| EngineError::other(format!("commit truncate: {e}")))?;
        Ok(())
    }

    /// 从所有已注册 source 重建索引(自动 truncate + populate)。
    /// Step 1.1 (2026-10-12): instrument rebuild + emit single info log on completion
    /// (entry span emit is enough; outcome logged at the bottom of the fn).
    #[tracing::instrument(skip_all, fields(data_root = %data_root.display()))]
    pub fn rebuild(&mut self, data_root: &Path) -> Result<BuildStats> {
        self.init_schema()?;
        self.truncate()?;
        let stats = self.populate(data_root)?;
        upsert_meta(&self.conn, "last_rebuild_at", &now_iso8601())?;
        tracing::info!(
            banks = stats.banks,
            questions = stats.questions,
            cards = stats.cards,
            "index rebuild complete"
        );
        Ok(stats)
    }

    /// 不清空,增量插入(如果重复 `id` 走 `INSERT OR REPLACE`)。
    ///
    /// 错误语义(P0-1):
    /// - **文件级**失败(某个 bank JSON 解析不了)→ 记 `issues` + `errors`,**继续**处理其余文件
    /// - **source 级**失败(缺 `banks/` 目录)→ 记 `issues`;只有**一个 bank 都没进来且至少一个
    ///   source 整体失败**才算整体失败(否则「空 data_root」会被当成成功,静默产出 0 题索引)
    #[tracing::instrument(skip_all, fields(data_root = %data_root.display()))]
    pub fn populate(&mut self, data_root: &Path) -> Result<BuildStats> {
        let mut stats = BuildStats::default();
        let sources = all_sources();
        if sources.is_empty() {
            return Err(EngineError::Other("no source registered".into()));
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| EngineError::other(format!("begin populate tx: {e}")))?;
        let mut source_errors: Vec<EngineError> = Vec::new();
        let mut file_error_count: usize = 0;
        for source in sources {
            match source.load(data_root) {
                Ok(outcome) => {
                    for issue in outcome.issues {
                        tracing::warn!(
                            source = %issue.source_id,
                            path = %issue.path.display(),
                            error = %issue.message,
                            "source file load failed — 跳过该文件"
                        );
                        stats.errors += 1;
                        file_error_count += 1;
                        stats.issues.push(issue);
                    }
                    for r in outcome.results {
                        stats.banks += 1;
                        insert_load_result(
                            &tx,
                            &r,
                            &mut stats.questions,
                            &mut stats.cards,
                            &mut stats.errors,
                        )?;
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        source = source.id(),
                        path = %data_root.display(),
                        error = %e,
                        "source load failed — 跳过该 source"
                    );
                    source_errors.push(e);
                    stats.errors += 1;
                    stats.issues.push(LoadIssue {
                        source_id: source.id().to_string(),
                        path: data_root.to_path_buf(),
                        message: source_errors
                            .last()
                            .map(|e| e.to_string())
                            .unwrap_or_default(),
                    });
                }
            }
        }
        tx.commit()
            .map_err(|e| EngineError::other(format!("commit populate: {e}")))?;
        // 一个 bank 都没进来 **且** 有错误 → 整体失败(不能静默产出 0 题索引)。
        // 失败原因优先级:文件级失败(用户看得见的现象)> source 级 schema 错误。
        if stats.banks == 0 && stats.errors > 0 {
            if file_error_count > 0 {
                return Err(EngineError::other(format!(
                    "0 个 bank 入库,{file_error_count} 个文件加载失败(首个:{})",
                    stats
                        .issues
                        .iter()
                        .find(|i| !i.message.is_empty())
                        .map(|i| i.message.as_str())
                        .unwrap_or("(无详情)")
                )));
            }
            if let Some(first) = source_errors.into_iter().next() {
                return Err(first);
            }
        }
        Ok(stats)
    }

    pub fn stats(&self) -> Result<BuildStats> {
        let mut s = BuildStats::default();
        let q_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM q", [], |r| r.get(0))
            .map_err(|e| EngineError::other(format!("count q: {e}")))?;
        s.questions = q_count as usize;
        let card_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM card", [], |r| r.get(0))
            .map_err(|e| EngineError::other(format!("count card: {e}")))?;
        s.cards = card_count as usize;
        let bank_count: i64 = self
            .conn
            .query_row("SELECT COUNT(DISTINCT src) FROM q", [], |r| r.get(0))
            .map_err(|e| EngineError::other(format!("distinct src: {e}")))?;
        s.banks = bank_count as usize;
        Ok(s)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct BuildStats {
    pub banks: usize,
    pub questions: usize,
    pub cards: usize,
    /// 加载错误总数(文件级 + source 级 + 行级)。
    ///
    /// P0-1 之前这是**死字段**:行级失败时计数 +1 就 `return Err`,
    /// 计数永远到不了调用方 —— 于是「0 错误」是假象。
    pub errors: usize,
    /// 加载错误明细(文件级 / source 级)。
    pub issues: Vec<LoadIssue>,
}

fn insert_load_result(
    tx: &Transaction<'_>,
    r: &LoadResult,
    q_counter: &mut usize,
    card_counter: &mut usize,
    err_counter: &mut usize,
) -> Result<()> {
    let src = r.bank_id.as_str();
    for q in &r.questions {
        let nq = norm(&q.stem);
        let nqstem = norm(&stem_of(&q.stem));
        let answer = q.answer_keys.join("/");
        let answer_text = join_answer_text(&q.answer_text);
        let analysis = q.analysis.clone().unwrap_or_default();
        let chapter = q.topic.clone().unwrap_or_default();
        let qtype = q.qtype.as_str();

        // 写入 `q`:用 **upsert** 而不是 `INSERT OR REPLACE`。
        // `INSERT OR REPLACE` = 删旧行 + 插新行 → rowid 会变 → `q_fts.rowid` 映射就断了。
        // upsert 只 UPDATE,rowid 保持不变,FTS 的 1:1 映射才成立。
        //
        // **为什么不用 `... RETURNING rowid`**(2026-11-04 实测,别改回去):
        // 同样一条 upsert,加 `RETURNING rowid` 比不加慢 3~4 倍 ——
        // 400 行 fixture 实测 22.3 ms vs 7.0 ms,整表 build 因此慢 3 倍。
        // 贵的不是 upsert 本身(去掉 RETURNING 后 upsert 几乎免费),而是 RETURNING:
        // 它让 SQLite 每行多走一条「物化输出行」的路。
        // 代价不划算 —— rowid 用一次主键索引 SELECT 拿回来,几乎免费(实测 +2 ms/400 行)。
        let q_insert = tx.execute(
            "INSERT INTO q(id, num, type, question, nq, nqstem, answer, answerText, analysis, src, chapter) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(src, id) DO UPDATE SET \
                num = excluded.num, type = excluded.type, question = excluded.question, \
                nq = excluded.nq, nqstem = excluded.nqstem, answer = excluded.answer, \
                answerText = excluded.answerText, analysis = excluded.analysis, \
                chapter = excluded.chapter",
            params![
                q.id.as_str(),
                q.number.map(|n| n as i64),
                qtype,
                q.stem,
                nq,
                nqstem,
                answer,
                answer_text,
                analysis,
                src,
                chapter,
            ],
        );
        if let Err(e) = q_insert {
            // 行级失败:计数 + 跳过该题,不中止整个 build(P0-1)
            *err_counter += 1;
            tracing::warn!(id = %q.id.as_str(), error = %e, "insert q failed — 跳过该题");
            continue;
        }
        // upsert 不改 rowid → 无论这次是「插入」还是「更新」,rowid 都是同一条记录的。
        // 用主键索引查回来(PK = (src, id) → 走 `sqlite_autoindex_q_1`)。
        let rowid = match tx.query_row(
            "SELECT rowid FROM q WHERE src = ? AND id = ?",
            params![src, q.id.as_str()],
            |r| r.get::<_, i64>(0),
        ) {
            Ok(rowid) => rowid,
            Err(e) => {
                *err_counter += 1;
                tracing::warn!(id = %q.id.as_str(), error = %e, "lookup q rowid failed — 跳过该题");
                continue;
            }
        };
        // 同步插入 FTS — question 与 analysis 都做 CJK 字间加空格,
        // 让 unicode61 tokenizer 按字切分,与 build_fts_term 一致。
        // rowid 与 `q.rowid` 对齐 → `q_fts` 与 `q` 严格一一对应(P1-1)。
        let fts_question = cjk_expand(&q.stem);
        let fts_analysis = cjk_expand(&analysis);
        let fts = tx.execute(
            "INSERT OR REPLACE INTO q_fts(rowid, question, analysis) VALUES (?, ?, ?)",
            params![rowid, fts_question, fts_analysis],
        );
        if let Err(e) = fts {
            *err_counter += 1;
            tracing::warn!(id = %q.id.as_str(), error = %e, "insert q_fts failed — 跳过该题");
            continue;
        }
        *q_counter += 1;
    }
    // Phase G: 写入问答材料卡片到 card + card_fts
    for card in &r.cards {
        let tags = card.tags.join(",");
        // 同 `q`:upsert 不带 RETURNING(见上面的性能说明),rowid 走主键索引查回。
        let card_insert = tx.execute(
            "INSERT INTO card(id, title, body, src, tags) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(src, id) DO UPDATE SET \
                title = excluded.title, body = excluded.body, tags = excluded.tags",
            params![card.id, card.title, card.body, src, tags],
        );
        if let Err(e) = card_insert {
            // 行级失败:计数 + 跳过该卡片,不中止整个 build(P0-1)
            *err_counter += 1;
            tracing::warn!(id = %card.id, error = %e, "insert card failed — 跳过该卡片");
            continue;
        }
        let card_rowid = match tx.query_row(
            "SELECT rowid FROM card WHERE src = ? AND id = ?",
            params![src, card.id],
            |r| r.get::<_, i64>(0),
        ) {
            Ok(rowid) => rowid,
            Err(e) => {
                *err_counter += 1;
                tracing::warn!(id = %card.id, error = %e, "lookup card rowid failed — 跳过该卡片");
                continue;
            }
        };
        let fts_title = cjk_expand(&card.title);
        let fts_body = cjk_expand(&card.body);
        let fts = tx.execute(
            "INSERT OR REPLACE INTO card_fts(rowid, title, body) VALUES (?, ?, ?)",
            params![card_rowid, fts_title, fts_body],
        );
        if let Err(e) = fts {
            *err_counter += 1;
            tracing::warn!(id = %card.id, error = %e, "insert card_fts failed — 跳过该卡片");
            continue;
        }
        *card_counter += 1;
    }
    Ok(())
}

/// 表是否存在(含 view)。
fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','view') AND name = ?1",
            params![name],
            |r| r.get(0),
        )
        .map_err(|e| EngineError::other(format!("table_exists({name}): {e}")))?;
    Ok(n > 0)
}

/// 表的主键列(按主键顺序)。
fn primary_key_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info(?1) WHERE pk > 0 ORDER BY pk")
        .map_err(|e| EngineError::other(format!("prepare table_info({table}): {e}")))?;
    let rows = stmt
        .query_map(params![table], |r| r.get::<_, String>(0))
        .map_err(|e| EngineError::other(format!("query table_info({table}): {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| EngineError::other(format!("row table_info: {e}")))?);
    }
    Ok(out)
}

/// 表的主键列是否恰好等于 `expected`(顺序也要求一致)。
fn pk_is(conn: &Connection, table: &str, expected: &[&str]) -> Result<bool> {
    let pk = primary_key_columns(conn, table)?;
    Ok(pk.len() == expected.len() && pk.iter().zip(expected).all(|(a, b)| a == b))
}

/// 表是否含某列。
fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            params![table, column],
            |r| r.get(0),
        )
        .map_err(|e| EngineError::other(format!("has_column({table},{column}): {e}")))?;
    Ok(n > 0)
}

/// 旧 `q` 表的一行(只取迁移需要的列)。
struct LegacyQRow {
    id: String,
    num: Option<i64>,
    qtype: Option<String>,
    question: String,
    answer: Option<String>,
    answer_text: Option<String>,
    analysis: Option<String>,
    src: String,
    chapter: Option<String>,
}

/// 把旧 `q` 表(主键 `id`)迁到复合主键 `(src, id)`。
///
/// 旧表因为 `INSERT OR REPLACE` 已经丢了跨 bank 撞 id 的题(实测 129 题)——那部分
/// **救不回来**,只能靠下一次 `rebuild` 从源数据补齐。这里能救的是「还剩的行」:
/// 换主键、保留数据,并用当前语义重算 `nq` / `nqstem`
/// (旧值是 `norm` 折叠空格时代的产物,已与 Python 契约不符)。
fn migrate_q_table(tx: &Transaction<'_>) -> Result<()> {
    if !table_exists(tx, "q")? || pk_is(tx, "q", &["src", "id"])? {
        return Ok(());
    }
    let rows = match read_legacy_q_rows(tx) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "旧 q 表读不出来 — 重建空表(下次 rebuild 会从源数据补齐)"
            );
            Vec::new()
        }
    };
    tx.execute_batch("DROP TABLE IF EXISTS q_migrate")
        .map_err(|e| EngineError::other(format!("drop q_migrate: {e}")))?;
    tx.execute_batch(CREATE_Q_MIGRATE)
        .map_err(|e| EngineError::other(format!("create q_migrate: {e}")))?;
    for r in rows {
        // nq / nqstem 用当前语义重算(question 是原题干,重算结果与 insert_load_result 一致)
        let nq = norm(&r.question);
        let nqstem = norm(&stem_of(&r.question));
        tx.execute(
            "INSERT OR REPLACE INTO q_migrate(id, num, type, question, nq, nqstem, answer, answerText, analysis, src, chapter) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                r.id,
                r.num,
                r.qtype,
                r.question,
                nq,
                nqstem,
                r.answer,
                r.answer_text,
                r.analysis,
                r.src,
                r.chapter,
            ],
        )
        .map_err(|e| EngineError::other(format!("insert q_migrate: {e}")))?;
    }
    tx.execute_batch("DROP TABLE q")
        .map_err(|e| EngineError::other(format!("drop old q: {e}")))?;
    tx.execute_batch("ALTER TABLE q_migrate RENAME TO q")
        .map_err(|e| EngineError::other(format!("rename q_migrate: {e}")))?;
    Ok(())
}

/// 旧 `card` 表的一行。
struct LegacyCardRow {
    id: String,
    title: Option<String>,
    body: Option<String>,
    src: String,
    tags: Option<String>,
}

/// 把旧 `card` 表(主键 `id`)迁到复合主键 `(src, id)`。
fn migrate_card_table(tx: &Transaction<'_>) -> Result<()> {
    if !table_exists(tx, "card")? || pk_is(tx, "card", &["src", "id"])? {
        return Ok(());
    }
    let rows = match read_legacy_card_rows(tx) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "旧 card 表读不出来 — 重建空表(下次 rebuild 会从源数据补齐)"
            );
            Vec::new()
        }
    };
    tx.execute_batch("DROP TABLE IF EXISTS card_migrate")
        .map_err(|e| EngineError::other(format!("drop card_migrate: {e}")))?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS card_migrate (
            id       TEXT NOT NULL,
            title    TEXT,
            body     TEXT,
            src      TEXT,
            tags     TEXT,
            PRIMARY KEY (src, id)
        )",
    )
    .map_err(|e| EngineError::other(format!("create card_migrate: {e}")))?;
    for r in rows {
        tx.execute(
            "INSERT OR REPLACE INTO card_migrate(id, title, body, src, tags) VALUES (?, ?, ?, ?, ?)",
            params![r.id, r.title, r.body, r.src, r.tags],
        )
        .map_err(|e| EngineError::other(format!("insert card_migrate: {e}")))?;
    }
    tx.execute_batch("DROP TABLE card")
        .map_err(|e| EngineError::other(format!("drop old card: {e}")))?;
    tx.execute_batch("ALTER TABLE card_migrate RENAME TO card")
        .map_err(|e| EngineError::other(format!("rename card_migrate: {e}")))?;
    Ok(())
}

/// 重建 FTS 表并从 `q` / `card` 回填 —— `q_fts.rowid == q.rowid`、`card_fts.rowid == card.rowid`。
///
/// 旧 FTS 表带 `id` 列且是「平行维护」的(去重规则与 `q` 不一致 → 行数不等 → join 扇出),
/// 这里直接重建:结构上不可能再出现 `q` / `q_fts` 行数漂移。
fn rebuild_fts_tables(tx: &Transaction<'_>) -> Result<()> {
    // 旧 FTS 表一律先删(它是旧结构 —— 带 `id` 列、平行维护)。
    // 内容表不存在时只删不建 —— `init_schema` 后面会用 IF NOT EXISTS 建出空表,
    // 避免「表不存在」直接报错(存量库不一定同时有 q 和 card)。
    tx.execute_batch("DROP TABLE IF EXISTS q_fts")
        .map_err(|e| EngineError::other(format!("drop q_fts: {e}")))?;
    if table_exists(tx, "q")? {
        tx.execute_batch(CREATE_Q_FTS)
            .map_err(|e| EngineError::other(format!("create q_fts: {e}")))?;
    }
    {
        if table_exists(tx, "q")? {
            let mut stmt = tx
                .prepare("SELECT rowid, question, analysis FROM q")
                .map_err(|e| EngineError::other(format!("prepare q for fts: {e}")))?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    ))
                })
                .map_err(|e| EngineError::other(format!("query q for fts: {e}")))?;
            for row in rows {
                let (rowid, question, analysis) =
                    row.map_err(|e| EngineError::other(format!("row q for fts: {e}")))?;
                tx.execute(
                    "INSERT OR REPLACE INTO q_fts(rowid, question, analysis) VALUES (?, ?, ?)",
                    params![rowid, cjk_expand(&question), cjk_expand(&analysis)],
                )
                .map_err(|e| EngineError::other(format!("insert q_fts: {e}")))?;
            }
        }
    }
    tx.execute_batch("DROP TABLE IF EXISTS card_fts")
        .map_err(|e| EngineError::other(format!("drop card_fts: {e}")))?;
    if table_exists(tx, "card")? {
        tx.execute_batch(CREATE_CARD_FTS)
            .map_err(|e| EngineError::other(format!("create card_fts: {e}")))?;
    }
    {
        if table_exists(tx, "card")? {
            let mut stmt = tx
                .prepare("SELECT rowid, title, body FROM card")
                .map_err(|e| EngineError::other(format!("prepare card for fts: {e}")))?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    ))
                })
                .map_err(|e| EngineError::other(format!("query card for fts: {e}")))?;
            for row in rows {
                let (rowid, title, body) =
                    row.map_err(|e| EngineError::other(format!("row card for fts: {e}")))?;
                tx.execute(
                    "INSERT OR REPLACE INTO card_fts(rowid, title, body) VALUES (?, ?, ?)",
                    params![rowid, cjk_expand(&title), cjk_expand(&body)],
                )
                .map_err(|e| EngineError::other(format!("insert card_fts: {e}")))?;
            }
        }
    }
    Ok(())
}

fn read_legacy_q_rows(tx: &Transaction<'_>) -> Result<Vec<LegacyQRow>> {
    let mut stmt = tx
        .prepare(
            "SELECT id, num, type, question, answer, answerText, analysis, src, chapter FROM q",
        )
        .map_err(|e| EngineError::other(format!("prepare legacy q: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(LegacyQRow {
                id: r.get(0)?,
                num: r.get(1)?,
                qtype: r.get(2)?,
                question: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                answer: r.get(4)?,
                answer_text: r.get(5)?,
                analysis: r.get(6)?,
                src: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                chapter: r.get(8)?,
            })
        })
        .map_err(|e| EngineError::other(format!("query legacy q: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| EngineError::other(format!("row legacy q: {e}")))?);
    }
    Ok(out)
}

fn read_legacy_card_rows(tx: &Transaction<'_>) -> Result<Vec<LegacyCardRow>> {
    let mut stmt = tx
        .prepare("SELECT id, title, body, src, tags FROM card")
        .map_err(|e| EngineError::other(format!("prepare legacy card: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(LegacyCardRow {
                id: r.get(0)?,
                title: r.get(1)?,
                body: r.get(2)?,
                src: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                tags: r.get(4)?,
            })
        })
        .map_err(|e| EngineError::other(format!("query legacy card: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| EngineError::other(format!("row legacy card: {e}")))?);
    }
    Ok(out)
}

fn upsert_meta(conn: &Connection, k: &str, v: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
        params![k, v],
    )
    .map_err(|e| EngineError::other(format!("upsert meta: {e}")))?;
    Ok(())
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

/// 用一个 `Source` trait object 直接构建,绕过 inventory(便于测试 + future remote source)。
pub fn build_with_source<S: Source>(
    conn: &mut Connection,
    source: &S,
    data_root: &Path,
) -> Result<BuildStats> {
    conn.execute_batch(CREATE_Q)
        .map_err(|e| EngineError::other(format!("create q: {e}")))?;
    conn.execute_batch(CREATE_IDX_NQ)
        .map_err(|e| EngineError::other(format!("idx_nq: {e}")))?;
    conn.execute_batch(CREATE_IDX_NQSTEM)
        .map_err(|e| EngineError::other(format!("idx_nqstem: {e}")))?;
    conn.execute_batch(CREATE_IDX_SRC)
        .map_err(|e| EngineError::other(format!("idx_src: {e}")))?;
    conn.execute_batch(CREATE_Q_FTS)
        .map_err(|e| EngineError::other(format!("create q_fts: {e}")))?;
    conn.execute_batch(CREATE_CARD)
        .map_err(|e| EngineError::other(format!("create card: {e}")))?;
    conn.execute_batch(CREATE_CARD_FTS)
        .map_err(|e| EngineError::other(format!("create card_fts: {e}")))?;
    conn.execute_batch(CREATE_META)
        .map_err(|e| EngineError::other(format!("create meta: {e}")))?;
    conn.execute("DELETE FROM q", [])
        .map_err(|e| EngineError::other(format!("delete q: {e}")))?;
    conn.execute("DELETE FROM q_fts", [])
        .map_err(|e| EngineError::other(format!("delete q_fts: {e}")))?;
    conn.execute("DELETE FROM card", [])
        .map_err(|e| EngineError::other(format!("delete card: {e}")))?;
    conn.execute("DELETE FROM card_fts", [])
        .map_err(|e| EngineError::other(format!("delete card_fts: {e}")))?;

    let tx = conn
        .unchecked_transaction()
        .map_err(|e| EngineError::other(format!("begin: {e}")))?;
    let outcome = source.load(data_root)?;
    let mut stats = BuildStats::default();
    for issue in outcome.issues {
        tracing::warn!(
            source = %issue.source_id,
            path = %issue.path.display(),
            error = %issue.message,
            "source file load failed — 跳过该文件"
        );
        stats.errors += 1;
        stats.issues.push(issue);
    }
    for r in outcome.results {
        stats.banks += 1;
        insert_load_result(
            &tx,
            &r,
            &mut stats.questions,
            &mut stats.cards,
            &mut stats.errors,
        )?;
    }
    tx.commit()
        .map_err(|e| EngineError::other(format!("commit: {e}")))?;
    Ok(stats)
}

/// 打开一个只读的查询连接(给 search 用)。
pub fn open_readonly(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| EngineError::other(format!("open {}: {e}", db_path.display())))?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::json_banks::JsonBanksSource;

    fn small_tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn populate_isolates_bad_bank_file_and_keeps_healthy_ones() {
        // P0-1 回归:坏文件只记 issue,健康题库照常进索引。
        // 修复前:load_one_bank 的 `?` → 整个 source Err → rebuild 整体失败,健康题库全丢。
        let dir = small_tempdir();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        // crawl/ 也建出来 —— 模拟真实 data_root(缺 crawl/ 会让 crawl source 整体报错)
        std::fs::create_dir_all(dir.path().join("crawl")).unwrap();
        std::fs::write(
            banks.join("good.json"),
            r#"{
                "meta": {"title": "健康题库"},
                "questions": [
                    {"id": "g-1", "question": "健康题干?", "answerKeys": ["A"], "answerText": ["对"]}
                ]
            }"#,
        )
        .unwrap();
        std::fs::write(banks.join("bad.json"), "{ broken json").unwrap();

        let mut b = IndexBuilder::open_in_memory().unwrap();
        let stats = b.rebuild(dir.path()).expect("坏文件不应让整体失败");

        assert_eq!(stats.banks, 1, "健康题库应入库");
        assert_eq!(stats.questions, 1);
        assert_eq!(stats.errors, 1, "坏文件应记 1 个错误");
        assert_eq!(stats.issues.len(), 1);
        assert!(
            stats.issues[0].path.to_string_lossy().contains("bad.json"),
            "issue 应指向坏文件: {:?}",
            stats.issues[0]
        );
        // DB 行数确认真的写进去了
        assert_eq!(b.stats().unwrap().questions, 1);
    }

    #[test]
    fn populate_with_only_bad_banks_reports_errors_without_panicking() {
        // 锁住宽容语义:banks/ 存在但全部文件坏 → 不 panic、错误如实上报,
        // 不能静默变成「0 题成功」。
        let dir = small_tempdir();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        std::fs::create_dir_all(dir.path().join("crawl")).unwrap();
        std::fs::write(banks.join("bad1.json"), "{ broken").unwrap();
        std::fs::write(banks.join("bad2.json"), "also broken").unwrap();

        let mut b = IndexBuilder::open_in_memory().unwrap();
        // 0 bank + 有错误 → 整体失败(而不是静默成功)
        let err = b.rebuild(dir.path()).expect_err("0 题入库必须报错");
        let msg = err.to_string();
        assert!(
            msg.contains("文件加载失败"),
            "错误信息应说明文件级失败,got: {msg}"
        );
    }

    #[test]
    fn open_in_memory_and_init_schema() {
        let b = IndexBuilder::open_in_memory().unwrap();
        b.init_schema().unwrap();
        let s = b.stats().unwrap();
        assert_eq!(s.questions, 0);
        assert_eq!(s.banks, 0);
    }

    #[test]
    fn build_with_json_banks_source_round_trip() {
        // 准备临时 banks 目录(只放 1 个最小题库 JSON)
        let dir = small_tempdir();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        std::fs::write(
            banks.join("demo.json"),
            r#"{
                "meta": {"title": "示例题库", "external": false, "year": "2024", "sourceNote": ""},
                "questions": [
                    {
                        "id": "demo-1",
                        "number": 1,
                        "year": "2024",
                        "type": "single",
                        "typeLabel": "单选",
                        "question": "1. 这是题干?A. 选项一 B. 选项二",
                        "options": [{"key": "A", "text": "选项一"}, {"key": "B", "text": "选项二"}],
                        "answerKeys": ["A"],
                        "answerText": ["一"],
                        "analysis": "解析内容",
                        "topic": "章节一",
                        "srcBank": "demo.json"
                    },
                    {
                        "id": "demo-2",
                        "number": 2,
                        "year": "2024",
                        "type": "judge",
                        "typeLabel": "判断",
                        "question": "这是判断题题干",
                        "answerKeys": ["T"],
                        "answerText": ["对"],
                        "analysis": "",
                        "topic": "章节二",
                        "srcBank": "demo.json"
                    }
                ]
            }"#,
        )
        .unwrap();
        std::fs::write(
            banks.join("answers_demo.json"),
            r#"{"meta":{"title":"answers"},"questions":[]}"#,
        )
        .unwrap();
        std::fs::write(banks.join("ignored.json.bak"), "{}").unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        let stats = build_with_source(&mut conn, &JsonBanksSource, dir.path()).unwrap();
        assert_eq!(stats.banks, 1, "1 个主 bank,answers/bak 被跳");
        assert_eq!(stats.questions, 2);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM q", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        let (nq, answer): (String, String) = conn
            .query_row("SELECT nq, answer FROM q WHERE id = 'demo-1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(answer, "A");
        assert!(nq.contains("题干"));
        assert!(nq.contains("选项一"));
    }

    #[test]
    fn synthetic_id_when_missing() {
        // 与 Python `solve.py::build` 形如 `<fn>-<number>` 一致;
        // 但 Rust BankId 约定不带 `.json` 后缀(规范化),所以合成 ID 也是 `<bank_id>-<idx>`
        // —— 见 types.rs::BankId 文档。Python 那边会拼出 `synth.json-7`,
        // 这是已知 < 1% 偏差(报告 R-009 留底)。
        let dir = small_tempdir();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        std::fs::write(
            banks.join("synth.json"),
            r#"{
                "meta": {"title": "synth"},
                "questions": [
                    {"number": 7, "type": "single", "question": "无 id 题", "answerKeys": ["A"]}
                ]
            }"#,
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        let stats = build_with_source(&mut conn, &JsonBanksSource, dir.path()).unwrap();
        assert_eq!(stats.questions, 1);
        let id: String = conn
            .query_row("SELECT id FROM q", [], |r| r.get(0))
            .unwrap();
        assert_eq!(id, "synth-0", "Rust 约定:<bank_id>-<idx>");
    }

    #[test]
    fn rebuild_is_idempotent() {
        let dir = small_tempdir();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        // CrawlCardsSource(Phase G 已注册)需要 crawl/ 目录,即便为空
        std::fs::create_dir_all(dir.path().join("crawl")).unwrap();
        std::fs::write(
            banks.join("a.json"),
            r#"{"meta":{"title":"a"},"questions":[{"id":"a-1","number":1,"type":"single","question":"q","answerKeys":["A"]}]}"#,
        )
        .unwrap();

        let db = dir.path().join("test.db");
        let mut b = IndexBuilder::open(&db).unwrap();
        b.rebuild(dir.path()).unwrap();
        b.rebuild(dir.path()).unwrap();
        let s = b.stats().unwrap();
        assert_eq!(s.questions, 1, "重复 rebuild 不应翻倍");
    }

    #[test]
    fn truncate_clears_data() {
        let dir = small_tempdir();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        std::fs::create_dir_all(dir.path().join("crawl")).unwrap();
        std::fs::write(
            banks.join("a.json"),
            r#"{"meta":{"title":"a"},"questions":[{"id":"a-1","number":1,"type":"single","question":"q","answerKeys":["A"]}]}"#,
        )
        .unwrap();
        let mut b = IndexBuilder::open_in_memory().unwrap();
        b.rebuild(dir.path()).unwrap();
        assert_eq!(b.stats().unwrap().questions, 1);
        b.truncate().unwrap();
        assert_eq!(b.stats().unwrap().questions, 0);
    }

    /// Quality-reviewer P1 #4 (2026-10-12):rebuild 在 data_root 存在但两个子目录都空时的行为
    /// - JsonBanksSource 看到空 banks/ → 返回 0 banks 不报错
    /// - CrawlCardsSource 同理
    /// - rebuild 应成功,stats 全 0,errors 0
    #[test]
    fn rebuild_succeeds_on_empty_subdirs() {
        let dir = small_tempdir();
        std::fs::create_dir_all(dir.path().join("banks")).unwrap();
        std::fs::create_dir_all(dir.path().join("crawl")).unwrap();

        let mut b = IndexBuilder::open_in_memory().unwrap();
        let stats = b.rebuild(dir.path()).expect("rebuild empty must succeed");
        assert_eq!(stats.banks, 0);
        assert_eq!(stats.questions, 0);
        assert_eq!(stats.cards, 0);
        assert_eq!(stats.errors, 0, "空目录不应产生 source error");
    }

    /// Quality-reviewer P1 #4:data_root 完全空(没有 banks/ 也没有 crawl/)→
    /// JsonBanksSource 因缺 banks/ 返回 schema Err。
    /// **当前行为**:`populate` 用 `?` 短路,rebuild 整体 fail-fast 返回 Err
    /// (`rebuild_succeeds_on_empty_subdirs` 测的是子目录存在但为空的情况,成功路径)。
    /// 这暴露一个潜在改进:rebuild 能否容错,逐 source 收集错误?
    /// 当前选择 fail-fast — 与 `load()` 错误透传语义一致(避免静默丢失)。
    #[test]
    fn rebuild_fails_fast_when_subdir_missing() {
        let dir = small_tempdir();
        // 不创建任何子目录

        let mut b = IndexBuilder::open_in_memory().unwrap();
        let err = b
            .rebuild(dir.path())
            .expect_err("data_root 完全空 → populate 应 fail-fast");
        assert!(
            matches!(err, EngineError::Schema { .. }),
            "期望 schema 错误,got: {err:?}"
        );
    }

    /// Quality-reviewer P1 #4:JSON 文件内同 id 出现 2 次 → INSERT OR REPLACE 不应翻倍
    /// (与 `rebuild_is_idempotent` 互补:后者跨 rebuild,本测试单文件内 in-place)
    ///
    /// 注意:`stats.questions` 是**输入计数**(`insert_load_result` 里 +=1),
    /// 不是 DB 行数。DB 真实行数必须查 `b.stats().questions`。
    /// 本测试用后者验证 REPLACE 实际生效。
    #[test]
    fn rebuild_handles_duplicate_ids_in_same_json() {
        let dir = small_tempdir();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        std::fs::create_dir_all(dir.path().join("crawl")).unwrap();
        // 同一 JSON 内 id="dup" 出现 2 次
        std::fs::write(
            banks.join("dup.json"),
            r#"{
                "meta":{"title":"dup"},
                "questions":[
                    {"id":"dup","number":1,"type":"single","question":"first","answerKeys":["A"]},
                    {"id":"dup","number":2,"type":"single","question":"second","answerKeys":["B"]}
                ]
            }"#,
        )
        .unwrap();

        let mut b = IndexBuilder::open_in_memory().unwrap();
        let stats = b.rebuild(dir.path()).expect("rebuild must succeed");
        // stats.questions 是 input 计数(2),与 DB 行数不同
        assert_eq!(
            stats.questions, 2,
            "stats.questions 反映输入数,不是 DB 行数"
        );
        // DB 真实行数:同 id REPLACE → 1 行
        assert_eq!(
            b.stats().unwrap().questions,
            1,
            "同 id REPLACE 后 DB 只剩 1 行(后者覆盖后者)"
        );
    }

    // ================ P1-1:跨 bank 同 id 不该丢题 + FTS 1:1 ================

    /// 两个 bank 用同一批 question id —— 真实数据里 1768 题有 129 个 id 跨 bank 重复。
    ///
    /// 修复前:`q` 是单列主键 + `INSERT OR REPLACE` → 跨 bank 同 id **互相顶掉**,
    /// 129 题静默消失(Python 侧契约是 `PRIMARY KEY(bank, qid)`,这里对齐)。
    #[test]
    fn questions_with_same_id_in_different_banks_are_both_kept() {
        let dir = small_tempdir();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        std::fs::create_dir_all(dir.path().join("crawl")).unwrap();
        for (bank, title) in [("a.json", "A"), ("b.json", "B")] {
            std::fs::write(
                banks.join(bank),
                format!(
                    r#"{{
                        "meta": {{"title": "{title}"}},
                        "questions": [
                            {{"id":"dup-1","number":1,"type":"single","question":"甲 bank 的第一题","answerKeys":["A"]}},
                            {{"id":"dup-2","number":2,"type":"single","question":"甲 bank 的第二题","answerKeys":["B"]}}
                        ]
                    }}"#
                ),
            )
            .unwrap();
        }

        let mut b = IndexBuilder::open_in_memory().unwrap();
        b.rebuild(dir.path()).expect("rebuild must succeed");

        let rows: i64 = b
            .connection()
            .query_row("SELECT COUNT(*) FROM q", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 4, "跨 bank 同 id 的题必须都保留(修复前只剩 2 行)");

        // 两个 bank 的同 id 题都在,且 src 不同
        let distinct: i64 = b
            .connection()
            .query_row("SELECT COUNT(DISTINCT src || '|' || id) FROM q", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(distinct, 4, "(src, id) 必须唯一");
    }

    /// `q_fts` 与 `q` 必须行数一致 —— 不一致就是 join 扇出的根。
    #[test]
    fn fts_row_count_tracks_q_row_count() {
        let dir = small_tempdir();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        std::fs::create_dir_all(dir.path().join("crawl")).unwrap();
        std::fs::write(
            banks.join("a.json"),
            r#"{"meta":{"title":"A"},"questions":[
                {"id":"q1","question":"网络安全基础","answerKeys":["A"]},
                {"id":"q2","question":"防火墙部署","answerKeys":["B"]}
            ]}"#,
        )
        .unwrap();

        let mut b = IndexBuilder::open_in_memory().unwrap();
        b.rebuild(dir.path()).expect("rebuild must succeed");

        let (q_n, fts_n): (i64, i64) = {
            let conn = b.connection();
            (
                conn.query_row("SELECT COUNT(*) FROM q", [], |r| r.get(0))
                    .unwrap(),
                conn.query_row("SELECT COUNT(*) FROM q_fts", [], |r| r.get(0))
                    .unwrap(),
            )
        };
        assert_eq!(q_n, fts_n, "q 与 q_fts 行数必须一致(不一致 → join 扇出)");
        assert_eq!(q_n, 2);
    }

    /// `populate` 是 public、注释写明「不清空,增量插入」→ 同一个库调两次不能产生重复。
    /// (修复前:`q` 用 REPLACE 去重、`q_fts` 用普通 INSERT 不去重 → 第二次 populate
    ///  让 `q_fts` 行数翻倍 → join 扇出。)
    #[test]
    fn populate_twice_does_not_duplicate_fts_rows() {
        let dir = small_tempdir();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        std::fs::create_dir_all(dir.path().join("crawl")).unwrap();
        std::fs::write(
            banks.join("a.json"),
            r#"{"meta":{"title":"A"},"questions":[{"id":"q1","question":"网络安全基础","answerKeys":["A"]}]}"#,
        )
        .unwrap();

        let mut b = IndexBuilder::open_in_memory().unwrap();
        b.init_schema().unwrap();
        b.populate(dir.path()).expect("first populate");
        b.populate(dir.path()).expect("second populate");

        let (q_n, fts_n): (i64, i64) = {
            let conn = b.connection();
            (
                conn.query_row("SELECT COUNT(*) FROM q", [], |r| r.get(0))
                    .unwrap(),
                conn.query_row("SELECT COUNT(*) FROM q_fts", [], |r| r.get(0))
                    .unwrap(),
            )
        };
        assert_eq!(q_n, 1, "q 应仍是 1 行");
        assert_eq!(fts_n, 1, "q_fts 增量写入不得重复(修复前会变 2 行)");
    }

    /// `q_fts.rowid` 必须与 `q.rowid` **严格 1:1** —— 这是 join 不扇出的结构保证。
    ///
    /// 2026-11-04:写入路径把 `... RETURNING rowid` 换成了「upsert + 主键索引 SELECT
    /// rowid」(RETURNING 让 SQLite 每行慢 3~4 倍,实测 400 行 22.3 ms → 7.0 ms)。
    /// 换掉取 rowid 的方式之后,这个不变量必须重新被钉死 —— 尤其**第二次 populate**
    /// 走的是 UPDATE 分支,最容易悄悄错位。
    #[test]
    fn fts_rowid_matches_q_rowid_one_to_one() {
        let dir = small_tempdir();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        std::fs::create_dir_all(dir.path().join("crawl")).unwrap();
        std::fs::write(
            banks.join("a.json"),
            r#"{"meta":{"title":"A"},"questions":[
                {"id":"q1","question":"网络安全基础","answerKeys":["A"]},
                {"id":"q2","question":"防火墙部署","answerKeys":["B"]}
            ]}"#,
        )
        .unwrap();

        let mut b = IndexBuilder::open_in_memory().unwrap();
        b.init_schema().unwrap();
        b.populate(dir.path()).expect("first populate");

        let rowids = |b: &IndexBuilder| -> Vec<(String, i64)> {
            let conn = b.connection();
            let mut stmt = conn.prepare("SELECT id, rowid FROM q ORDER BY id").unwrap();
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .unwrap();
            rows.map(|r| r.unwrap()).collect()
        };

        let before = rowids(&b);
        assert_eq!(before.len(), 2, "应有 2 题");

        // 第二次 populate:走 upsert 的 UPDATE 分支 —— rowid 必须**保持不变**。
        b.populate(dir.path()).expect("second populate");
        let after = rowids(&b);
        assert_eq!(before, after, "upsert 不该改变 rowid(否则 q_fts 映射断裂)");

        // 每条 q 都必须有且只有一条同 rowid 的 q_fts。
        let conn = b.connection();
        let joined: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM q JOIN q_fts ON q.rowid = q_fts.rowid",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let q_n: i64 = conn
            .query_row("SELECT COUNT(*) FROM q", [], |r| r.get(0))
            .unwrap();
        let fts_n: i64 = conn
            .query_row("SELECT COUNT(*) FROM q_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(joined, q_n, "每条 q 都必须有对应的 q_fts 行");
        assert_eq!(fts_n, q_n, "q_fts 行数必须等于 q 行数");
    }

    /// upsert 的 UPDATE 分支必须真的改内容 —— 不能退化成「插不进去就跳过」。
    ///
    /// 这条守住 `INSERT OR IGNORE`(无 UPDATE)那类改写:它行数对、rowid 也对,
    /// 但题干改了之后索引里还是旧文本 → 搜新词搜不到,静默错。
    #[test]
    fn repopulate_updates_existing_row_content() {
        let dir = small_tempdir();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        std::fs::create_dir_all(dir.path().join("crawl")).unwrap();
        let bank = banks.join("a.json");

        std::fs::write(
            &bank,
            r#"{"meta":{"title":"A"},"questions":[{"id":"q1","question":"网络安全基础","answerKeys":["A"]}]}"#,
        )
        .unwrap();

        let mut b = IndexBuilder::open_in_memory().unwrap();
        b.init_schema().unwrap();
        b.populate(dir.path()).expect("first populate");

        // 同一 id,题干改了 → 第二次 populate 必须更新 FTS 索引里的文本。
        std::fs::write(
            &bank,
            r#"{"meta":{"title":"A"},"questions":[{"id":"q1","question":"量子加密通信","answerKeys":["A"]}]}"#,
        )
        .unwrap();
        b.populate(dir.path()).expect("second populate");

        let conn = b.connection();
        let stem: String = conn
            .query_row("SELECT question FROM q WHERE id = 'q1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stem, "量子加密通信", "q 表内容必须被更新");

        // FTS 侧也必须换成新文本(查新词能命中、旧词查不到)。
        // 注意:CJK 在 FTS 里是**按字**存的(`cjk_expand`),所以查询也得按字 ——
        // 引擎侧 `build_fts_term` 就是把 CJK 拆成单字 token 再 OR-join。
        let new_hit: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM q_fts WHERE q_fts MATCH '\"量\"'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let old_hit: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM q_fts WHERE q_fts MATCH '\"网\"'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_hit, 1, "FTS 索引里应是新题干");
        assert_eq!(old_hit, 0, "旧题干不该还留在 FTS 索引里");
    }

    // ================ P2-3:DB 迁移路径 ================

    /// 旧库(单列主键 + 旧 FTS 结构)在 `init_schema` 时被就地升级,
    /// **行不能丢**,且 FTS 要按新结构(rowid 映射)重建。
    #[test]
    fn migration_upgrades_legacy_schema_in_place() {
        let dir = small_tempdir();
        let db_path = dir.path().join("legacy.db");
        {
            // 手工造一个 v2 旧库
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE q (id TEXT PRIMARY KEY, num INTEGER, type TEXT, question TEXT,
                    nq TEXT, nqstem TEXT, answer TEXT, answerText TEXT, analysis TEXT,
                    src TEXT, chapter TEXT);
                 CREATE TABLE card (id TEXT PRIMARY KEY, title TEXT, body TEXT, src TEXT, tags TEXT);
                 CREATE VIRTUAL TABLE q_fts USING fts5(id UNINDEXED, question, analysis,
                    tokenize='unicode61 remove_diacritics 2');
                 CREATE VIRTUAL TABLE card_fts USING fts5(id UNINDEXED, title, body,
                    tokenize='unicode61 remove_diacritics 2');
                 CREATE TABLE meta(k TEXT PRIMARY KEY, v TEXT);
                 INSERT INTO q VALUES('x1', 1, 'single', '旧题干,带标点', '旧nq值', '旧nqstem值',
                    'A', '答案', '', 'bankA', '');
                 INSERT INTO q_fts(id, question, analysis) VALUES('x1', '旧 题干,带 标点', '');
                 INSERT INTO card VALUES('c1', '卡片标题', '卡片正文', 'bankA', 'tag');
                 INSERT INTO card_fts(id, title, body) VALUES('c1', '卡片 标题', '卡片 正文');
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        }

        let b = IndexBuilder::open(&db_path).unwrap();
        b.init_schema().expect("migration must succeed");
        let conn = b.connection();

        // 1. 行没丢
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM q", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "迁移不应丢行");
        let cards: i64 = conn
            .query_row("SELECT COUNT(*) FROM card", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cards, 1, "迁移不应丢卡片");

        // 2. 主键已是 (src, id)
        assert!(
            pk_is(conn, "q", &["src", "id"]).unwrap(),
            "q 主键应迁到 (src, id)"
        );

        // 3. nq / nqstem 用当前语义重算(旧值是 norm 折叠空格时代的产物)
        let (nq, nqstem): (String, String) = conn
            .query_row("SELECT nq, nqstem FROM q WHERE id = 'x1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(nq, "旧题干带标点", "nq 应按当前 norm 语义重算");
        assert_eq!(nqstem, "旧题干带标点", "nqstem 应按当前 stem_of 语义重算");

        // 4. FTS 按新结构重建,rowid 与 q.rowid 对齐
        let fts_cols = primary_key_columns(conn, "q_fts").unwrap();
        assert!(
            !fts_cols.iter().any(|c| c == "id"),
            "新 q_fts 不该再有 id 列(它就是旧的平行维护信号)"
        );
        let joined: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM q_fts JOIN q ON q.rowid = q_fts.rowid",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(joined, 1, "FTS rowid 必须与 q.rowid 一一对应");

        // 5. 版本号双写
        let user_version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(user_version, SCHEMA_VERSION as i64);
        let meta_version: String = conn
            .query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(meta_version, SCHEMA_VERSION.to_string());
    }

    /// 迁移判定不能只看 `user_version` —— 库里结构是旧的,即使版本号写着新也要迁。
    /// (Python 侧 `kb_db.py::ensure_schema` 的注释记着这个坑。)
    #[test]
    fn migration_triggers_on_actual_shape_not_version_number() {
        let dir = small_tempdir();
        let db_path = dir.path().join("lying.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE q (id TEXT PRIMARY KEY, num INTEGER, type TEXT, question TEXT,
                    nq TEXT, nqstem TEXT, answer TEXT, answerText TEXT, analysis TEXT,
                    src TEXT, chapter TEXT);
                 PRAGMA user_version = 999;",
            )
            .unwrap();
        }
        let b = IndexBuilder::open(&db_path).unwrap();
        b.init_schema().expect("init must succeed");
        let conn = b.connection();
        assert!(
            pk_is(conn, "q", &["src", "id"]).unwrap(),
            "user_version 说 999 也得按实际结构迁移"
        );
        let user_version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(user_version, SCHEMA_VERSION as i64, "迁移后版本号要写回");
    }

    /// `nqstem` 精确命中是搜索的第一步 —— 必须走索引,不能全表扫描。
    #[test]
    fn nqstem_lookup_uses_index() {
        let b = IndexBuilder::open_in_memory().unwrap();
        b.init_schema().unwrap();
        let plan: String = b
            .connection()
            .query_row(
                "EXPLAIN QUERY PLAN SELECT id FROM q WHERE nqstem = ?",
                params!["x"],
                |r| r.get::<_, String>(3),
            )
            .unwrap();
        assert!(
            plan.contains("idx_nqstem"),
            "nqstem 查询应走索引,实际计划:{plan}"
        );
    }
}
