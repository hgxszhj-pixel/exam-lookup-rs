//! 数据契约:newtype + serde,保证 ID 不混,字段类型稳定。

use serde::{Deserialize, Serialize};

/// 题库 ID:对应 `banks/<name>.json` 的 `name`(去掉 `.json` 后缀)。
///
/// 例:BankId::new("baomi-2024") 来自 `banks/baomi-2024.json`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(transparent)]
pub struct BankId(pub String);

impl BankId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BankId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for BankId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for BankId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// 题目 ID:通常与原 JSON 里的 `id` 字段一致(若缺失则合成 `<bank>-<idx>`)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(transparent)]
pub struct QuestionId(pub String);

impl QuestionId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for QuestionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for QuestionId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// 题型枚举:对应 newsnow 不需要,但 exam-lookup 必须有。
///
/// 字符串映射宽容:`"single"` / `"单选"` / `"single_choice"` 都映射到 `Single`。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum QuestionType {
    Single,   // 单选
    Multiple, // 多选
    Judge,    // 判断
    Fill,     // 填空
    Unknown,  // 未识别类型
}

impl QuestionType {
    /// 从字符串解析,容忍多种命名(单选/单选题/multi_choice/single 等)。
    pub fn from_str_loose(s: &str) -> Self {
        let lower = s.trim().to_ascii_lowercase();
        match lower.as_str() {
            "single" | "single_choice" | "single-choice" | "单选" | "单选题" => Self::Single,
            "multiple" | "multi" | "multi_choice" | "multi-choice" | "多选" | "多选题" => {
                Self::Multiple
            }
            "judge" | "boolean" | "true_false" | "true-false" | "判断" | "判断题" => {
                Self::Judge
            }
            "fill" | "fill_blank" | "fill-blank" | "填空" | "填空题" => Self::Fill,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Multiple => "multiple",
            Self::Judge => "judge",
            Self::Fill => "fill",
            Self::Unknown => "unknown",
        }
    }

    /// 是否需要展示选项。单选 / 多选需要;判断 / 填空不需要。
    pub fn needs_options(self) -> bool {
        matches!(self, Self::Single | Self::Multiple)
    }
}

impl std::fmt::Display for QuestionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 单个选项:A/B/C/D 之一 + 文本。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BankOption {
    pub key: String, // "A" / "B" / "C" / "D"
    pub text: String,
}

/// 题库元信息(顶层 `meta` 块)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BankMeta {
    pub title: String,
    #[serde(default)]
    pub external: bool,
    #[serde(default)]
    pub year: Option<String>,
    #[serde(default, rename = "sourceNote")]
    pub source_note: Option<String>,
}

/// 单题。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Question {
    pub id: QuestionId,
    pub bank: BankId,
    #[serde(default)]
    pub number: Option<u32>,
    #[serde(default)]
    pub year: Option<String>,
    pub qtype: QuestionType,
    #[serde(default, rename = "typeLabel")]
    pub type_label: Option<String>,
    pub stem: String,
    #[serde(default)]
    pub options: Vec<BankOption>,
    #[serde(default, rename = "answerKeys", deserialize_with = "string_or_vec")]
    pub answer_keys: Vec<String>,
    #[serde(default, rename = "answerText", deserialize_with = "string_or_vec")]
    pub answer_text: Vec<String>,
    #[serde(default)]
    pub analysis: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default, rename = "srcBank")]
    pub src_bank: Option<String>,
}

/// 宽容反序列化:把「标量」当「单元素数组」。
///
/// 真实题库里 `answerText` / `answerKeys` 有三种形态:
/// - `["30分钟", "20kg"]` —— 数组(多数题)
/// - `"30分钟"` —— 单个字符串(部分填空题只有一个答案)
/// - `null` / 字段缺失 —— 空
///
/// Python 端 `kb_db.py::_blank_answers` 显式兼容了前两种:
/// ```python
/// items = at if isinstance(at, list) else ([at] if at else [])
/// ```
/// Rust 侧移植时只写了 `#[serde(default)]` —— 它只覆盖**字段缺失**,
/// **不覆盖类型不符**,于是标量形态直接让整个题库文件解析失败(P0-1):
/// 一个坏文件 → 整个 `load()` 归零 → 16 个健康题库一起陪葬。
///
/// 数字 / 布尔也按 Python 的 `str(x)` 语义转成字符串。
pub fn string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Scalar {
        Str(String),
        Int(i64),
        Float(f64),
        Bool(bool),
    }

    impl Scalar {
        fn into_string(self) -> String {
            match self {
                Self::Str(s) => s,
                Self::Int(i) => i.to_string(),
                Self::Float(f) => f.to_string(),
                Self::Bool(b) => b.to_string(),
            }
        }
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Many(Vec<Scalar>),
        One(Scalar),
    }

    Ok(match Option::<OneOrMany>::deserialize(deserializer)? {
        None => Vec::new(),
        Some(OneOrMany::Many(v)) => v.into_iter().map(Scalar::into_string).collect(),
        Some(OneOrMany::One(s)) => vec![s.into_string()],
    })
}

