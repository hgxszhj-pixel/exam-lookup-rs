//! `JsonBanksSource`:从 `data_root/banks/*.json` 读取题库。
//!
//! 数据契约(`banks/<bank>.json`):
//! ```json
//! {
//!   "meta": { "title": "...", "external": true, "year": "2024", "sourceNote": "..." },
//!   "questions": [
//!     {
//!       "id": "BM-80e028f4", "number": 1, "year": "2024",
//!       "type": "single", "typeLabel": "单选",
//!       "question": "题干",
//!       "options": [{"key":"A","text":"..."}, ...],
//!       "answerKeys": ["D"], "answerText": ["..."],
//!       "analysis": "...", "topic": "...", "srcBank": "..."
//!     }
//!   ]
//! }
//! ```
//!
//! 跳过文件:
//! - `selftest`:测试 fixture,非 selection
//! - `answers_*.json`:答案文件 schema 不同(顶层是 `{<bank>: []}`),交给后续 source 处理
//! - `<name>.json.bak`:备份文件
//!
//! 已知 quirk:若原 `id` 缺失,用 `<bank>-idx` 合成,保证 SQLite 主键不空。

use crate::error::EngineError;
use crate::source::{LoadResult, Source, SourceEntry};
use crate::types::{BankId, BankMeta, BankOption, Question, QuestionId, QuestionType};
use std::path::Path;

/// `banks/*.json` 顶层 schema。
#[derive(serde::Deserialize)]
struct RawBankFile {
    meta: RawMeta,
    questions: Vec<RawQuestion>,
}

#[derive(serde::Deserialize)]
struct RawMeta {
    title: String,
    #[serde(default)]
    external: Option<bool>,
    #[serde(default)]
    year: Option<String>,
    #[serde(default, rename = "sourceNote")]
    source_note: Option<String>,
}

#[derive(serde::Deserialize)]
struct RawQuestion {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    number: Option<u32>,
    #[serde(default)]
    year: Option<String>,
    #[serde(default, rename = "type")]
    qtype: Option<String>,
    #[serde(default, rename = "typeLabel")]
    type_label: Option<String>,
    question: String,
    #[serde(default)]
    options: Vec<RawOption>,
    #[serde(default, rename = "answerKeys")]
    answer_keys: Vec<String>,
    #[serde(default, rename = "answerText")]
    answer_text: Vec<String>,
    #[serde(default)]
    analysis: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default, rename = "srcBank")]
    src_bank: Option<String>,
}

#[derive(serde::Deserialize)]
struct RawOption {
    key: String,
    text: String,
}

#[derive(Debug)]
pub struct JsonBanksSource;

impl JsonBanksSource {
    pub const ID: &'static str = "json_banks";
    pub const DISPLAY_NAME: &'static str = "本地 JSON 题库";
    pub const SUBDIR: &'static str = "banks";
    pub const ANSWER_PREFIX: &'static str = "answers_";
    pub const SELFTEST_NAME: &'static str = "selftest";
    pub const BACKUP_SUFFIX: &'static str = ".bak";
}

impl Source for JsonBanksSource {
    fn id(&self) -> &'static str {
        Self::ID
    }
    fn display_name(&self) -> &'static str {
        Self::DISPLAY_NAME
    }
    fn interval(&self) -> crate::source::Interval {
        // 题库 JSON 来自 exam-lookup Python 仓,变更缓慢
        crate::source::Interval::Slow
    }

    fn load(&self, data_root: &Path) -> Result<Vec<LoadResult>, EngineError> {
        let banks_dir = data_root.join(Self::SUBDIR);
        if !banks_dir.is_dir() {
            return Err(EngineError::schema(
                banks_dir,
                format!(
                    "JsonBanksSource expects `<data_root>/{}/` directory, not found",
                    Self::SUBDIR
                ),
            ));
        }

        let entries = std::fs::read_dir(&banks_dir).map_err(|e| EngineError::io(&banks_dir, e))?;

        let mut results = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| EngineError::io(&banks_dir, e))?;
            let path = entry.path();

            // 只处理 .json 后缀
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            // 跳过答案文件、selftest、备份
            if stem.starts_with(Self::ANSWER_PREFIX)
                || stem == Self::SELFTEST_NAME
                || stem.ends_with(Self::BACKUP_SUFFIX)
            {
                continue;
            }

            let result = load_one_bank(&path, BankId(stem))?;
            results.push(result);
        }

        results.sort_by(|a, b| a.bank_id.cmp(&b.bank_id));
        Ok(results)
    }
}

