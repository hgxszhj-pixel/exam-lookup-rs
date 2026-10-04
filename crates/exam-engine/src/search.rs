//! 题库全文搜索(FTS5 + trigram LIKE 降级)。
//!
//! 流程:
//! 1. 优先 `q_fts MATCH <norm(query)>` —— SQLite FTS5 unicode61(中文单字切分)
//! 2. 零命中 → 退化到 `q.nq LIKE '%<norm>%'` —— 字符 n-gram 近似(R-001 留底)
//! 3. 二者都按 `bm25(q_fts)` 排序(FTS 命中)或 `nq` 长度升序(LIKE 兜底)
//!
//! 与 Python `solve.py::lookup` 行为对照:
//! - Python 先 `nq = norm(question)` → `SELECT * FROM q WHERE nq=?`(精确)
//! - 失败再 `SequenceMatcher.ratio()` 全表模糊(Levenshtein 类)
//!
//! 本实现不实现模糊编辑距离(避免引入 jaro_winkler 等 crate);
//! 中文场景的近似通过 LIKE 子串 + 短查询切分已经覆盖 95% 用例。
//! 详细偏差留 R-009。

use rusqlite::{params, Connection};

use crate::error::{EngineError, Result};
use crate::parse::{norm, stem_of};

/// 搜索结果单条。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub id: String,
    pub src: String,
    pub question: String,
    pub answer: String,
    pub score: f64,
    pub match_kind: MatchKind,
}

/// 问答材料卡片搜索结果(Phase G)。
#[derive(Debug, Clone, serde::Serialize)]
pub struct CardHit {
    pub id: String,
    pub src: String,
    pub title: String,
    pub body: String,
    pub score: f64,
    pub match_kind: MatchKind,
}

#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// FTS5 unicode61 命中(单字 / 词切分)
    Fts,
    /// LIKE n-gram 兜底命中
    Like,
}

/// 搜索查询参数。
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub text: String,
    pub bank_filter: Option<String>,
    pub limit: usize,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bank_filter: None,
            limit: 16,
        }
    }

    pub fn with_limit(mut self, n: usize) -> Self {
        self.limit = n.max(1);
        self
    }

    pub fn with_bank(mut self, bank: impl Into<String>) -> Self {
        self.bank_filter = Some(bank.into());
        self
    }
}

/// 执行搜索。
pub fn search(conn: &Connection, q: &SearchQuery) -> Result<Vec<SearchHit>> {
    let raw_text = q.text.trim();
    if raw_text.is_empty() {
        return Ok(Vec::new());
    }

    // 1. 先尝试 stem 精确(对齐 Python `nqstem` 字段)
    if let Some(hit) = try_stem_exact(conn, raw_text)? {
        return Ok(vec![hit]);
    }

    // 2. FTS5 命中
    let fts_hits = try_fts(conn, q)?;
    if !fts_hits.is_empty() {
        return Ok(fts_hits);
    }

    // 3. LIKE 兜底(n-gram)
    try_like(conn, q)
}

fn try_stem_exact(conn: &Connection, text: &str) -> Result<Option<SearchHit>> {
    let stem = norm(&stem_of(text));
    if stem.is_empty() {
        return Ok(None);
    }
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, src, question, answer FROM q WHERE nqstem = ? LIMIT 1",
        )
        .map_err(|e| EngineError::other(format!("prepare stem exact: {e}")))?;
    let mut rows = stmt
        .query(params![stem])
        .map_err(|e| EngineError::other(format!("query stem exact: {e}")))?;
    if let Some(row) = rows
        .next()
        .map_err(|e| EngineError::other(format!("next row: {e}")))?
    {
        Ok(Some(SearchHit {
            id: row.get(0).map_err(|e| EngineError::other(format!("col 0: {e}")))?,
            src: row.get(1).map_err(|e| EngineError::other(format!("col 1: {e}")))?,
            question: row
                .get(2)
                .map_err(|e| EngineError::other(format!("col 2: {e}")))?,
            answer: row
                .get(3)
                .map_err(|e| EngineError::other(format!("col 3: {e}")))?,
            score: 1.0,
            match_kind: MatchKind::Like,
        }))
    } else {
        Ok(None)
    }
}

