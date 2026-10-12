//! `CrawlCardsSource`:从 `data_root/crawl/*问答.json` 读取问答材料(Phase G)。
//!
//! 数据契约(`crawl/<bank>问答.json`):
//! ```json
//! {
//!   "meta": { "title": "...", "sourceNote": "...", "generatedFrom": "..." },
//!   "questions": [
//!     {
//!       "chapter": "章节",
//!       "num": 1,
//!       "page": null,
//!       "q": "问题...",
//!       "a": "答案...",
//!       "_book": "出处书名",
//!       "_src": "语料库定义句",
//!       "_tested": ""
//!     }
//!   ]
//! }
//! ```
//!
//! 与题库 source(`JsonBanksSource`)的区别:
//! - 不跳任何文件(全部 `*问答.json` 都载入)
//! - 每条 `questions[]` 转 `CardItem { id, title=q, body=a, tags=[chapter, _book] }`
//! - `LoadResult.questions = vec![]`(问答材料非考试题),`LoadResult.cards` 填满
//!
//! 文件名 glob:`<bank>-问答.json`(任意前缀 + `-问答.json`)与 `<bank>问答.json`(无连字符)
//! —— 例:`考点解析-问答.json` / `公开问答-安康保密25题-问答.json` / `数字考点-问答.json`。
//! 简化:用 `contains("问答") && ends_with(".json")` 匹配,排除 `*.bak`。

use crate::error::EngineError;
use crate::source::{LoadResult, Source, SourceEntry};
use crate::types::{BankId, CardItem};
use std::path::Path;

const SUBDIR: &str = "crawl";
const ID: &str = "crawl_cards";
const DISPLAY_NAME: &str = "问答材料 JSON (crawl/问答*.json)";

#[derive(Debug)]
pub struct CrawlCardsSource;

impl CrawlCardsSource {
    pub const ID: &'static str = ID;
    pub const DISPLAY_NAME: &'static str = DISPLAY_NAME;
}

impl Source for CrawlCardsSource {
    fn id(&self) -> &'static str {
        Self::ID
    }
    fn display_name(&self) -> &'static str {
        Self::DISPLAY_NAME
    }
    fn interval(&self) -> crate::source::Interval {
        // 问答材料变更频率与题库同步
        crate::source::Interval::Slow
    }

    fn load(&self, data_root: &Path) -> Result<Vec<LoadResult>, EngineError> {
        let crawl_dir = data_root.join(SUBDIR);
        if !crawl_dir.is_dir() {
            return Err(EngineError::schema(
                crawl_dir,
                format!(
                    "CrawlCardsSource expects `<data_root>/{}/` directory, not found",
                    SUBDIR
                ),
            ));
        }

        let entries =
            std::fs::read_dir(&crawl_dir).map_err(|e| EngineError::io(&crawl_dir, e))?;
        let mut results = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| EngineError::io(&crawl_dir, e))?;
            let path = entry.path();

            // 只处理 .json 后缀且文件名含「问答」,跳过 .bak
            let fname = match path.file_name().and_then(|s| s.to_str()) {
                Some(s) => s,
                None => continue,
            };
            if !fname.ends_with(".json") || !fname.contains("问答") || fname.ends_with(".bak") {
                continue;
            }

            // bank_id = 文件名去掉 ".json" 后缀
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let bank_id = BankId(stem);

            let result = load_one_card_file(&path, bank_id)?;
            if !result.cards.is_empty() {
                results.push(result);
            }
        }

        results.sort_by(|a, b| a.bank_id.cmp(&b.bank_id));
        Ok(results)
    }
}

#[derive(serde::Deserialize)]
struct RawCardFile {
    // meta.title 仅用于兼容原 schema,当前不索引;留 `_meta` 抑制 unused 警告
    #[allow(dead_code)]
    #[serde(default)]
    meta: Option<RawMeta>,
    #[serde(default)]
    questions: Vec<RawCard>,
}

#[derive(serde::Deserialize)]
struct RawMeta {
    #[allow(dead_code)]
    #[serde(default)]
    title: Option<String>,
}

#[derive(serde::Deserialize)]
struct RawCard {
    #[serde(default)]
    chapter: Option<String>,
    #[serde(default)]
    num: Option<u32>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    a: Option<String>,
    #[serde(default, rename = "_book")]
    book: Option<String>,
    #[serde(default, rename = "_src")]
    src: Option<String>,
    #[allow(dead_code)]
    #[serde(default, rename = "_tested")]
    tested: Option<String>,
}

fn load_one_card_file(path: &Path, bank_id: BankId) -> Result<LoadResult, EngineError> {
    // Step 1.3: 用 `load_json_bounded` 替裸 `fs::read` — 64 MiB cap 防 OOM。
    let bytes = crate::source::load_json_bounded(path, crate::source::MAX_JSON_BYTES)?;
    let raw: RawCardFile =
        serde_json::from_slice(&bytes).map_err(|e| EngineError::json(path, e))?;

    let mut cards = Vec::new();
    for (idx, raw_card) in raw.questions.into_iter().enumerate() {
        let q = raw_card.q.unwrap_or_default();
        let a = raw_card.a.unwrap_or_default();
        // 跳过完全空的(q + a 都空)
        if q.trim().is_empty() && a.trim().is_empty() {
            continue;
        }

        let mut tags: Vec<String> = Vec::new();
        if let Some(ch) = raw_card.chapter {
            if !ch.is_empty() {
                tags.push(ch);
            }
        }
        if let Some(bk) = raw_card.book {
            if !bk.is_empty() {
                tags.push(bk);
            }
        }
        // _src = 语料库定义句 — 截前 60 字作为来源 tag(避免过长)
        if let Some(src) = raw_card.src {
            let trimmed: String = src.chars().take(60).collect();
            if !trimmed.is_empty() {
                tags.push(trimmed);
            }
        }

        // id: <bank>-<num or idx> 保持稳定(同一文件多次载入不变)
        let id = match raw_card.num {
            Some(n) => format!("{}-{}", bank_id.0, n),
            None => format!("{}-{}", bank_id.0, idx),
        };

        cards.push(CardItem {
            id,
            title: q,
            body: a,
            tags,
        });
    }

    Ok(LoadResult {
        bank_id,
        meta: None,
        questions: Vec::new(),
        cards,
    })
}