/// 读取单个 bank 文件并解析为 `LoadResult`。
fn load_one_bank(path: &Path, bank_id: BankId) -> Result<LoadResult, EngineError> {
    let bytes = std::fs::read(path).map_err(|e| EngineError::io(path, e))?;
    let raw: RawBankFile =
        serde_json::from_slice(&bytes).map_err(|e| EngineError::json(path, e))?;

    let meta = BankMeta {
        title: raw.meta.title,
        external: raw.meta.external.unwrap_or(false),
        year: raw.meta.year,
        source_note: raw.meta.source_note,
    };

    let questions = raw
        .questions
        .into_iter()
        .enumerate()
        .map(|(idx, raw_q)| {
            let id = raw_q
                .id
                .filter(|s| !s.is_empty())
                .map(QuestionId::new)
                .unwrap_or_else(|| QuestionId::new(format!("{}-{}", bank_id.0, idx)));

            let qtype = raw_q
                .qtype
                .as_deref()
                .map(QuestionType::from_str_loose)
                .unwrap_or(QuestionType::Unknown);

            let options = raw_q
                .options
                .into_iter()
                .map(|o| BankOption {
                    key: o.key,
                    text: o.text,
                })
                .collect();

            Question {
                id,
                bank: bank_id.clone(),
                number: raw_q.number,
                year: raw_q.year,
                qtype,
                type_label: raw_q.type_label,
                stem: raw_q.question,
                options,
                answer_keys: raw_q.answer_keys,
                answer_text: raw_q.answer_text,
                analysis: raw_q.analysis,
                topic: raw_q.topic,
                src_bank: raw_q.src_bank,
            }
        })
        .collect();

    Ok(LoadResult {
        bank_id,
        meta: Some(meta),
        questions,
        cards: Vec::new(),
    })
}

// 自动注册到全局 source 注册表
static JSON_BANKS_INSTANCE: JsonBanksSource = JsonBanksSource;
inventory::submit!(SourceEntry {
    source: &JSON_BANKS_INSTANCE
});

#[cfg(test)]
mod tests {
    use super::*;

    /// 用临时目录 + fixture JSON 模拟 exam-lookup 的 banks/ 目录。
    fn make_fixture_dir() -> tempdir::TempDir {
        let dir = tempdir::TempDir::new("exam-engine-json-banks").unwrap();
        let banks = dir.path().join("banks");
        std::fs::create_dir_all(&banks).unwrap();

        // 1) 正常 bank
        std::fs::write(
            banks.join("sample-bank.json"),
            r#"{
                "meta": { "title": "示例题库", "external": false, "year": "2024", "sourceNote": "test" },
                "questions": [
                    {
                        "id": "Q-001", "number": 1, "year": "2024",
                        "type": "single", "typeLabel": "单选",
                        "question": "1+1=?",
                        "options": [{"key":"A","text":"1"}, {"key":"B","text":"2"}],
                        "answerKeys": ["B"], "answerText": ["2"],
                        "analysis": "基础算术", "topic": "math", "srcBank": "sample-bank"
                    },
                    {
                        "id": "", "number": 2,
                        "type": "judge", "typeLabel": "判断",
                        "question": "地球是圆的。",
                        "answerKeys": [], "answerText": ["对"]
                    }
                ]
            }"#,
        )
        .unwrap();

        // 2) 应跳过的文件
        std::fs::write(
            banks.join("answers_sample.json"),
            r#"{"sample-bank": []}"#,
        )
        .unwrap();
        std::fs::write(
            banks.join("selftest"),
            r#"{"meta":{"title":"x"},"questions":[]}"#,
        )
        .unwrap();
        std::fs::write(
            banks.join("sample-bank.json.bak"),
            r#"{"meta":{"title":"backup"},"questions":[]}"#,
        )
        .unwrap();