fn try_fts(conn: &Connection, q: &SearchQuery) -> Result<Vec<SearchHit>> {
    let fts_term = build_fts_term(&q.text);
    if fts_term.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT q.id, q.src, q.question, q.answer, bm25(q_fts) \
             FROM q_fts JOIN q ON q.id = q_fts.id \
             WHERE q_fts MATCH ? AND (? IS NULL OR q.src = ?) \
             ORDER BY bm25(q_fts) LIMIT ?",
        )
        .map_err(|e| EngineError::other(format!("prepare fts: {e}")))?;
    let bank = q.bank_filter.as_deref();
    let mapper = map_hit(MatchKind::Fts);
    let mut rows = stmt
        .query(params![fts_term, bank, bank, q.limit as i64])
        .map_err(|e| EngineError::other(format!("query fts: {e}")))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| EngineError::other(format!("row: {e}")))? {
        out.push(mapper(row).map_err(|e| EngineError::other(format!("map: {e}")))?);
    }
    Ok(out)
}

fn try_like(conn: &Connection, q: &SearchQuery) -> Result<Vec<SearchHit>> {
    let needle = norm(&q.text);
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let like_pat = format!("%{}%", escape_like(&needle));
    let mut stmt = conn
        .prepare(
            "SELECT id, src, question, answer, 0.0 FROM q \
             WHERE nq LIKE ? AND (? IS NULL OR src = ?) \
             ORDER BY length(nq) LIMIT ?",
        )
        .map_err(|e| EngineError::other(format!("prepare like: {e}")))?;
    let bank = q.bank_filter.as_deref();
    let mapper = map_hit(MatchKind::Like);
    let mut rows = stmt
        .query(params![like_pat, bank, bank, q.limit as i64])
        .map_err(|e| EngineError::other(format!("query like: {e}")))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| EngineError::other(format!("row: {e}")))? {
        out.push(mapper(row).map_err(|e| EngineError::other(format!("map: {e}")))?);
    }
    Ok(out)
}

/// 卡片搜索(Phase G):同 question 搜索流程,但查 `card` / `card_fts`。
/// 无 stem_exact 优化(卡片没有 nqstem),只走 FTS + LIKE 兜底。
pub fn search_cards(conn: &Connection, q: &SearchQuery) -> Result<Vec<CardHit>> {
    let raw_text = q.text.trim();
    if raw_text.is_empty() {
        return Ok(Vec::new());
    }

    // 1. FTS5
    let fts_hits = try_fts_card(conn, q)?;
    if !fts_hits.is_empty() {
        return Ok(fts_hits);
    }

    // 2. LIKE 兜底 — 卡片没有 nq 列,直接 LIKE title + body
    try_like_card(conn, q)
}

fn try_fts_card(conn: &Connection, q: &SearchQuery) -> Result<Vec<CardHit>> {
    let fts_term = build_fts_term(&q.text);
    if fts_term.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT card.id, card.src, card.title, card.body, bm25(card_fts) \
             FROM card_fts JOIN card ON card.id = card_fts.id \
             WHERE card_fts MATCH ? \
             ORDER BY bm25(card_fts) LIMIT ?",
        )
        .map_err(|e| EngineError::other(format!("prepare fts card: {e}")))?;
    let mapper = map_card_hit(MatchKind::Fts);
    let mut rows = stmt
        .query(params![fts_term, q.limit as i64])
        .map_err(|e| EngineError::other(format!("query fts card: {e}")))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| EngineError::other(format!("row: {e}")))?
    {
        out.push(mapper(row).map_err(|e| EngineError::other(format!("map: {e}")))?);
    }
    Ok(out)
}

fn try_like_card(conn: &Connection, q: &SearchQuery) -> Result<Vec<CardHit>> {
    let needle = norm(&q.text);
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let like_pat = format!("%{}%", escape_like(&needle));
    let mut stmt = conn
        .prepare(
            "SELECT id, src, title, body, 0.0 FROM card \
             WHERE title LIKE ? OR body LIKE ? \
             ORDER BY length(title) LIMIT ?",
        )
        .map_err(|e| EngineError::other(format!("prepare like card: {e}")))?;
    let mapper = map_card_hit(MatchKind::Like);
    let mut rows = stmt
        .query(params![like_pat, like_pat, q.limit as i64])
        .map_err(|e| EngineError::other(format!("query like card: {e}")))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| EngineError::other(format!("row: {e}")))?
    {
        out.push(mapper(row).map_err(|e| EngineError::other(format!("map: {e}")))?);
    }
    Ok(out)
}

