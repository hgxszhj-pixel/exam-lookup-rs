//! SettingsPage:主题 / data root / db path 设置 + 持久化 + 重建索引(Phase H)。

use crate::state::{AppSettings, AppState, DbStatus, Theme, apply_theme_to_root, save_settings};
use dioxus::prelude::*;

#[component]
pub fn SettingsPage() -> Element {
    let state = use_context::<AppState>();
    let current = state.settings.read().ok().map(|s| s.clone()).unwrap_or_default();
    let mut data_root = use_signal(|| current.data_root.to_string_lossy().to_string());
    let mut db_path = use_signal(|| current.db_path.to_string_lossy().to_string());
    let mut theme = use_signal(|| current.theme);
    let mut auto_rebuild = use_signal(|| current.auto_rebuild);
    let mut last_msg = use_signal(|| Option::<String>::None);
    let mut rebuild_msg = use_signal(|| Option::<String>::None);
    let mut rebuilding = use_signal(|| false);

    rsx! {
        main {
            id: "main",
            style: "max-width:760px; margin:0 auto; padding:24px 16px;",
            h1 { "设置" }
            // ── 当前 DB 状态徽章 ──────────────────────────────────────────
            {
                let s = state.db_status_snapshot();
                match s {
                    DbStatus::Ready { questions } => rsx! {
                        p { style: "margin:8px 0; color:var(--success);",
                            "DB 已就绪 — {questions} 道题" }
                    },
                    DbStatus::Error(e) => rsx! {
                        p { style: "margin:8px 0; color:var(--danger);", "DB 错误:{e}" }
                    },
                    DbStatus::Unloaded => rsx! {
                        p { style: "margin:8px 0; color:var(--muted);", "DB 未加载" }
                    },
                }
            }
            div {
                style: "margin:16px 0;",
                label {
                    style: "display:block; margin-bottom:4px; color:var(--muted);",
                    "data_root 题库目录"
                }
                input {
                    r#type: "text",
                    value: "{data_root}",
                    oninput: move |e| data_root.set(e.value().to_string()),
                }
            }
            div {
                style: "margin:16px 0;",
                label {
                    style: "display:block; margin-bottom:4px; color:var(--muted);",
                    "db_path 索引文件"
                }
                input {
                    r#type: "text",
                    value: "{db_path}",
                    oninput: move |e| db_path.set(e.value().to_string()),
                }
            }
            div {
                style: "margin:16px 0;",
                label {
                    style: "display:block; margin-bottom:4px; color:var(--muted);",
                    "主题"
                }
                div {
                    style: "display:flex; align-items:center; gap:8px;",
                    select {
                        style: "flex:1;",
                        onchange: move |e| {
                            let next = match e.value().as_str() {
                                "light" => Theme::Light,
                                "dark" => Theme::Dark,
                                _ => Theme::System,
                            };
                            theme.set(next);
                            // Phase I+:主题实时预览,选中即生效;「保存」按钮只负责落盘。
                            apply_theme_to_root(next);
                        },
                        option { value: "system", selected: theme() == Theme::System, "跟随系统" }
                        option { value: "light",  selected: theme() == Theme::Light,  "亮色" }
                        option { value: "dark",   selected: theme() == Theme::Dark,   "暗色" }
                    }
                    button {
                        class: "secondary",
                        style: "padding:4px 10px; font-size:12px;",
                        title: "依次切换:跟随系统 → 亮色 → 暗色",
                        onclick: move |_| {
                            let next = match theme() {
                                Theme::System => Theme::Light,
                                Theme::Light => Theme::Dark,
                                Theme::Dark => Theme::System,
                            };
                            theme.set(next);
                            apply_theme_to_root(next);
                        },
                        "↻"
                    }
                }
            }
            // ── 主题色板预览(零事件:跟随根 data-theme / 当前 CSS 变量) ──
            div {
                style: "display:flex; gap:6px; margin:6px 0 4px;",
                div { title: "bg",     style: "width:28px; height:28px; border-radius:4px; background:var(--bg);     border:1px solid var(--border);" }
                div { title: "card",   style: "width:28px; height:28px; border-radius:4px; background:var(--card);   border:1px solid var(--border);" }
                div { title: "accent", style: "width:28px; height:28px; border-radius:4px; background:var(--accent); border:1px solid var(--border);" }
                div { title: "success",style: "width:28px; height:28px; border-radius:4px; background:var(--success);border:1px solid var(--border);" }
                div { title: "danger", style: "width:28px; height:28px; border-radius:4px; background:var(--danger); border:1px solid var(--border);" }
                span { style: "margin-left:8px; color:var(--muted); font-size:12px; align-self:center;",
                    match theme() {
                        Theme::System => "跟随系统(随 macOS 切换)",
                        Theme::Light => "亮色 · 白底蓝点",
                        Theme::Dark => "暗色 · 黑底蓝点",
                    }
                }
            }
            // ── Phase M:data_root 自动重建 toggle ────────────────────────
            div {
                style: "margin:16px 0; padding:8px 12px; border:1px solid var(--border); border-radius:6px;",
                label {
                    style: "display:flex; align-items:center; gap:8px; cursor:pointer;",
                    input {
                        r#type: "checkbox",
                        checked: auto_rebuild(),
                        onchange: {
                            let state = state.clone();
                            move |e| {
                                let next = e.value() == "true";
                                auto_rebuild.set(next);
                                if next {
                                    let _ = state.ensure_watcher();
                                } else {
                                    state.stop_watcher();
                                }
                            }
                        },
                    }
                    span { "data_root 变化时自动重建索引" }
                }
                p {
                    style: "margin:6px 0 0 24px; color:var(--muted); font-size:12px;",
                    if auto_rebuild() {
                        "已启用 — 监听 data_root 文件变化(300ms debounce)"
                    } else {
                        "未启用 — 需手动点「重建索引」"
                    }
                }
            }
            div {
                style: "display:flex; gap:12px; margin-top:8px;",
                button {
                    class: "primary",
                    onclick: {
                        let state = state.clone();
                        move |_| {
                            let new_settings = AppSettings {
                                theme: theme(),
                                data_root: std::path::PathBuf::from(data_root()),
                                db_path: std::path::PathBuf::from(db_path()),
                                auto_rebuild: auto_rebuild(),
                            };
                            match save_settings(&new_settings) {
                                Ok(_) => {
                                    apply_theme_to_root(new_settings.theme);
                                    if let Ok(mut g) = state.settings.write() {
                                        *g = new_settings;
                                    }
                                    last_msg.set(Some("已保存(主题立即生效;DB 路径变更后请「重建索引」)".into()));
                                }
                                Err(e) => last_msg.set(Some(e)),
                            }
                        }
                    },
                    "保存"
                }
                button {
                    class: "secondary",
                    disabled: "{rebuilding()}",
                    onclick: {
                        let state = state.clone();
                        move |_| {
                            rebuilding.set(true);
                            rebuild_msg.set(Some("重建中…".into()));
                            // 用 settings 里 *当前* 的 data_root / db_path,而不是未保存的 signal 值,
                            // 避免"编辑了但没保存就重建"的歧义。
                            let (root, dbp) = match state.settings.read() {
                                Ok(s) => (s.data_root.clone(), s.db_path.clone()),
                                Err(_) => {
                                    rebuild_msg.set(Some("读取设置失败".into()));
                                    rebuilding.set(false);
                                    return;
                                }
                            };
                            match state.rebuild_index(&root, &dbp) {
                                Ok(stats) => {
                                    rebuild_msg.set(Some(format!(
                                        "重建完成:banks={} questions={} cards={} errors={}",
                                        stats.banks, stats.questions, stats.cards, stats.errors
                                    )));
                                }
                                Err(e) => rebuild_msg.set(Some(e)),
                            }
                            rebuilding.set(false);
                        }
                    },
                    if rebuilding() { "重建中…" } else { "重建索引" }
                }
            }
            if let Some(m) = last_msg() {
                p { style: "margin-top:12px; color:var(--success);", "{m}" }
            }
            if let Some(m) = rebuild_msg() {
                p { style: "margin-top:12px; color:var(--muted);", "{m}" }
            }
        }
    }
}