//! 题干 / 选项 / 答案 解析与归一化(对齐 Python `solve.py::norm` / `stem_of`)。
//!
//! 关键不变量:
//! - `norm(s)` 与 Python `_PUNC` 正则行为一致(同样的 unicode 标点集合)
//! - `stem_of(s)` 与 Python `stem_of` 一致:剥离 `(X)` 注解 + `A.` 选项前缀 + 尾标点
//! - 解析失败必须返回 `None` 而非 panic;空字符串归一为空
//!
//! **设计取舍**:不引入 `regex` crate 依赖,所有解析用 char-by-char 扫描完成;
//! Python 的 `re.findall(r"([A-D])[.、．]\s*([^A-D]{2,40}?)(?=\s*[A-D]...|$)")`
//! 等价于「切分 + 取首尾非选项字符区间」的状态机。

use std::borrow::Cow;

use crate::types::BankOption;

/// 标点符号集合 — 与 Python `_PUNC` 严格对齐:
/// `\s`，。、；：！？（）《》【】<>,.;:!?()[]"'’‘“”\-—_/\\|`
///
/// 注意:Rust `char` 是 Unicode 标量值;`\u{2018}` 等是单 codepoint,直接比对。
///
/// P0-2 修复:原集合**漏了** `.`(半角句点)与 `。`(U+3002),
/// 而 Python `_PUNC` 两者都有 → `norm("1. 这是题干？")` 在 Rust 是
/// "1.这是题干"、在 Python 是 "1这是题干"。补齐后两边逐字一致。
const PUNCT_CHARS: &[char] = &[
    ' ', '\t', '\r', '\n', // \s
    '\u{FF0C}', ',', // ，
    '\u{3002}', '.', // 。 .
    '\u{3001}', '、', '\u{FF1B}', ';', // ；
    '\u{FF1A}', ':', // ：
    '\u{FF01}', '!', // ！
    '\u{FF1F}', '?', // ？
    '\u{FF08}', '(', // （
    '\u{FF09}', ')', // ）
    '\u{300A}', '<', // 《
    '\u{300B}', '>', // 》
    '\u{3010}', '[', // 【
    '\u{3011}', ']', // 】
    '"', '\'', '\u{2018}', '\u{2019}', // ’‘
    '\u{201C}', '\u{201D}', // “”
    '-', '\u{2014}', // — (em-dash)
    '_', '/', '\\', '|',
];

/// `norm(s)`:删除所有标点与空白 —— 严格对齐 Python `solve.py::norm`:
/// ```python
/// _PUNC = re.compile(r"[\s，。、；：！？（）《》【】<>,.;:!?()\[\]\"'’‘“”\-—_/\\|]+")
/// def norm(s): return _PUNC.sub("", (s or "")).strip()
/// ```
///
/// **不能**把标点折叠成空格(那是旧实现,已改):Python 端是「删除」,
/// 而 `q` 表的 `nq` / `nqstem` 列在 Python(`qb.sqlite3`)与 Rust 两边
/// schema 完全一致 —— 折叠成空格会让同一个库在两套系统里对不上
/// (Python `lookup()` 用 `norm(question) == r[4]` 做精确命中)。
///
/// 空白也算标点:Python 的 `\s` 在 str 模式下匹配 Unicode 空白
/// (U+3000 / U+00A0 等),Rust 用 `char::is_whitespace()` 对齐。
pub fn norm(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if !is_punct(ch) {
            out.push(ch);
        }
    }
    out
}

