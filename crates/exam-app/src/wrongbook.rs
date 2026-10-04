//! 错题本:题目 ID 集合 + 答错详情 + JSON 导出。
//!
//! Phase E 简化:存题目 id + 题干 + 用户答案 + 正确答案 + 时间戳。
//! 按 question_id 去重(重复答错更新 my_answer + 时间)。
//!
//! 持久化:`~/Library/Application Support/exam-lookup-rs/wrong.json`(macOS)。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WrongEntry {
    pub question_id: String,
    pub src: String,
    pub question: String,
    pub correct_answer: String,
    pub my_answer: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WrongBook {
    entries: Vec<WrongEntry>,
}

impl WrongBook {
    pub fn entries(&self) -> &[WrongEntry] {
        &self.entries
    }

    pub fn add(
        &mut self,
        question_id: impl Into<String>,
        src: impl Into<String>,
        question: impl Into<String>,
        my_answer: impl Into<String>,
    ) -> Result<(), String> {
        let qid = question_id.into();
        let my = my_answer.into();
        let now = crate::state::now_secs();
        if let Some(existing) = self.entries.iter_mut().find(|e| e.question_id == qid) {
            existing.my_answer = my;
            existing.timestamp = now;
            return Ok(());
        }
        let entry = WrongEntry {
            question_id: qid,
            src: src.into(),
            question: question.into(),
            correct_answer: String::new(), // 简化版:不在此层查 DB 拿
            my_answer: my,
            timestamp: now,
        };
        self.entries.push(entry);
        Ok(())
    }

    pub fn remove(&mut self, question_id: &str) {
        self.entries.retain(|e| e.question_id != question_id);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[allow(dead_code)] // 测试用 + 未来 GUI 内联预览
    pub fn export_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(&self.entries).map_err(|e| e.to_string())
    }
}

/// 全局可共享的错题本(线程安全)。
#[derive(Clone)]
pub struct WrongBookStore {
    inner: Arc<RwLock<WrongBook>>,
    path: PathBuf,
}

impl WrongBookStore {
    pub fn load_or_default() -> Self {
        let path = wrong_book_path();
        let book = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<WrongBook>(&s).ok())
                .unwrap_or_default()
        } else {
            WrongBook::default()
        };
        Self {
            inner: Arc::new(RwLock::new(book)),
            path,
        }
    }

    pub fn entries(&self) -> Vec<WrongEntry> {
        self.inner.read().map(|b| b.entries().to_vec()).unwrap_or_default()
    }

    #[allow(dead_code)] // 留作未来 GUI 进度条 / header 角标
    pub fn len(&self) -> usize {
        self.inner.read().map(|b| b.entries().len()).unwrap_or(0)
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn add(
        &self,
        qid: &str,
        src: &str,
        question: &str,
        my_answer: &str,
    ) -> Result<(), String> {
        let mut g = self.inner.write().map_err(|e| e.to_string())?;
        g.add(qid, src, question, my_answer)?;
        let json = serde_json::to_string_pretty(&*g).map_err(|e| e.to_string())?;
        write_atomic(&self.path, &json).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn remove(&self, qid: &str) {
        if let Ok(mut g) = self.inner.write() {
            g.remove(qid);
            if let Ok(json) = serde_json::to_string_pretty(&*g) {
                let _ = write_atomic(&self.path, &json);
            }
        }
    }

    pub fn clear(&self) {
        if let Ok(mut g) = self.inner.write() {
            g.clear();
            if let Ok(json) = serde_json::to_string_pretty(&*g) {
                let _ = write_atomic(&self.path, &json);
            }
        }
    }

    pub fn export_to(&self, path: &std::path::Path) -> Result<(), String> {
        let g = self.inner.read().map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(&*g).map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        std::fs::write(path, json).map_err(|e| e.to_string())
    }
}

fn write_atomic(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, content)
}

fn wrong_book_path() -> PathBuf {
    if let Some(dir) = dirs::data_dir() {
        dir.join("exam-lookup-rs").join("wrong.json")
    } else {
        PathBuf::from("exam-lookup-rs.wrong.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_book_persistence_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrong.json");
        let book = WrongBook::default();
        std::fs::write(&path, serde_json::to_string_pretty(&book).unwrap()).unwrap();
        let read: WrongBook = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(read.entries().is_empty());
    }

    #[test]
    fn wrong_entry_serde_roundtrip() {
        let e = WrongEntry {
            question_id: "q1".into(),
            src: "src".into(),
            question: "Q?".into(),
            correct_answer: "A".into(),
            my_answer: "B".into(),
            timestamp: 12345,
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: WrongEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}