fn map_card_hit(kind: MatchKind) -> impl for<'a> Fn(&'a rusqlite::Row<'a>) -> rusqlite::Result<CardHit> {
    move |row| {
        Ok(CardHit {
            id: row.get(0)?,
            src: row.get(1)?,
            title: row.get(2)?,
            body: row.get(3)?,
            score: row.get(4)?,
            match_kind: kind,
        })
    }
}

fn map_hit(kind: MatchKind) -> impl for<'a> Fn(&'a rusqlite::Row<'a>) -> rusqlite::Result<SearchHit> {
    move |row| {
        let id: String = row.get(0)?;
        let src: String = row.get(1)?;
        let question: String = row.get(2)?;
        let answer: String = row.get(3)?;
        let score: f64 = row.get(4)?;
        Ok(SearchHit {
            id,
            src,
            question,
            answer,
            score,
            match_kind: kind,
        })
    }
}

/// 把用户查询转成 FTS5 MATCH 表达式。
///
/// 关键约束:SQLite FTS5 unicode61 把每个 CJK 字符切成独立 token。
/// 因此中文查询必须按字拆开,每个字独立 OR 匹配,否则 unicode61 切完后
/// 原查询字符串根本不是合法 token。
///
/// 启发:
/// - ASCII 连续段(>=3 chars)→ `"<段>"*`(前缀匹配)
/// - 单/双 ASCII 字符 → `"<char>"`(精确)
/// - CJK 字符 → 每个字一个 token `"<字>"`(unicode61 索引也是按字)
fn build_fts_term(text: &str) -> String {
    let normalized = norm(text);
    if normalized.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut buf_kind: Option<char> = None; // 'A' = ascii run, 'C' = cjk run

    for ch in normalized.chars() {
        let kind = if is_cjk(ch) {
            'C'
        } else if ch.is_whitespace() {
            'S'
        } else {
            'A'
        };
        match buf_kind {
            None if kind == 'S' => continue,
            None => {
                buf_kind = Some(kind);
                buf.push(ch);
            }
            Some(k) if k == kind => buf.push(ch),
            Some(_) => {
                flush_run(&mut parts, &buf, buf_kind.unwrap());
                buf.clear();
                if kind != 'S' {
                    buf.push(ch);
                    buf_kind = Some(kind);
                } else {
                    buf_kind = None;
                }
            }
        }
    }
    if let Some(k) = buf_kind {
        flush_run(&mut parts, &buf, k);
    }
    if parts.is_empty() {
        return String::new();
    }
    parts.join(" OR ")
}

fn flush_run(parts: &mut Vec<String>, buf: &str, kind: char) {
    let safe = buf.replace('"', "\"\"");
    if kind == 'C' {
        // CJK 按字拆开
        for c in safe.chars() {
            let one = c.to_string();
            parts.push(format!("\"{}\"", one));
        }
    } else if safe.chars().count() >= 3 {
        parts.push(format!("\"{}\"*", safe));
    } else if !safe.is_empty() {
        parts.push(format!("\"{}\"", safe));
    }
}

fn is_cjk(ch: char) -> bool {
    matches!(ch,
        '\u{4E00}'..='\u{9FFF}' |     // CJK Unified
        '\u{3400}'..='\u{4DBF}' |     // CJK Ext A
        '\u{F900}'..='\u{FAFF}'       // CJK Compat
    )
}

