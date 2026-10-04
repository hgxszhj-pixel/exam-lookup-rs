//! Shell:顶栏(Logo + Tabs + DB 状态徽章 + 帮助按钮)。

use crate::router::Route;
use crate::state::{AppState, DbStatus};
use dioxus::prelude::*;
use dioxus_router::{use_navigator, use_route};

#[component]
pub fn Shell(on_help_toggle: EventHandler<MouseEvent>) -> Element {
    let current = use_route::<Route>();
    let state = use_context::<AppState>();
    let status = state.db_status_snapshot();
    rsx! {
        header {
            style: "display:flex; align-items:center; gap:16px; padding:10px 16px; border-bottom:1px solid var(--border); background:var(--card);",
            strong { "exam-lookup-rs" }
            for (label, target) in [
                ("Ask",      Route::AskPage {}),
                ("Practice", Route::PracticePage {}),
                ("Cards",    Route::CardsPage {}),
                ("Wrong",    Route::WrongBookPage {}),
                ("Settings", Route::SettingsPage {}),
            ] {
                {
                    let active = current == target;
                    let label_s = label.to_string();
                    let target_c = target.clone();
                    rsx! {
                        button {
                            class: if active { "primary" } else { "" },
                            onclick: move |_| {
                                let nav = use_navigator();
                                nav.push(target_c.clone());
                            },
                            "{label_s}"
                        }
                    }
                }
            }
            DbStatusBadge { status }
            span { style: "flex:1;" }
            button {
                title: "帮助 (?)",
                onclick: move |e| on_help_toggle.call(e),
                "?"
            }
        }
    }
}

#[component]
fn DbStatusBadge(status: DbStatus) -> Element {
    let (color, text, title) = match &status {
        DbStatus::Unloaded => ("var(--muted)", "DB: 未加载".to_string(), "尚未尝试加载".to_string()),
        DbStatus::Ready { questions } => ("var(--success)", format!("DB: 就绪 ({questions} 题)"), format!("题数 {questions}")),
        DbStatus::Error(e) => ("var(--danger)", "DB: 错误".to_string(), e.clone()),
    };
    rsx! {
        span {
            style: "color:{color}; font-size:12px; padding:2px 8px; border:1px solid {color}; border-radius:4px;",
            title: "{title}",
            "{text}"
        }
    }
}

#[component]
pub fn HelpBar() -> Element {
    rsx! {
        div {
            style: "position:fixed; right:16px; bottom:16px; padding:12px 14px; border:1px solid var(--border); border-radius:8px; background:var(--card); box-shadow:0 4px 16px rgba(0,0,0,0.12); max-width:360px;",
            strong { "键盘快捷键" }
            ul { style: "margin:8px 0; padding-left:20px; line-height:1.7;",
                li { "/      聚焦搜索框(Ask)" }
                li { "1-9    跳到第 N 个命中(Ask)" }
                li { "j/k    下/上一题(Practice)" }
                li { "A/B/C/D 选答(Practice)" }
                li { "Enter  提交(Practice)" }
                li { "Esc    清空(Ask)/退出(Practice)" }
                li { "?      切换本帮助(全局)" }
            }
            p { style: "margin:0; color:var(--muted); font-size:12px;", "Phase E 简版按钮点击也支持" }
        }
    }
}