/// 问答材料条目(对应 `crawl/问答*.json`)。
///
/// Phase A 暂未实现抓取,只放类型。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CardItem {
    pub id: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_id_newtype_roundtrip() {
        let id = BankId::new("baomi-2024");
        assert_eq!(id.as_str(), "baomi-2024");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"baomi-2024\"");
        let back: BankId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn question_id_display() {
        let id = QuestionId::new("BM-80e028f4");
        assert_eq!(format!("{}", id), "BM-80e028f4");
    }

    #[test]
    fn question_type_loose_parsing() {
        assert_eq!(QuestionType::from_str_loose("single"), QuestionType::Single);
        assert_eq!(QuestionType::from_str_loose("单选"), QuestionType::Single);
        assert_eq!(
            QuestionType::from_str_loose("multi_choice"),
            QuestionType::Multiple
        );
        assert_eq!(
            QuestionType::from_str_loose("多选题"),
            QuestionType::Multiple
        );
        assert_eq!(QuestionType::from_str_loose("judge"), QuestionType::Judge);
        assert_eq!(QuestionType::from_str_loose("判断题"), QuestionType::Judge);
        assert_eq!(
            QuestionType::from_str_loose("fill-blank"),
            QuestionType::Fill
        );
        assert_eq!(
            QuestionType::from_str_loose("unknown_type"),
            QuestionType::Unknown
        );
        assert_eq!(QuestionType::from_str_loose(""), QuestionType::Unknown);
    }

    #[test]
    fn question_type_needs_options() {
        assert!(QuestionType::Single.needs_options());
        assert!(QuestionType::Multiple.needs_options());
        assert!(!QuestionType::Judge.needs_options());
        assert!(!QuestionType::Fill.needs_options());
        assert!(!QuestionType::Unknown.needs_options());
    }

    #[test]
    fn question_type_serde_roundtrip() {
        for qt in [
            QuestionType::Single,
            QuestionType::Multiple,
            QuestionType::Judge,
            QuestionType::Fill,
            QuestionType::Unknown,
        ] {
            let json = serde_json::to_string(&qt).unwrap();
            let back: QuestionType = serde_json::from_str(&json).unwrap();
            assert_eq!(qt, back);
        }
    }

    #[test]
    fn bank_option_serde() {
        let opt = BankOption {
            key: "A".into(),
            text: "测试文本".into(),
        };
        let json = serde_json::to_string(&opt).unwrap();
        assert!(json.contains("\"key\":\"A\""));
        let back: BankOption = serde_json::from_str(&json).unwrap();
        assert_eq!(opt, back);
    }

    #[test]
    fn bank_meta_default_when_missing_fields() {
        let meta: BankMeta = serde_json::from_str(r#"{"title":"测试"}"#).unwrap();
        assert_eq!(meta.title, "测试");
        assert!(!meta.external);
        assert_eq!(meta.year, None);
        assert_eq!(meta.source_note, None);
    }

    #[test]
    fn bank_meta_with_renamed_source_note() {
        let meta: BankMeta = serde_json::from_str(
            r#"{"title":"T","external":true,"year":"2024","sourceNote":"note"}"#,
        )
        .unwrap();
        assert_eq!(meta.title, "T");
        assert!(meta.external);
        assert_eq!(meta.year.as_deref(), Some("2024"));
        assert_eq!(meta.source_note.as_deref(), Some("note"));
    }

    // ================ 宽容反序列化(P0-1) ================

    /// 标量 `answerText` 必须被当成单元素数组 —— 这是真实数据
    /// `banks/web-learned.json` 里的形态,也是 P0-1 的直接触发点。
    #[test]
    fn answer_text_accepts_scalar_string() {
        let q: Question = serde_json::from_str(
            r#"{"id":"x","bank":"b","qtype":"fill","stem":"题干","answerText":"30分钟"}"#,
        )
        .expect("标量 answerText 必须能解析");
        assert_eq!(q.answer_text, vec!["30分钟".to_string()]);
    }

    #[test]
    fn answer_text_accepts_array_null_and_missing() {
        let q: Question = serde_json::from_str(
            r#"{"id":"x","bank":"b","qtype":"fill","stem":"题干","answerText":["a","b"]}"#,
        )
        .unwrap();
        assert_eq!(q.answer_text, vec!["a".to_string(), "b".to_string()]);

        let q: Question = serde_json::from_str(
            r#"{"id":"x","bank":"b","qtype":"fill","stem":"题干","answerText":null}"#,
        )
        .unwrap();
        assert!(q.answer_text.is_empty(), "null → 空");

        let q: Question =
            serde_json::from_str(r#"{"id":"x","bank":"b","qtype":"fill","stem":"题干"}"#).unwrap();
        assert!(q.answer_text.is_empty(), "字段缺失 → 空");
    }

    /// Python 端 `_blank_answers` 用 `str(x)`,数字也接受。
    #[test]
    fn answer_text_accepts_numbers() {
        let q: Question = serde_json::from_str(
            r#"{"id":"x","bank":"b","qtype":"fill","stem":"题干","answerText":20}"#,
        )
        .unwrap();
        assert_eq!(q.answer_text, vec!["20".to_string()]);

        let q: Question = serde_json::from_str(
            r#"{"id":"x","bank":"b","qtype":"fill","stem":"题干","answerText":[1,2.5]}"#,
        )
        .unwrap();
        assert_eq!(q.answer_text, vec!["1".to_string(), "2.5".to_string()]);
    }

    #[test]
    fn answer_keys_accept_scalar_string() {
        let q: Question = serde_json::from_str(
            r#"{"id":"x","bank":"b","qtype":"single","stem":"题干","answerKeys":"A"}"#,
        )
        .unwrap();
        assert_eq!(q.answer_keys, vec!["A".to_string()]);
    }
}
