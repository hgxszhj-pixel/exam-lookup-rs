//! AskPage:关键词查题 + 命中预览。
//!
//! 简化:输入即时 search(无 debounce,SQLite 16 hits 足够快)。
//! 数字键 1-9 跳转靠按钮组(避免 wasm-bindgen 跨 desktop 依赖)。

use crate::state::{AppState, DbStatus};
use dioxus::prelude::*;

#[component]
pub fn AskPage() -> Element {
    let state = use_context::<AppState>();
    let mut query = use_signal(String::new);
    let mut hits: Signal<Vec<exam_engine::SearchHit>> = use_signal(Vec::new);
    let mut error = use_signal(|| Option::<String>::None);

    // 用独立 clone 的 state 喂每个闭包,避免 move 多重捕获。
    rsx! {
        main {
            id: "main",
            style: "max-width:880px; margin:0 auto; padding:24px 16px;",
            DbStatusBanner { status: state.db_status_snapshot() }
            div {
                style: "display:flex; align-items:center; gap:8px; margin-bottom:16px;",
                input {
                    r#type: "search",
                    id: "search-input",
                    placeholder: "搜索题库(支持中英文,按 / 聚焦)",
                    autofocus: true,
                    value: "{query}",
                    oninput: {
                        let state = state.clone();
                        move |e: Event<FormData>| {
                            let v = e.value().to_string();
                            query.set(v.clone());
                            if v.trim().is_empty() {
                                hits.set(Vec::new());
                                error.set(None);
                                return;
                            }
                            match state.search(&v, None, 16) {
                                Ok(r) => { hits.set(r); error.set(None); }
                                Err(e) => error.set(Some(e)),
                            }
                        }
                    },
                    onkeydown: {
                        let state = state.clone();
                        move |e: Event<KeyboardData>| {
                            if e.key() == Key::Escape {
                                query.set(String::new());
                                hits.set(Vec::new());
                                error.set(None);
                                let _ = state; // 保留 capture,触发 search 副作用可放这
                            }
                        }
                    },
                }
            }
            if let Some(err) = error() {
                p { style: "color:var(--danger);", "{err}" }
            }
            if !query.read().trim().is_empty() && hits.read().is_empty() && error.read().is_none() {
                p { style: "color:var(--muted);", "无命中(尝试缩短关键词)" }
            }
            for (i, h) in hits.read().iter().enumerate() {
                div {
                    key: "{h.id}",
                    style: "border:1px solid var(--border); border-radius:8px; padding:12px; margin:8px 0; background:var(--card);",
                    div {
                        style: "color:var(--muted); font-size:12px; margin-bottom:4px;",
                        "{i+1}. "
                        span {
                            style: "margin-right:8px; padding:2px 6px; border-radius:4px; background:var(--code-bg);",
                            "{h.match_kind:?}"
                        }
                        span { "{h.src}" }
                        span { style: "float:right;", "score {h.score:.3}" }
                    }
                    p { style: "margin:6px 0;", "{h.question}" }
                    p { style: "margin:6px 0; color:var(--success);", "答案:{h.answer}" }
                    p { style: "margin:0; color:var(--muted); font-size:12px;", "id: {h.id}" }
                }
            }
        }
    }
}

/// DB 未就绪时的可执行提示。Ready 且题数 = 0 时同样提示(避免空表零命中)。
#[component]
pub fn DbStatusBanner(status: DbStatus) -> Element {
    match status {
        DbStatus::Unloaded => rsx! {
            div {
                style: "padding:10px 14px; margin-bottom:16px; border:1px solid var(--border); border-radius:6px; color:var(--muted);",
                "DB 尚未加载。请到 Settings 检查 data_root / db_path,然后重启。"
            }
        },
        DbStatus::Error(e) => rsx! {
            div {
                style: "padding:10px 14px; margin-bottom:16px; border:1px solid var(--danger); border-radius:6px; color:var(--danger);",
                "DB 加载失败:{e} — 请到 Settings 确认路径。"
            }
        },
        DbStatus::Ready { questions: 0 } => rsx! {
            div {
                style: "padding:10px 14px; margin-bottom:16px; border:1px solid var(--border); border-radius:6px; color:var(--muted);",
                "DB 已就绪但题数为 0。请确认 data_root/banks 下有题库 JSON,然后重启重建索引。"
            }
        },
        DbStatus::Ready { .. } => rsx! { Fragment {} },
    }
}