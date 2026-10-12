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
use crate::source::{LoadResult, Source, all_sources};

const SCHEMA_VERSION: i32 = 2;

const CREATE_Q: &str = r#"
CREATE TABLE IF NOT EXISTS q (
    id          TEXT PRIMARY KEY,
    num         INTEGER,
    type        TEXT,
    question    TEXT,
    nq          TEXT,
    nqstem      TEXT,
    answer      TEXT,
    answerText  TEXT,
    analysis    TEXT,
    src         TEXT,
    chapter     TEXT
)"#;

const CREATE_IDX_NQ: &str = "CREATE INDEX IF NOT EXISTS idx_nq ON q(nq)";
const CREATE_IDX_SRC: &str = "CREATE INDEX IF NOT EXISTS idx_src ON q(src)";

const CREATE_Q_FTS: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS q_fts USING fts5(
    id UNINDEXED,
    question,
    analysis,
    tokenize = 'unicode61 remove_diacritics 2'
)"#;

const CREATE_CARD: &str = r#"
CREATE TABLE IF NOT EXISTS card (
    id       TEXT PRIMARY KEY,
    title    TEXT,
    body     TEXT,
    src      TEXT,
    tags     TEXT
)"#;

const CREATE_CARD_FTS: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS card_fts USING fts5(
    id UNINDEXED,
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

    /// 幂等创建表 / 索引 / FTS 虚拟表。
    pub fn init_schema(&self) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| EngineError::other(format!("begin tx: {e}")))?;
        tx.execute(CREATE_Q, [])
            .map_err(|e| EngineError::other(format!("create q: {e}")))?;
        tx.execute(CREATE_IDX_NQ, [])
            .map_err(|e| EngineError::other(format!("idx_nq: {e}")))?;
        tx.execute(CREATE_IDX_SRC, [])
            .map_err(|e| EngineError::other(format!("idx_src: {e}")))?;
        tx.execute(CREATE_Q_FTS, [])
            .map_err(|e| EngineError::other(format!("create q_fts: {e}")))?;
        tx.execute(CREATE_CARD, [])
            .map_err(|e| EngineError::other(format!("create card: {e}")))?;
        tx.execute(CREATE_CARD_FTS, [])
            .map_err(|e| EngineError::other(format!("create card_fts: {e}")))?;
        tx.execute(CREATE_META, [])
            .map_err(|e| EngineError::other(format!("create meta: {e}")))?;
        upsert_meta(&tx, "schema_version", &SCHEMA_VERSION.to_string())?;
        tx.commit()
            .map_err(|e| EngineError::other(format!("commit init: {e}")))?;
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
        upsert_meta(
            &self.conn,
            "last_rebuild_at",
            &now_iso8601(),
        )?;
        tracing::info!(
            banks = stats.banks,
            questions = stats.questions,
            cards = stats.cards,
            "index rebuild complete"
        );
        Ok(stats)
    }

    /// 不清空,增量插入(如果重复 `id` 走 `INSERT OR REPLACE`)。
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
        for source in sources {
            let results = source.load(data_root)?;
            for r in results {
                stats.banks += 1;
                insert_load_result(&tx, &r, &mut stats.questions, &mut stats.cards, &mut stats.errors)?;
            }
        }
        tx.commit()
            .map_err(|e| EngineError::other(format!("commit populate: {e}")))?;
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
    pub errors: usize,
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

        let result = tx.execute(
            "INSERT OR REPLACE INTO q(id, num, type, question, nq, nqstem, answer, answerText, analysis, src, chapter) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        if let Err(e) = result {
            *err_counter += 1;
            return Err(EngineError::other(format!("insert q {}: {e}", q.id)));
        }
        // 同步插入 FTS — question 与 analysis 都做 CJK 字间加空格,
        // 让 unicode61 tokenizer 按字切分,与 build_fts_term 一致
        let fts_question = cjk_expand(&q.stem);
        let fts_analysis = cjk_expand(&analysis);
        let fts = tx.execute(
            "INSERT INTO q_fts(id, question, analysis) VALUES (?, ?, ?)",
            params![q.id.as_str(), fts_question, fts_analysis],
        );
        if let Err(e) = fts {
            *err_counter += 1;
            return Err(EngineError::other(format!("insert fts {}: {e}", q.id)));
        }
        *q_counter += 1;
    }
    // Phase G: 写入问答材料卡片到 card + card_fts
    for card in &r.cards {
        let tags = card.tags.join(",");
        let result = tx.execute(
            "INSERT OR REPLACE INTO card(id, title, body, src, tags) VALUES (?, ?, ?, ?, ?)",
            params![card.id, card.title, card.body, src, tags],
        );
        if let Err(e) = result {
            *err_counter += 1;
            return Err(EngineError::other(format!("insert card {}: {e}", card.id)));
        }
        let fts_title = cjk_expand(&card.title);
        let fts_body = cjk_expand(&card.body);
        let fts = tx.execute(
            "INSERT INTO card_fts(id, title, body) VALUES (?, ?, ?)",
            params![card.id, fts_title, fts_body],
        );
        if let Err(e) = fts {
            *err_counter += 1;
            return Err(EngineError::other(format!("insert card_fts {}: {e}", card.id)));
        }
        *card_counter += 1;
    }
    Ok(())
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
    let results = source.load(data_root)?;
    let mut stats = BuildStats::default();
    for r in results {
        stats.banks += 1;
        insert_load_result(&tx, &r, &mut stats.questions, &mut stats.cards, &mut stats.errors)?;
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
        std::fs::write(banks.join("answers_demo.json"), r#"{"meta":{"title":"answers"},"questions":[]}"#).unwrap();
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
            .query_row(
                "SELECT nq, answer FROM q WHERE id = 'demo-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
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
        assert_eq!(stats.questions, 2, "stats.questions 反映输入数,不是 DB 行数");
        // DB 真实行数:同 id REPLACE → 1 行
        assert_eq!(
            b.stats().unwrap().questions,
            1,
            "同 id REPLACE 后 DB 只剩 1 行(后者覆盖前者)"
        );
    }
}
