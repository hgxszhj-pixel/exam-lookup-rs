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
use crate::parse::{fold_punct_to_space, norm, stem_of};

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

/// Step 1.2 (2026-10-12): query length cap to prevent FTS5 DoS.
/// 4096 bytes ≈ 1300 个 CJK 字符,远大于任何题干长度,但能拦住 5MB 攻击。
pub const MAX_QUERY_BYTES: usize = 4096;

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
/// Step 1.1 (2026-10-12): tracing instrument — `skip_all` because `Connection`
/// has no useful `Debug` impl; capture query text + limit/bank fields.
#[tracing::instrument(skip_all, fields(query = %q.text, limit = q.limit, bank_filter = ?q.bank_filter))]
pub fn search(conn: &Connection, q: &SearchQuery) -> Result<Vec<SearchHit>> {
    // Step 1.2: query length cap before any FTS5 work.
    if q.text.len() > MAX_QUERY_BYTES {
        return Err(EngineError::other(format!(
            "query too long: {} bytes (cap {})",
            q.text.len(),
            MAX_QUERY_BYTES
        )));
    }
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
        .prepare_cached("SELECT id, src, question, answer FROM q WHERE nqstem = ? LIMIT 1")
        .map_err(|e| EngineError::other(format!("prepare stem exact: {e}")))?;
    let mut rows = stmt
        .query(params![stem])
        .map_err(|e| EngineError::other(format!("query stem exact: {e}")))?;
    if let Some(row) = rows
        .next()
        .map_err(|e| EngineError::other(format!("next row: {e}")))?
    {
        Ok(Some(SearchHit {
            id: row
                .get(0)
                .map_err(|e| EngineError::other(format!("col 0: {e}")))?,
            src: row
                .get(1)
                .map_err(|e| EngineError::other(format!("col 1: {e}")))?,
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
    // JOIN 条件用 `rowid` 而不是 `id`:
    // `q_fts.rowid == q.rowid` 是 1:1 映射(见 `index.rs::insert_load_result`),
    // 而 `id` 在跨 bank 时会重复 —— 用 `id` join 就会一对多扇出,
    // 同一条命中重复返回,`LIMIT 16` 实际只给 8 个不同问题(P1-1)。
    let mut stmt = conn
        .prepare(
            "SELECT q.id, q.src, q.question, q.answer, bm25(q_fts) \
             FROM q_fts JOIN q ON q.rowid = q_fts.rowid \
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
    while let Some(row) = rows
        .next()
        .map_err(|e| EngineError::other(format!("row: {e}")))?
    {
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
             WHERE nq LIKE ? ESCAPE '\\' AND (? IS NULL OR src = ?) \
             ORDER BY length(nq) LIMIT ?",
        )
        .map_err(|e| EngineError::other(format!("prepare like: {e}")))?;
    let bank = q.bank_filter.as_deref();
    let mapper = map_hit(MatchKind::Like);
    let mut rows = stmt
        .query(params![like_pat, bank, bank, q.limit as i64])
        .map_err(|e| EngineError::other(format!("query like: {e}")))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| EngineError::other(format!("row: {e}")))?
    {
        out.push(mapper(row).map_err(|e| EngineError::other(format!("map: {e}")))?);
    }
    Ok(out)
}

/// 卡片搜索(Phase G):同 question 搜索流程,但查 `card` / `card_fts`。
/// 无 stem_exact 优化(卡片没有 nqstem),只走 FTS + LIKE 兜底。
#[tracing::instrument(skip_all, fields(query = %q.text, limit = q.limit, bank_filter = ?q.bank_filter))]
pub fn search_cards(conn: &Connection, q: &SearchQuery) -> Result<Vec<CardHit>> {
    // Step 1.2: query length cap before any FTS5 work.
    if q.text.len() > MAX_QUERY_BYTES {
        return Err(EngineError::other(format!(
            "query too long: {} bytes (cap {})",
            q.text.len(),
            MAX_QUERY_BYTES
        )));
    }
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
    // 同 `try_fts`:`card_fts.rowid == card.rowid` 是 1:1 映射,用 rowid join 不会扇出。
    let mut stmt = conn
        .prepare(
            "SELECT card.id, card.src, card.title, card.body, bm25(card_fts) \
             FROM card_fts JOIN card ON card.rowid = card_fts.rowid \
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
             WHERE title LIKE ? ESCAPE '\\' OR body LIKE ? ESCAPE '\\' \
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

fn map_card_hit(
    kind: MatchKind,
) -> impl for<'a> Fn(&'a rusqlite::Row<'a>) -> rusqlite::Result<CardHit> {
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

fn map_hit(
    kind: MatchKind,
) -> impl for<'a> Fn(&'a rusqlite::Row<'a>) -> rusqlite::Result<SearchHit> {
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
    // 用 fold_punct_to_space(不是 norm):unicode61 把标点当分隔符,
    // 查询串必须在标点处断词,否则 "hello,world" 会变成单 token "helloworld",
    // 与索引里的两个 token 对不上。
    let normalized = fold_punct_to_space(text);
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

/// 转义 LIKE 模式里的通配符。
///
/// **必须配合 SQL 里的 `ESCAPE '\'` 子句** —— SQLite 默认把 `\` 当普通字面量,
/// 没有 ESCAPE 时 `%a\%b\_c%` 匹配不到字面值 `a%b_c`(实测:含 `100%` 的题干
/// 永远查不到)。顺序不能换:先转义 `\` 自身,再转义 `%` / `_`。
///
/// 注:`norm()` 会删掉 `_` 和 `\`(它们在 Python `_PUNC` 里),所以实际能漏到
/// 这里的通配符主要是 `%`。
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
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
        // 直接调 try_like —— 走 search() 的话 FTS 先命中,这个测试就钉不住兜底路径
        // (旧写法:即使**整个删掉 try_like**,该测试照样通过)。
        let hits = try_like(&conn, &SearchQuery::new("密码").with_limit(5)).unwrap();
        assert!(!hits.is_empty(), "LIKE 兜底应命中");
        assert!(hits.iter().any(|h| h.question.contains("密码")));
        assert!(
            hits.iter().all(|h| h.match_kind == MatchKind::Like),
            "兜底路径的 kind 必须是 Like"
        );
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
        // bank id 是文件名去后缀(`a.json` → `a`),旧测试写 "a.json" → 过滤不中任何行,
        // `all()` 在空集合上恒真 —— 测试其实什么都没验。
        let q = SearchQuery::new("网络").with_bank("a");
        let hits = search(&conn, &q).unwrap();
        assert!(!hits.is_empty(), "过滤到 bank 'a' 应有命中");
        assert!(hits.iter().all(|h| h.src == "a"));
    }

    /// 两个 bank 用同一批 question id(真实数据:1768 题里 129 个 id 跨 bank 重复)。
    fn cross_bank_fixture() -> Connection {
        let dir = tempfile::tempdir().unwrap();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        for (bank, title) in [("a.json", "A"), ("b.json", "B")] {
            std::fs::write(
                banks.join(bank),
                format!(
                    r#"{{
                        "meta": {{"title": "{title}"}},
                        "questions": [
                            {{"id":"dup-1","number":1,"type":"single","question":"网络安全基础","answerKeys":["A"]}},
                            {{"id":"dup-2","number":2,"type":"single","question":"网络安全进阶","answerKeys":["B"]}}
                        ]
                    }}"#
                ),
            )
            .unwrap();
        }
        let mut conn = Connection::open_in_memory().unwrap();
        crate::index::build_with_source(&mut conn, &JsonBanksSource, dir.path()).unwrap();
        conn
    }

    /// P1-1 回归:FTS 命中不得重复返回同一条问题。
    ///
    /// 修复前:`q` 单列主键 + REPLACE(跨 bank 同 id 互相顶掉)、`q_fts` 普通 INSERT
    /// (不去重)→ `JOIN q ON q.id = q_fts.id` 一对多扇出 → 同一条命中出现两次,
    /// `LIMIT 16` 实际只给 8 个不同问题。
    #[test]
    fn fts_search_returns_each_question_once() {
        let conn = cross_bank_fixture();
        let hits = search(&conn, &SearchQuery::new("网络安全").with_limit(16)).unwrap();
        assert_eq!(hits.len(), 4, "2 bank × 2 题都应命中");

        let mut keys: Vec<(String, String)> =
            hits.iter().map(|h| (h.src.clone(), h.id.clone())).collect();
        keys.sort();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "同一条命中不应重复返回(P1-1 扇出)");
    }

    /// P1-1 回归:`LIMIT` 必须作用在**不同问题**上。
    /// 修复前:limit=16 只给 8 个不同问题(另一半是重复行)。
    #[test]
    fn limit_counts_distinct_questions() {
        let conn = cross_bank_fixture();
        let hits = search(&conn, &SearchQuery::new("网络安全").with_limit(2)).unwrap();
        assert_eq!(hits.len(), 2, "limit=2 应返回 2 行");
        let mut keys: Vec<(String, String)> =
            hits.iter().map(|h| (h.src.clone(), h.id.clone())).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 2, "limit=2 应给出 2 个不同问题");
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

    /// P1-2 回归:LIKE 兜底必须真的把通配符当字面量。
    ///
    /// 修复前:`escape_like` 输出了反斜杠转义,但两条 LIKE 查询都**没有 `ESCAPE '\'`**
    /// —— SQLite 无 ESCAPE 时把 `\` 当普通字面量 → 模式 `%100\%%` 匹配不到字面值
    /// `100%`(实测:含 `100%` 的题永远查不到)。旧测试只断言转义后的字符串,
    /// **从不验证 SQL 是否匹配** —— 整个转义机制坏掉,测试照绿。
    #[test]
    fn like_fallback_treats_percent_as_literal() {
        let conn = percent_fixture();
        // 直接调 try_like:走 search() 的话 FTS 会先命中,钉不住兜底路径
        // (旧测试 `like_fallback_for_substring` 就是这么"测"的 —— 删掉 try_like 它照样绿)。
        let hits = try_like(&conn, &SearchQuery::new("100%").with_limit(10)).unwrap();
        assert_eq!(
            hits.len(),
            1,
            "字面量 '100%' 应命中含百分号的那一题(修复前 0 条)"
        );
        assert!(hits[0].question.contains("100%"));

        // 反向:不存在的字面量不该命中任何题
        let none = try_like(&conn, &SearchQuery::new("999%").with_limit(10)).unwrap();
        assert!(none.is_empty(), "'999%' 不该命中任何题");
    }

    /// 含 `%` 字面量的题干 + 一道干扰题。
    fn percent_fixture() -> Connection {
        let dir = tempfile::tempdir().unwrap();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();
        std::fs::write(
            banks.join("pct.json"),
            r#"{
                "meta": {"title": "百分号"},
                "questions": [
                    {"id":"p-1","number":1,"type":"single","question":"增长率100%的计算方法","answerKeys":["A"]},
                    {"id":"p-2","number":2,"type":"single","question":"增长率100元的计算方法","answerKeys":["B"]}
                ]
            }"#,
        )
        .unwrap();
        let mut conn = Connection::open_in_memory().unwrap();
        crate::index::build_with_source(&mut conn, &JsonBanksSource, dir.path()).unwrap();
        conn
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

    /// Step 1.2 (2026-10-12) regression: oversized query must be rejected
    /// before any FTS5 work to prevent OOM / re-tokenization DoS.
    #[test]
    fn search_rejects_oversized_query() {
        let conn = fixture();
        // 构造一个 MAX_QUERY_BYTES + 1 长度的字符串
        let huge = "a".repeat(MAX_QUERY_BYTES + 1);
        let q = SearchQuery::new(huge);
        let err = search(&conn, &q).expect_err("should reject oversized query");
        assert!(err.to_string().contains("query too long"), "got: {err}");
    }

    /// Step 1.2 (2026-10-12) regression: same cap on search_cards.
    #[test]
    fn search_cards_rejects_oversized_query() {
        let conn = card_fixture();
        let huge = "a".repeat(MAX_QUERY_BYTES + 1);
        let q = SearchQuery::new(huge);
        let err = search_cards(&conn, &q).expect_err("should reject oversized query");
        assert!(err.to_string().contains("query too long"), "got: {err}");
    }

    /// Step 1.2 boundary check: exactly MAX_QUERY_BYTES must still pass
    /// (trim of whitespace could push it slightly over, so test the bare max).
    #[test]
    fn search_accepts_query_at_exact_cap() {
        let conn = fixture();
        let at_cap = "a".repeat(MAX_QUERY_BYTES);
        let q = SearchQuery::new(at_cap);
        // Should not panic with "query too long"; result is empty (no match).
        let hits = search(&conn, &q).expect("at-cap query should not error");
        assert!(hits.is_empty());
    }
}
