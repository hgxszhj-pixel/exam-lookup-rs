//! 路由配置:5 个 Tab + 顶层 ShellLayout(顶栏 + Outlet)。

use crate::ui::{AskPage, CardsPage, PracticePage, SettingsPage, WrongBookPage, HelpBar};
use dioxus::prelude::*;
use dioxus_router::*;

#[rustfmt::skip]
#[derive(Routable, Clone, Debug, PartialEq, Eq)]
pub enum Route {
    #[layout(ShellLayout)]
        #[route("/")]
        Index {},

        #[route("/ask")]
        AskPage {},

        #[route("/practice")]
        PracticePage {},

        #[route("/cards")]
        CardsPage {},

        #[route("/wrong")]
        WrongBookPage {},

        #[route("/settings")]
        SettingsPage {},
    #[end_layout]

    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[component]
pub fn ShellLayout() -> Element {
    let mut show_help = use_signal(|| false);
    // 全局键盘(tabindex 让 root 可聚焦,Dioxus desktop 把 root 作为默认焦点):
    //   ? 切换 HelpBar
    //   / 聚焦 #search-input(Ask/Cards 各自渲染一个同名 id)
    rsx! {
        div {
            tabindex: 0,
            autofocus: true,
            style: "outline:none; min-height:100vh;",
            onkeydown: move |e: Event<KeyboardData>| {
                let key = e.key();
                if key == Key::Character("?".into()) || key == Key::Character("？".into()) {
                    show_help.toggle();
                } else if key == Key::Character("/".into()) {
                    let _ = document::eval(
                        r#"const el = document.getElementById('search-input'); if (el) el.focus();"#
                    );
                }
            },
            crate::ui::Shell {
                on_help_toggle: move |_| show_help.toggle(),
            }
            Outlet::<Route> {}
            if show_help() {
                HelpBar {}
            }
        }
    }
}

#[component]
pub fn Index() -> Element {
    let nav = use_navigator();
    use_effect(move || {
        nav.push(Route::AskPage {});
    });
    rsx! { div { style: "padding:32px;", "loading..." } }
}

#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    rsx! {
        div { style: "padding:32px;",
            h1 { "404 — 未找到" }
            p { "路径:/{segments.join(\"/\")}" }
            Link { to: Route::AskPage {}, "回 Ask" }
        }
    }
}