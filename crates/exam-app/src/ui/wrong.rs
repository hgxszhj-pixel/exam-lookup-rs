//! WrongBookPage:错题本 + 单题删除 + 全部清空 + 导出 JSON。

use crate::state::AppState;
use dioxus::prelude::*;

#[component]
pub fn WrongBookPage() -> Element {
    let state = use_context::<AppState>();
    let entries = state.wrong_book.entries();
    let mut last_export_path = use_signal(|| Option::<String>::None);
    let mut error_msg = use_signal(|| Option::<String>::None);
    let mut refresh = use_signal(|| 0usize);

    rsx! {
        main {
            id: "main",
            style: "max-width:880px; margin:0 auto; padding:24px 16px;",
            div {
                style: "display:flex; align-items:center; gap:8px; margin-bottom:16px;",
                h1 { style: "margin:0;", "错题本" }
                span { style: "color:var(--muted);", "{entries.len()} 题" }
                span { style: "flex:1;" }
                button {
                    onclick: {
                        let state = state.clone();
                        move |_| {
                            let home = dirs::home_dir()
                                .map(|h| h.join("Desktop").join("exam-wrong.json"))
                                .unwrap_or_else(|| std::path::PathBuf::from("exam-wrong.json"));
                            match state.wrong_book.export_to(&home) {
                                Ok(_) => {
                                    last_export_path.set(Some(home.to_string_lossy().to_string()));
                                    error_msg.set(None);
                                }
                                Err(e) => error_msg.set(Some(e)),
                            }
                        }
                    },
                    class: "primary",
                    disabled: entries.is_empty(),
                    "导出 JSON"
                }
                button {
                    class: "danger",
                    disabled: entries.is_empty(),
                    onclick: {
                        let state = state.clone();
                        move |_| {
                            state.wrong_book.clear();
                            refresh.set(refresh() + 1);
                        }
                    },
                    "全部清空"
                }
            }
            if let Some(p) = last_export_path() {
                p { style: "color:var(--success);", "已导出:{p}" }
            }
            if let Some(e) = error_msg() {
                p { style: "color:var(--danger);", "{e}" }
            }
            if entries.is_empty() {
                p { style: "color:var(--muted);", "暂无错题。Practice 答错自动入库。" }
            } else {
                for e in entries.iter() {
                    {
                        let state = state.clone();
                        let qid = e.question_id.clone();
                        rsx! {
                            div {
                                key: "{e.question_id}",
                                style: "border:1px solid var(--border); border-radius:8px; padding:12px; margin:8px 0; background:var(--card);",
                                div {
                                    style: "color:var(--muted); font-size:12px; display:flex; gap:8px;",
                                    span { "{e.src}" }
                                    span { "·" }
                                    span { "{e.question_id}" }
                                    span { style: "flex:1;" }
                                    button {
                                        class: "danger",
                                        style: "padding:2px 8px; font-size:11px;",
                                        onclick: move |_| {
                                            state.wrong_book.remove(&qid);
                                            refresh.set(refresh() + 1);
                                        },
                                        "删除"
                                    }
                                }
                                p { style: "margin:6px 0;", "{e.question}" }
                                p { style: "margin:0; color:var(--danger);", "我的答案:{e.my_answer}" }
                            }
                        }
                    }
                }
            }
        }
    }
}