//! CardsPage:背题卡片(Phase G — 接 `card` / `card_fts`)。
//!
//! 数据源:`data_root/crawl/*问答.json` → IndexBuilder → SQLite `card` + `card_fts`。
//! 搜索:`state.search_cards(query, limit)` → `Vec<CardHit>`。

use crate::state::AppState;
use crate::ui::ask::DbStatusBanner;
use dioxus::prelude::*;

#[component]
pub fn CardsPage() -> Element {
    let state = use_context::<AppState>();
    let mut query = use_signal(String::new);
    let mut hits: Signal<Vec<exam_engine::CardHit>> = use_signal(Vec::new);
    let mut error = use_signal(|| Option::<String>::None);

    rsx! {
        main {
            id: "main",
            style: "max-width:880px; margin:0 auto; padding:24px 16px;",
            DbStatusBanner { status: state.db_status_snapshot() }
            h1 { "背题卡片" }
            div {
                style: "display:flex; align-items:center; gap:8px; margin-bottom:16px;",
                input {
                    r#type: "search",
                    id: "search-input",
                    placeholder: "搜索问答材料(crawl/*问答.json,按 / 聚焦)",
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
                            match state.search_cards(&v, 32) {
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
                                let _ = state;
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
            if query.read().trim().is_empty() && hits.read().is_empty() {
                p { style: "color:var(--muted);", "在搜索框输入关键词,匹配 card.title + card.body。" }
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
                    p { style: "margin:6px 0; font-weight:600;", "{h.title}" }
                    p { style: "margin:6px 0;", "{h.body}" }
                    p { style: "margin:0; color:var(--muted); font-size:12px;", "id: {h.id}" }
                }
            }
        }
    }
}