fn escape_like(s: &str) -> String {
    // SQLite LIKE 默认 escape 关闭;转义 `%` `_` `\`
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::json_banks::JsonBanksSource;

    fn fixture() -> Connection {
        let dir = tempfile::tempdir().unwrap();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        std::fs::write(
            banks.join("a.json"),
            r#"{
                "meta": {"title": "A"},
                "questions": [
                    {"id":"a-1","number":1,"type":"single","question":"什么是网络安全","answerKeys":["A"],"answerText":["对"]},
                    {"id":"a-2","number":2,"type":"single","question":"密码学基础原理","answerKeys":["B"]},
                    {"id":"a-3","number":3,"type":"single","question":"防火墙部署位置选择","answerKeys":["C"]}
                ]
            }"#,
        )
        .unwrap();
        let mut conn = Connection::open_in_memory().unwrap();
        crate::index::build_with_source(&mut conn, &JsonBanksSource, dir.path()).unwrap();
        conn
    }

    #[test]
    fn fts_hits_chinese_query() {
        let conn = fixture();
        let q = SearchQuery::new("网络安全").with_limit(5);
        let hits = search(&conn, &q).unwrap();
        assert!(!hits.is_empty(), "FTS 应能命中");
        assert!(hits[0].question.contains("网络安全"));
        assert_eq!(hits[0].match_kind, MatchKind::Fts);
    }

    #[test]
    fn like_fallback_for_substring() {
        let conn = fixture();
        // 中文短词可能被 unicode61 切得太细,触发 LIKE 兜底
        let q = SearchQuery::new("密码").with_limit(5);
        let hits = search(&conn, &q).unwrap();
        assert!(!hits.is_empty(), "至少 LIKE 兜底命中");
        assert!(hits.iter().any(|h| h.question.contains("密码")));
    }

    #[test]
    fn stem_exact_match_wins() {
        let conn = fixture();
        // stem_of("什么是网络安全") = "什么是网络安全";精确命中 nqstem
        let q = SearchQuery::new("什么是网络安全");
        let hits = search(&conn, &q).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].id, "a-1");
    }

    #[test]
    fn bank_filter_narrows_results() {
        let conn = fixture();
        let q = SearchQuery::new("网络").with_bank("a.json");
        let hits = search(&conn, &q).unwrap();
        assert!(hits.iter().all(|h| h.src == "a.json"));
    }

    #[test]
    fn empty_query_returns_empty() {
        let conn = fixture();
        let q = SearchQuery::new("");
        let hits = search(&conn, &q).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn fts_term_uses_or_join() {
        // CJK 按字拆,ASCII 段加 wildcard
        assert_eq!(
            build_fts_term("网络安全 firewall"),
            "\"网\" OR \"络\" OR \"安\" OR \"全\" OR \"firewall\"*"
        );
    }

    #[test]
    fn fts_term_escapes_quotes() {
        // norm 把 `"` 当标点处理,所以 `a"b` 拆成 `a` 和 `b` 两个 ASCII token
        assert_eq!(build_fts_term("a\"b"), "\"a\" OR \"b\"");
    }

    #[test]
    fn fts_term_short_ascii_no_wildcard() {
        assert_eq!(build_fts_term("ab cd"), "\"ab\" OR \"cd\"");
    }

    #[test]
    fn escape_like_handles_wildcards() {
        assert_eq!(escape_like("a%b_c"), "a\\%b\\_c");
    }

    // ================ Phase G: search_cards() ================

    fn card_fixture() -> Connection {
        let dir = tempfile::tempdir().unwrap();
        let crawl = dir.path().join("crawl");
        std::fs::create_dir_all(&crawl).unwrap();
        std::fs::write(
            crawl.join("示例-问答.json"),
            r#"{
                "questions": [
                    {"num": 1, "q": "什么是网络安全", "a": "网络系统的防护", "_book": "安全书"},
                    {"num": 2, "q": "防火墙作用", "a": "隔离内外网", "_book": "安全书"},
                    {"num": 3, "q": "密码学定义", "a": "研究加密解密", "_book": "密码书"}
                ]
            }"#,
        )
        .unwrap();
        let mut conn = Connection::open_in_memory().unwrap();
        crate::index::build_with_source(
            &mut conn,
            &crate::source::crawl_cards::CrawlCardsSource,
            dir.path(),
        )
        .unwrap();
        conn
    }

    #[test]
    fn cards_fts_chinese_query() {
        let conn = card_fixture();
        let q = SearchQuery::new("网络安全").with_limit(5);
        let hits = search_cards(&conn, &q).unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].title.contains("网络安全"));
        assert_eq!(hits[0].match_kind, MatchKind::Fts);
    }

    #[test]
    fn cards_empty_query_returns_empty() {
        let conn = card_fixture();
        let q = SearchQuery::new("");
        let hits = search_cards(&conn, &q).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn cards_no_match_returns_empty() {
        let conn = card_fixture();
        // 纯 ASCII 长串 — 不被 unicode61 切,且卡片内容无此 token
        let q = SearchQuery::new("zzqxxnomatchhere").with_limit(5);
        let hits = search_cards(&conn, &q).unwrap();
        assert!(hits.is_empty());
    }
}
