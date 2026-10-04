//! PracticePage:逐题刷题 + 判分 + 错题本写入。

use crate::state::AppState;
use dioxus::prelude::*;

#[component]
pub fn PracticePage() -> Element {
    let state = use_context::<AppState>();
    let mut questions: Signal<Vec<PracticeQuestion>> = use_signal(Vec::new);
    let mut idx: Signal<usize> = use_signal(|| 0usize);
    let mut picked: Signal<Option<String>> = use_signal(|| Option::<String>::None);
    let mut score: Signal<(usize, usize)> = use_signal(|| (0usize, 0usize));
    let mut exit = use_signal(|| false);
    let mut last_action = use_signal(|| Option::<String>::None);

    rsx! {
        main {
            id: "main",
            style: "max-width:760px; margin:0 auto; padding:24px 16px;",
            div {
                style: "display:flex; align-items:center; gap:8px; margin-bottom:16px;",
                button {
                    onclick: {
                        let state = state.clone();
                        move |_| {
                            exit.set(false);
                            idx.set(0);
                            picked.set(None);
                            score.set((0, 0));
                            let guard = state.conn();
                            match guard {
                                Ok(g) => {
                                    let Some(conn) = g.as_ref() else {
                                        last_action.set(Some("DB 未就绪".into()));
                                        return;
                                    };
                                    let stmt = conn.prepare("SELECT id, src, question, answer FROM q LIMIT 20");
                                    match stmt {
                                        Ok(mut stmt) => {
                                            let rows = stmt.query_map([], |r| {
                                                Ok(PracticeQuestion {
                                                    id: r.get(0)?,
                                                    src: r.get(1)?,
                                                    stem: r.get(2)?,
                                                    correct: r.get(3)?,
                                                })
                                            });
                                            match rows {
                                                Ok(rs) => {
                                                    let v: Vec<_> = rs.filter_map(|r| r.ok()).collect();
                                                    last_action.set(Some(if v.is_empty() {
                                                        "题库为空".into()
                                                    } else {
                                                        format!("加载 {} 题", v.len())
                                                    }));
                                                    questions.set(v);
                                                }
                                                Err(e) => last_action.set(Some(format!("查询失败:{e}"))),
                                            }
                                        }
                                        Err(e) => last_action.set(Some(format!("prepare 失败:{e}"))),
                                    }
                                }
                                Err(e) => last_action.set(Some(format!("DB 锁失败:{e}"))),
                            }
                        }
                    },
                    class: "primary",
                    "加载题目"
                }
                span { style: "color:var(--muted);",
                    if !questions.read().is_empty() {
                        "进度 {idx()+1}/{questions.read().len()} 得分 {score().0}/{score().1}"
                    } else {
                        "未加载"
                    }
                }
                span { style: "flex:1;" }
                button {
                    onclick: move |_| exit.set(true),
                    "退出"
                }
            }
            if exit() {
                p { style: "color:var(--muted);", "已退出。" }
            } else if let Some(q) = questions.read().get(idx()) {
                div {
                    style: "border:1px solid var(--border); border-radius:8px; padding:16px; background:var(--card);",
                    div { style: "color:var(--muted); font-size:12px;", "{q.src} · {q.id}" }
                    h2 { style: "margin:8px 0;", "{q.stem}" }
                    p { style: "color:var(--muted);", "按 A/B/C/D 选答,点击 提交" }
                    div {
                        style: "display:flex; gap:8px; margin-top:12px;",
                        for opt in ["A", "B", "C", "D"] {
                            button {
                                class: if picked() == Some(opt.to_string()) { "primary" } else { "" },
                                onclick: move |_| picked.set(Some(opt.to_string())),
                                "{opt}"
                            }
                        }
                        button {
                            class: "primary",
                            onclick: {
                                let state = state.clone();
                                move |_| {
                                    let qs = questions.read().clone();
                                    let p = picked();
                                    if let (Some(cur), Some(pick)) = (qs.get(idx()), p) {
                                        let correct_contains = cur.correct.to_uppercase().contains(&pick.to_uppercase());
                                        let (mut r, mut t) = score();
                                        t += 1;
                                        if correct_contains {
                                            r += 1;
                                            last_action.set(Some(format!("✓ 答对 ({pick})")));
                                        } else {
                                            let _ = state.wrong_book.add(&cur.id, &cur.src, &cur.stem, &pick);
                                            last_action.set(Some(format!(
                                                "✗ 答错 (你答 {pick}, 正确 {})",
                                                cur.correct
                                            )));
                                        }
                                        score.set((r, t));
                                        if idx() + 1 < qs.len() {
                                            idx.set(idx() + 1);
                                            picked.set(None);
                                        } else {
                                            last_action.set(Some(format!("刷题完成:{r}/{t}")));
                                        }
                                    }
                                }
                            },
                            disabled: picked().is_none(),
                            "提交 (Enter)"
                        }
                    }
                    if let Some(msg) = last_action() {
                        p {
                            style: if msg.starts_with("✓") { "margin-top:12px; color:var(--success);" } else { "margin-top:12px; color:var(--danger);" },
                            "{msg}"
                        }
                    }
                }
            } else if let Some(msg) = last_action() {
                p { "{msg}" }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct PracticeQuestion {
    id: String,
    src: String,
    stem: String,
    correct: String,
}