// 自动注册到全局 source 注册表
static CRAWL_CARDS_INSTANCE: CrawlCardsSource = CrawlCardsSource;
inventory::submit!(SourceEntry {
    source: &CRAWL_CARDS_INSTANCE
});

#[cfg(test)]
mod tests {
    use super::*;

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

    fn fixture_dir() -> tempdir::TempDir {
        let dir = tempdir::TempDir::new("exam-engine-crawl-cards").unwrap();
        let crawl = dir.path().join("crawl");
        std::fs::create_dir_all(&crawl).unwrap();

        // 1) 正常 问答 JSON
        std::fs::write(
            crawl.join("示例-问答.json"),
            r#"{
                "meta": {"title": "示例问答", "sourceNote": "test"},
                "questions": [
                    {
                        "chapter": "第一章",
                        "num": 1,
                        "q": "什么是 X?",
                        "a": "X 是指...",
                        "_book": "示例书",
                        "_src": "语料库",
                        "_tested": ""
                    },
                    {
                        "chapter": "第一章",
                        "num": 2,
                        "q": "Y 包括哪些?",
                        "a": "包括 a, b, c",
                        "_book": "示例书"
                    }
                ]
            }"#,
        )
        .unwrap();

        // 2) 应跳过的文件(非「问答」)
        std::fs::write(
            crawl.join("无关.json"),
            r#"{"meta":{"title":"无关"},"questions":[]}"#,
        )
        .unwrap();
        // 3) .bak 后缀跳过
        std::fs::write(
            crawl.join("示例-问答.json.bak"),
            r#"{"questions":[]}"#,
        )
        .unwrap();
        // 4) 全空 q+a 的记录应被跳过
        std::fs::write(
            crawl.join("部分空-问答.json"),
            r#"{
                "meta": {"title": "x"},
                "questions": [
                    {"q": "", "a": "", "num": 1},
                    {"q": "正常问", "a": "正常答", "num": 2}
                ]
            }"#,
        )
        .unwrap();

        dir
    }

    #[test]
    fn source_id_and_display_name() {
        let s = CrawlCardsSource;
        assert_eq!(s.id(), "crawl_cards");
        assert!(s.display_name().contains("问答"));
    }

    #[test]
    fn source_interval_is_slow() {
        let s = CrawlCardsSource;
        assert_eq!(s.interval(), crate::source::Interval::Slow);
    }

    #[test]
    fn load_finds_two_banks_skips_extras() {
        let tmp = fixture_dir();
        let results = CrawlCardsSource.load(tmp.path()).unwrap();
        assert_eq!(results.len(), 2, "示例 + 部分空");
        let names: Vec<&str> = results.iter().map(|r| r.bank_id.as_str()).collect();
        assert_eq!(names, vec!["示例-问答", "部分空-问答"]);
    }

    #[test]
    fn load_parses_card_fields() {
        let tmp = fixture_dir();
        let results = CrawlCardsSource.load(tmp.path()).unwrap();
        let bank = &results[0]; // 示例-问答
        assert_eq!(bank.questions.len(), 0, "问答 source 不产 Question");
        assert_eq!(bank.cards.len(), 2);
        let card = &bank.cards[0];
        assert_eq!(card.id, "示例-问答-1");
        assert_eq!(card.title, "什么是 X?");
        assert_eq!(card.body, "X 是指...");
        assert!(card.tags.contains(&"第一章".to_string()));
        assert!(card.tags.contains(&"示例书".to_string()));
    }

    #[test]
    fn load_skips_blank_q_and_a() {
        let tmp = fixture_dir();
        let results = CrawlCardsSource.load(tmp.path()).unwrap();
        let bank = results
            .iter()
            .find(|r| r.bank_id.as_str() == "部分空-问答")
            .unwrap();
        assert_eq!(bank.cards.len(), 1, "全空 q+a 跳过");
        assert_eq!(bank.cards[0].title, "正常问");
    }

    #[test]
    fn load_synthetic_id_when_num_missing() {
        let tmp = tempdir::TempDir::new("exam-engine-crawl-cards-2").unwrap();
        let crawl = tmp.path().join("crawl");
        std::fs::create_dir_all(&crawl).unwrap();
        std::fs::write(
            crawl.join("无num-问答.json"),
            r#"{"questions":[{"q":"q","a":"a"}]}"#,
        )
        .unwrap();

        let results = CrawlCardsSource.load(tmp.path()).unwrap();
        assert_eq!(results[0].cards[0].id, "无num-问答-0");
    }

    #[test]
    fn load_missing_dir_returns_schema_error() {
        let r = CrawlCardsSource.load(Path::new("/nonexistent/path/should/not/exist"));
        assert!(matches!(r, Err(EngineError::Schema { .. })));
    }

    #[test]
    fn source_is_registered_in_global_registry() {
        let all = crate::source::all_sources();
        assert!(
            all.iter().any(|s| s.id() == CrawlCardsSource::ID),
            "CrawlCardsSource should be registered via inventory"
        );
    }
}
