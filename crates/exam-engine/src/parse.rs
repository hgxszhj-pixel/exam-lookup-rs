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
const PUNCT_CHARS: &[char] = &[
    ' ', '\t', '\r', '\n', // \s
    '\u{FF0C}', ',', // ，
    '\u{3001}', '、',
    '\u{FF1B}', ';', // ；
    '\u{FF1A}', ':', // ：
    '\u{FF01}', '!', // ！
    '\u{FF1F}', '?', // ？
    '\u{FF08}', '(', // （
    '\u{FF09}', ')', // ）
    '\u{300A}', '<', // 《
    '\u{300B}', '>', // 》
    '\u{3010}', '[', // 【
    '\u{3011}', ']', // 】
    '"', '\'',
    '\u{2018}', '\u{2019}', // ’‘
    '\u{201C}', '\u{201D}', // “”
    '-', '\u{2014}', // — (em-dash)
    '_', '/', '\\', '|',
];

/// `norm(s)`:把所有标点折叠成单空格,trim,再压缩连续空白。
pub fn norm(s: &str) -> String {
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
    PUNCT_CHARS.contains(&ch)
}

/// `stem_of(s)`:从题干中剥掉选项标注,只剩真正的题干部分。
///
/// 1. 切掉前导题号(`1.` / `(1)` / `10、`)
/// 2. 切掉 `(X)` 形式注解
/// 3. 切掉 `A.` / `A、` / `A．` 选项前缀(可重复,直到不是选项前缀)
/// 4. 剥掉尾部 `，。` 标点
pub fn stem_of(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }

    let mut t = strip_leading_question_number(s);

    if let Some(idx) = t.find(['(', '\u{FF08}']) {
        t = &t[..idx];
    }

    // 反复切选项前缀(`A. B. C.` 这种多层罕见但允许)
    while let Some(rest) = strip_option_prefix(t) {
        t = rest;
    }

    t.trim()
        .trim_end_matches([',', '.', '\u{FF0C}', '\u{3002}'])
        .trim()
        .to_string()
}

fn strip_leading_question_number(s: &str) -> &str {
    // 简化:`^[（(]?\s*[0-9〜~\-]{1,3}\s*[)）]?[、.．]?\s*`
    // 全部用 char-aware 扫描,避免多字节 UTF-8 切错
    let mut chars = s.char_indices().peekable();

    // 可选 ASCII '('
    if let Some(&(_, '(')) = chars.peek() {
        chars.next();
    }
    // 可选空白
    while let Some(&(_, c)) = chars.peek() {
        if c == ' ' || c == '\t' {
            chars.next();
        } else {
            break;
        }
    }

    let num_start_byte = chars.peek().map(|(idx, _)| *idx).unwrap_or(s.len());
    let mut seen = 0usize;
    while seen < 3 {
        match chars.peek() {
            Some(&(_, c)) if c.is_ascii_digit() || c == '~' || c == '-' || c == '\u{301C}' => {
                chars.next();
                seen += 1;
            }
            _ => break,
        }
    }
    if seen == 0 {
        return s;
    }

    // 可选空白
    while let Some(&(_, c)) = chars.peek() {
        if c == ' ' || c == '\t' {
            chars.next();
        } else {
            break;
        }
    }
    // 可选 ')'
    if let Some(&(_, ')')) = chars.peek() {
        chars.next();
    }
    // 可选 [、.．]
    if let Some(&(_, c)) = chars.peek() {
        if c == ',' || c == '.' || c == '\u{3001}' || c == '\u{FF0E}' {
            chars.next();
        }
    }
    // 可选空白
    while let Some(&(_, c)) = chars.peek() {
        if c == ' ' || c == '\t' {
            chars.next();
        } else {
            break;
        }
    }

    let end_byte = chars.peek().map(|(idx, _)| *idx).unwrap_or(s.len());
    if end_byte == num_start_byte {
        s
    } else {
        &s[end_byte..]
    }
}

fn strip_option_prefix(s: &str) -> Option<&str> {
    let mut chars = s.chars();
    let first = chars.next()?;
    if !matches!(first, 'A'..='D' | 'a'..='d') {
        return None;
    }
    let second = chars.next()?;
    // 第二字符必须是 `.<space>?` 或 `、` / `．`
    let after_sep = match second {
        '.' => 2usize,
        '\u{3001}' | '\u{FF0E}' => 2, // 跳过 A + 分隔符 = 2 个 char
        _ => return None,
    };
    let start_byte = s
        .char_indices()
        .nth(after_sep)
        .map(|(idx, _)| idx)
        .unwrap_or(s.len());
    let rest = &s[start_byte..];
    let trimmed = rest.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed)
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
        let end_byte = chars.get(end).map(|(idx, _)| *idx).unwrap_or(question.len());

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

    #[test]
    fn norm_strips_punctuation() {
        assert_eq!(norm("hello,world!"), "hello world");
        assert_eq!(norm("你好,世界!"), "你好 世界");
        assert_eq!(norm("a, b,,c"), "a b c");
        assert_eq!(norm(""), "");
        assert_eq!(norm("plain"), "plain");
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
        assert_eq!(norm("a  b\t\tc"), "a b c");
        assert_eq!(norm("  leading and trailing  "), "leading and trailing");
    }

    #[test]
    fn norm_handles_chinese_punctuation() {
        // 答:AB / A、B、C / (双引号“内”)
        assert_eq!(norm("答:AB"), "答 AB");
        assert_eq!(norm("A、B、C"), "A B C");
        assert_eq!(norm("\u{201C}双引号\u{201D}内"), "双引号 内");
    }

    #[test]
    fn stem_of_strips_parenthesized_annotations() {
        assert_eq!(stem_of("题干(X)"), "题干");
        assert_eq!(stem_of("题干 (注) 后面"), "题干");
    }

    #[test]
    fn stem_of_strips_option_prefix() {
        assert_eq!(stem_of("A. 这是选项"), "这是选项");
        assert_eq!(stem_of("A、也是选项"), "也是选项");
    }

    #[test]
    fn stem_of_strips_leading_question_number() {
        assert_eq!(stem_of("1. 题干开始"), "题干开始");
        assert_eq!(stem_of("10、题干"), "题干");
        assert_eq!(stem_of("(1) 题干"), "题干");
    }

    #[test]
    fn stem_of_empty_inputs() {
        assert_eq!(stem_of(""), "");
        assert_eq!(stem_of("   "), "");
    }

    #[test]
    fn stem_of_chinese_full_stops() {
        // Python `re.sub(r"[，。,.]*$", "", s)` 只剥尾标点,中间逗号保留
        assert_eq!(stem_of("这是题干,后面带逗号。"), "这是题干,后面带逗号");
        assert_eq!(stem_of("题干尾部.,"), "题干尾部");
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