        dir
    }

    // ================ Unit tests for Source trait ================

    #[test]
    fn source_id_and_display_name() {
        let s = JsonBanksSource;
        assert_eq!(s.id(), "json_banks");
        assert_eq!(s.display_name(), "本地 JSON 题库");
    }

    #[test]
    fn source_interval_is_slow() {
        let s = JsonBanksSource;
        assert_eq!(s.interval(), crate::source::Interval::Slow);
    }

    // ================ load() tests with fixture ================

    #[test]
    fn load_finds_one_bank_and_skips_extras() {
        let tmp = make_fixture_dir();
        let results = JsonBanksSource.load(tmp.path()).unwrap();
        assert_eq!(results.len(), 1);
        let bank = &results[0];
        assert_eq!(bank.bank_id.as_str(), "sample-bank");
        assert_eq!(bank.questions.len(), 2);
    }

    #[test]
    fn load_parses_meta_correctly() {
        let tmp = make_fixture_dir();
        let results = JsonBanksSource.load(tmp.path()).unwrap();
        let meta = results[0].meta.as_ref().unwrap();
        assert_eq!(meta.title, "示例题库");
        assert!(!meta.external);
        assert_eq!(meta.year.as_deref(), Some("2024"));
        assert_eq!(meta.source_note.as_deref(), Some("test"));
    }

    #[test]
    fn load_parses_question_fields() {
        let tmp = make_fixture_dir();
        let results = JsonBanksSource.load(tmp.path()).unwrap();
        let q1 = &results[0].questions[0];
        assert_eq!(q1.id.as_str(), "Q-001");
        assert_eq!(q1.number, Some(1));
        assert_eq!(q1.qtype, QuestionType::Single);
        assert_eq!(q1.stem, "1+1=?");
        assert_eq!(q1.options.len(), 2);
        assert_eq!(q1.options[0].key, "A");
        assert_eq!(q1.answer_keys, vec!["B"]);
        assert_eq!(q1.analysis.as_deref(), Some("基础算术"));
        assert_eq!(q1.topic.as_deref(), Some("math"));
        assert_eq!(q1.src_bank.as_deref(), Some("sample-bank"));
    }

    #[test]
    fn load_generates_synthetic_id_when_missing() {
        let tmp = make_fixture_dir();
        let results = JsonBanksSource.load(tmp.path()).unwrap();
        let q2 = &results[0].questions[1];
        // 原 id="" 被替换为 <bank>-1
        assert_eq!(q2.id.as_str(), "sample-bank-1");
        assert_eq!(q2.qtype, QuestionType::Judge);
    }

    #[test]
    fn load_missing_dir_returns_schema_error() {
        let s = JsonBanksSource;
        let result = s.load(Path::new("/nonexistent/path/should/not/exist/at/all"));
        assert!(matches!(result, Err(EngineError::Schema { .. })));
    }

    #[test]
    fn load_skips_answer_files() {
        let tmp = make_fixture_dir();
        let results = JsonBanksSource.load(tmp.path()).unwrap();
        // 应只返回 sample-bank,不应返回 answers_sample
        for r in &results {
            assert!(!r.bank_id.as_str().starts_with("answers_"));
        }
    }

    #[test]
    fn load_results_sorted_by_bank_id() {
        // 在 fixture 里再加一个 bank,验证排序
        let tmp = make_fixture_dir();
        std::fs::write(
            tmp.path().join("banks/aaa-first.json"),
            r#"{"meta":{"title":"a"},"questions":[]}"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("banks/zzz-last.json"),
            r#"{"meta":{"title":"z"},"questions":[]}"#,
        )
        .unwrap();

        let results = JsonBanksSource.load(tmp.path()).unwrap();
        let names: Vec<&str> = results.iter().map(|r| r.bank_id.as_str()).collect();
        assert_eq!(names, vec!["aaa-first", "sample-bank", "zzz-last"]);
    }

    #[test]
    fn source_is_registered_in_global_registry() {
        let all = crate::source::all_sources();
        assert!(
            all.iter().any(|s| s.id() == JsonBanksSource::ID),
            "JsonBanksSource should be registered via inventory"
        );
        let found = crate::source::find_source(JsonBanksSource::ID).unwrap();
        assert_eq!(found.display_name(), "本地 JSON 题库");
    }

    // 把 tempdir 当成本地依赖:用 std 自带的临时目录替代以避免引入 dev-dep
    mod tempdir {
        use std::path::PathBuf;
        pub struct TempDir(PathBuf);
        impl TempDir {
            pub fn new(prefix: &str) -> std::io::Result<Self> {
                let pid = std::process::id();
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let path = std::env::temp_dir()
                    .join(format!("{}-{}-{}", prefix, pid, nanos));
                std::fs::create_dir_all(&path)?;
                Ok(Self(path))
            }
            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}