/// 把标点折叠成单空格 —— 只给 FTS 查询构造用。
///
/// 与 `norm` 的区别:SQLite FTS5 的 unicode61 分词器把标点当**分隔符**,
/// 索引里 "hello,world" 是两个 token("hello" / "world")。查询串若按
/// `norm` 删掉逗号会变成单个 token "helloworld",与索引对不上 → 查不到。
/// 所以构造 MATCH 表达式时必须在标点处断开,这里把标点换成空格来断词。
pub(crate) fn fold_punct_to_space(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(s.len());
    let mut prev_punct = false;
    for ch in s.chars() {
        if is_punct(ch) {
            if !prev_punct && !out.is_empty() {
                out.push(' ');
            }
            prev_punct = true;
        } else {
            out.push(ch);
            prev_punct = false;
        }
    }
    let trimmed = out.trim_matches(|c: char| c.is_whitespace());
    collapse_whitespace(trimmed)
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

fn is_punct(ch: char) -> bool {
    // Python 的 `_PUNC` 里 `\s` 在 str 模式下匹配 Unicode 空白
    // (U+3000 全角空格 / U+00A0 NBSP 等),`PUNCT_CHARS` 只列了 ASCII 空白 → 这里补齐。
    PUNCT_CHARS.contains(&ch) || ch.is_whitespace()
}

/// `stem_of(s)`:剥掉选项块,只留题干 —— 严格对齐 Python `solve.py::stem_of`:
/// ```python
/// def stem_of(s):
///     t = re.split(r"[（(]\s*[)）]\s*", s or "")[0]
///     if norm(t) == norm(s):
///         t = re.split(r"\s*A[.、．]\s*", s or "")[0]
///     return t.strip()
/// ```
///
/// 语义:
/// 1. 切掉**第一个空括号占位**(`（ ）` / `( )`,中间允许空白)之前的内容。
///    注意只切**空**括号 —— `（简称涉密信息系统）` 这种带内容的括号要保留。
/// 2. 若第 1 步没删掉实质内容(归一化后与原文相同)→ 再切「A. / A、/ A．」选项前缀。
///
/// P0-2 修复:旧实现是「切掉第一个 `(`/`（` 之后的所有内容 + 剥前导题号 +
/// 剥尾部标点」,与 Python 无关 —— 例如「坚持（ ）对保守国家秘密工作的领导」
/// 在 Rust 下只剩「坚持」,会让 `nqstem = ?` 精确命中退化成「任意以『坚持』开头
/// 的题」,把无关题当成命中。
pub fn stem_of(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    let t = before_blank_paren(s);
    let t = if norm(&t) == norm(s) {
        // 注意:Python 这里切的是**原文** s,不是 t
        before_option_prefix(s)
    } else {
        t
    };
    t.trim().to_string()
}

/// 第一个「空括号占位」(`（ ）` / `( )`)之前的内容;没有则返回原串。
fn before_blank_paren(s: &str) -> String {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    for i in 0..chars.len() {
        let (start, ch) = chars[i];
        if ch != '(' && ch != '\u{FF08}' {
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && chars[j].1.is_whitespace() {
            j += 1;
        }
        if j < chars.len() && (chars[j].1 == ')' || chars[j].1 == '\u{FF09}') {
            return s[..start].to_string();
        }
    }
    s.to_string()
}

/// 第一个「A. / A、/ A．」选项前缀之前的内容;没有则返回原串。
fn before_option_prefix(s: &str) -> String {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    for i in 0..chars.len() {
        let (idx, ch) = chars[i];
        if ch != 'A' {
            continue;
        }
        if let Some(&(_, next)) = chars.get(i + 1) {
            if next == '.' || next == '\u{3001}' || next == '\u{FF0E}' {
                return s[..idx].to_string();
            }
        }
    }
    s.to_string()
}

/// 把 `Vec<String>` 答案文本用中文顿号 `、` 拼接(对齐 Python `solve.py::build`)。
pub fn join_answer_text(parts: &[String]) -> String {
    parts.join("\u{3001}")
}

/// 在相邻的 CJK 字符之间插入空格,使 SQLite FTS5 unicode61 按字切分。
///
/// 例:`什么是网络安全` → `什 是 网 络 安 全`
///
/// 这样索引列与查询列用同样的「按字 token」形态,FTS5 即可正常 OR 命中。
/// ASCII / whitespace 段保持原样(不被插空)。
pub fn cjk_expand(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut prev: Option<char> = None;
    for ch in s.chars() {
        if is_cjk(ch) {
            if let Some(p) = prev {
                if is_cjk(p) {
                    out.push(' ');
                }
            }
        }
        out.push(ch);
        prev = Some(ch);
    }
    out
}

fn is_cjk(ch: char) -> bool {
    matches!(ch,
        '\u{4E00}'..='\u{9FFF}' |     // CJK Unified
        '\u{3400}'..='\u{4DBF}' |     // CJK Ext A
        '\u{F900}'..='\u{FAFF}'       // CJK Compat
    )
}

/// 从题干中解析出选项字典 `{A: ..., B: ..., ...}`(对齐 Python `solve.py` 的 opts regex)。
///
/// 启发:每出现 `[A-D]<sep>`,扫描到下一个 `[A-D]<sep>` 或字符串末尾,中间文本即选项内容。
pub fn parse_options(question: &str) -> Option<Vec<(char, String)>> {
    let mut out: Vec<(char, String)> = Vec::new();
    let chars: Vec<(usize, char)> = question.char_indices().collect();
    let mut i = 0;
    while i + 1 < chars.len() {
        let (first_idx, first) = chars[i];
        if !matches!(first, 'A'..='D' | 'a'..='d') {
            i += 1;
            continue;
        }
        // 第二字符必须是分隔符
        let (sep_chars, after_sep_idx) = match chars[i + 1].1 {
            '.' => (1usize, i + 2),
            '\u{3001}' | '\u{FF0E}' => (1, i + 2),
            _ => {
                i += 1;
                continue;
            }
        };

        // 跳过分隔符后的空白
        let mut start = after_sep_idx;
        while start < chars.len() && matches!(chars[start].1, ' ' | '\t' | '\n' | '\r') {
            start += 1;
        }
        let start_byte = chars
            .get(start)
            .map(|(idx, _)| *idx)
            .unwrap_or(question.len());

        // 找到下一个选项起点或字符串末尾
        let mut end = chars.len();
        let mut j = start;
        while j < chars.len() {
            let c = chars[j].1;
            if matches!(c, 'A'..='D' | 'a'..='d')
                && j + 1 < chars.len()
                && matches!(chars[j + 1].1, '.' | '\u{3001}' | '\u{FF0E}')
            {
                end = j;
                break;
            }
            j += 1;
        }
        let end_byte = chars
            .get(end)
            .map(|(idx, _)| *idx)
            .unwrap_or(question.len());

        let text = question[start_byte..end_byte].trim_end();
        if !text.is_empty() {
            let key = first.to_ascii_uppercase();
            if !out.iter().any(|(k, _)| *k == key) {
                out.push((key, text.to_string()));
            }
        }

        // 推进:i 到 end 或 after_sep + sep_chars,取大者避免死循环
        i = end.max(after_sep_idx + sep_chars);
        let _ = first_idx; // 抑制未用警告(为日志预留)
    }

    if out.len() >= 2 {
        Some(out)
    } else {
        None
    }
}

/// 把 `[(char, String)]` 转成 `Vec<BankOption>`。
pub fn to_bank_options(opts: &[(char, String)]) -> Vec<BankOption> {
    opts.iter()
        .map(|(k, v)| BankOption {
            key: k.to_string(),
            text: v.clone(),
        })
        .collect()
}

/// 借用版 norm:已干净的输入返回 `Cow::Borrowed`,否则归一化后返回 `Cow::Owned`。
pub fn norm_borrowed(s: &str) -> Cow<'_, str> {
    let needs_norm = s.chars().any(|c| is_punct(c) || c.is_whitespace());
    if needs_norm {
        Cow::Owned(norm(s))
    } else {
        Cow::Borrowed(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P0-2 回归:`norm` 必须与 Python `solve.py::norm` **逐字一致**(删除标点,
    /// 不是折叠成空格)。期望值直接取自 Python 端的 `_PUNC.sub("", s).strip()`。
    ///
    /// 旧实现把标点折叠成空格("hello world"),Python 端是 "helloworld" ——
    /// `q` 表 `nq` / `nqstem` 两边 schema 一致,值不一致会让 Python 的
    /// `lookup()`(`norm(question) == r[4]` 精确命中)对 Rust 建的库全部落空。
    #[test]
    fn norm_strips_punctuation() {
        assert_eq!(norm("hello,world!"), "helloworld");
        assert_eq!(norm("你好,世界!"), "你好世界");
        assert_eq!(norm("a, b,,c"), "abc");
        assert_eq!(norm(""), "");
        assert_eq!(norm("plain"), "plain");
    }

    /// 与 Python `solve.py::norm` 的**跨语言契约**测试。
    ///
    /// 期望值全部由 Python 端实跑产出(见 scripts/dump_banks.py 的 `norm`),
    /// 任何一边改了标点集合或空白处理,这个测试就会红。
    #[test]
    fn norm_matches_python_reference_exactly() {
        let cases: &[(&str, &str)] = &[
            ("hello,world!", "helloworld"),
            ("a  b\t\tc", "abc"),
            ("答:AB", "答AB"),
            ("A、B、C", "ABC"),
            (
                "新修订的《中华人民共和国保守国家秘密法》新增和修改的内容不包括（ ）",
                "新修订的中华人民共和国保守国家秘密法新增和修改的内容不包括",
            ),
            ("题干(X) 后面", "题干X后面"),
            ("", ""),
            ("   ", ""),
            ("1. 这是题干？", "1这是题干"),
            ("中\u{3000}国", "中国"), // 全角空格:Python 的 \s 匹配 Unicode 空白
            ("a\"b", "ab"),
            ("第一， 第二。 第三！", "第一第二第三"),
        ];
        for (input, expected) in cases {
            assert_eq!(&norm(input), expected, "norm({input:?}) 与 Python 不一致");
        }
    }

    /// FTS 查询构造用的折叠版:标点 → 空格(unicode61 分词器把标点当分隔符,
    /// 查询串必须在标点处断词,否则 "hello,world" 变成单 token 查不到)。
    #[test]
    fn fold_punct_to_space_keeps_token_boundaries() {
        assert_eq!(fold_punct_to_space("hello,world!"), "hello world");
        assert_eq!(fold_punct_to_space("a, b,,c"), "a b c");
        assert_eq!(fold_punct_to_space("答:AB"), "答 AB");
        assert_eq!(fold_punct_to_space(""), "");
    }

    /// Step 3.2 (2026-10-12):基本 CJK 串词间插空格(主路径)
    #[test]
    fn cjk_expand_inserts_space_between_cjk() {
        assert_eq!(cjk_expand("你好世界"), "你 好 世 界");
        assert_eq!(cjk_expand("中华人民共和国"), "中 华 人 民 共 和 国");
        // ASCII 段保持原样,不被插空
        assert_eq!(cjk_expand("hello"), "hello");
        assert_eq!(cjk_expand("中a国"), "中a国");
        // CJK Ext A (3400-4DBF) 也算
        assert_eq!(cjk_expand("a𪀀b"), "a𪀀b");
    }

    /// Step 3.2:ideographic space (U+3000) 不是 CJK 字符,被原样保留
    #[test]
    fn cjk_expand_passes_through_ideographic_space() {
        // U+3000 是空格,不是 CJK Unified — 函数不插额外空格
        assert_eq!(cjk_expand("中　国"), "中　国");
        assert_eq!(cjk_expand("　"), "　");
    }

    /// Step 3.2:fullwidth digits (U+FF10-19) 不在 CJK 范围,原样保留(无插空)
    #[test]
    fn cjk_expand_passes_through_fullwidth_digits() {
        // ０１２ 是 fullwidth 数字,非 CJK
        assert_eq!(cjk_expand("中０１国"), "中０１国");
        assert_eq!(cjk_expand("Ａ１"), "Ａ１");
    }

    /// Step 3.2:halfwidth 片假名 (U+FF65-FF9F) 不在 CJK 范围,原样保留
    #[test]
    fn cjk_expand_passes_through_halfwidth_katakana() {
        // ｱ = U+FF71,halfwidth 片假名 — 非 CJK
        assert_eq!(cjk_expand("中ｱ国"), "中ｱ国");
    }

    /// Step 3.2:空串 / 纯 ASCII / 纯 CJK 边界
    #[test]
    fn cjk_expand_edge_cases() {
        assert_eq!(cjk_expand(""), "");
        assert_eq!(cjk_expand("abc"), "abc");
        assert_eq!(cjk_expand("中"), "中");
        // 首尾 CJK 不需要前后空格
        assert_eq!(cjk_expand("中"), "中");
    }

    #[test]
    fn norm_collapses_whitespace() {
        // Python 端把空白一起删掉(`\s` 在 `_PUNC` 字符类里)→ 不是折叠,是删除
        assert_eq!(norm("a  b\t\tc"), "abc");
        assert_eq!(norm("  leading and trailing  "), "leadingandtrailing");
    }

    #[test]
    fn norm_handles_chinese_punctuation() {
        // 答:AB / A、B、C / (双引号“内”)
        assert_eq!(norm("答:AB"), "答AB");
        assert_eq!(norm("A、B、C"), "ABC");
        assert_eq!(norm("\u{201C}双引号\u{201D}内"), "双引号内");
    }

    /// P0-2 回归:带内容的括号**不切**,只切空括号占位(`（ ）`)。
    #[test]
    fn stem_of_keeps_parenthesized_content() {
        // Python 只切「空括号」——`（注）` 有内容,原样保留
        assert_eq!(stem_of("题干(X)"), "题干(X)");
        assert_eq!(stem_of("题干 (注) 后面"), "题干 (注) 后面");
    }

    #[test]
    fn stem_of_strips_option_prefix() {
        // Python `re.split(r"\s*A[.、．]\s*", s)[0]` → 切在 A 之前,选项本身也被切掉
        assert_eq!(stem_of("A. 这是选项"), "");
        assert_eq!(stem_of("A、也是选项"), "");
        assert_eq!(stem_of("题干A.内容"), "题干");
    }

    /// P0-2 回归:Python **不剥**前导题号(旧 Rust 实现会剥)。
    #[test]
    fn stem_of_keeps_leading_question_number() {
        assert_eq!(stem_of("1. 题干开始"), "1. 题干开始");
        assert_eq!(stem_of("10、题干"), "10、题干");
        assert_eq!(stem_of("(1) 题干"), "(1) 题干");
    }

    #[test]
    fn stem_of_empty_inputs() {
        assert_eq!(stem_of(""), "");
        assert_eq!(stem_of("   "), "");
    }

    /// P0-2 回归:Python **不剥**尾部标点(旧 Rust 实现会剥)。
    #[test]
    fn stem_of_keeps_trailing_punctuation() {
        assert_eq!(stem_of("这是题干,后面带逗号。"), "这是题干,后面带逗号。");
        assert_eq!(stem_of("题干尾部.,"), "题干尾部.,");
    }

    /// 与 Python `solve.py::stem_of` 的**跨语言契约**测试。
    ///
    /// 期望值全部由 Python 端实跑产出 —— 任何一边改了切分规则,这个测试就会红。
    #[test]
    fn stem_of_matches_python_reference_exactly() {
        let cases: &[(&str, &str)] = &[
            ("题干(X)", "题干(X)"),
            ("题干 (注) 后面", "题干 (注) 后面"),
            ("A. 这是选项", ""),
            ("A、也是选项", ""),
            ("1. 题干开始", "1. 题干开始"),
            ("10、题干", "10、题干"),
            ("(1) 题干", "(1) 题干"),
            ("", ""),
            ("   ", ""),
            ("这是题干,后面带逗号。", "这是题干,后面带逗号。"),
            ("题干尾部.,", "题干尾部.,"),
            (
                "坚持（ ）对保守国家秘密工作的领导。中央保密工作领导机构。",
                "坚持",
            ),
            (
                "存储、处理国家秘密的计算机信息系统（简称涉密信息系统）按照涉密程度实行（ ）。",
                "存储、处理国家秘密的计算机信息系统（简称涉密信息系统）按照涉密程度实行（ ）。",
            ),
            ("（ ）工作应党而生、伴党而行。", ""),
            ("1. 这是题干？", "1. 这是题干？"),
            ("A. 选项一 B. 选项二", ""),
            ("题干A.内容", "题干"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                &stem_of(input),
                expected,
                "stem_of({input:?}) 与 Python 不一致"
            );
        }
    }

    #[test]
    fn join_answer_text_with_chinese_separator() {
        assert_eq!(join_answer_text(&["甲".into(), "乙".into()]), "甲、乙");
        assert_eq!(join_answer_text(&[]), "");
    }

    #[test]
    fn parse_options_extracts_lettered_choices() {
        let q = "题干。A. 选项一 B. 选项二 C. 选项三";
        let opts = parse_options(q).expect("应能解析出选项");
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[0].0, 'A');
        assert_eq!(opts[0].1, "选项一");
        assert_eq!(opts[2].0, 'C');
    }

    #[test]
    fn parse_options_returns_none_for_short_questions() {
        assert!(parse_options("就一句话").is_none());
        assert!(parse_options("").is_none());
    }

    #[test]
    fn to_bank_options_converts_tuple_list() {
        let opts = vec![('A', "x".into()), ('B', "y".into())];
        let bank = to_bank_options(&opts);
        assert_eq!(bank.len(), 2);
        assert_eq!(bank[0].key, "A");
        assert_eq!(bank[0].text, "x");
    }

    #[test]
    fn norm_borrowed_returns_borrowed_for_clean_input() {
        let owned = norm_borrowed("plaintext");
        assert!(matches!(owned, Cow::Borrowed(_)));
    }

    #[test]
    fn norm_borrowed_returns_owned_for_punctuated_input() {
        let owned = norm_borrowed("with, punct");
        assert!(matches!(owned, Cow::Owned(_)));
    }
}
