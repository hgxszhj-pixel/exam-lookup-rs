//! 应用根:`App` + Router(ShellLayout + Outlet)。

use crate::router::Route;
use crate::state::{AppState, apply_theme_to_root};
use dioxus::prelude::*;
use dioxus_router::Router;

#[component]
pub fn App() -> Element {
    let state = use_context_provider(AppState::bootstrap);
    // 启动时把当前 Theme 写入 html[data-theme],让 CSS 选择器生效。
    apply_theme_to_root(state.settings.read().map(|s| s.theme).unwrap_or_default());

    rsx! {
        style { "{APP_CSS}" }
        Router::<Route> {}
    }
}

const APP_CSS: &str = r#"
:root {
    color-scheme: light dark;
    --bg: #ffffff;
    --fg: #1a1a1a;
    --muted: #6b7280;
    --card: #f8fafc;
    --border: #e5e7eb;
    --accent: #2563eb;
    --accent-fg: #ffffff;
    --danger: #dc2626;
    --success: #16a34a;
    --code-bg: #f3f4f6;
}
@media (prefers-color-scheme: dark) {
    :root {
        --bg: #0b0b0c;
        --fg: #e5e7eb;
        --muted: #9ca3af;
        --card: #15171a;
        --border: #2a2d31;
        --accent: #60a5fa;
        --accent-fg: #0b0b0c;
        --danger: #f87171;
        --success: #4ade80;
        --code-bg: #1f2125;
    }
}
[data-theme="light"] {
    --bg: #ffffff; --fg: #1a1a1a; --muted: #6b7280;
    --card: #f8fafc; --border: #e5e7eb; --accent: #2563eb;
    --accent-fg: #ffffff; --danger: #dc2626; --success: #16a34a;
    --code-bg: #f3f4f6;
}
[data-theme="dark"] {
    --bg: #0b0b0c; --fg: #e5e7eb; --muted: #9ca3af;
    --card: #15171a; --border: #2a2d31; --accent: #60a5fa;
    --accent-fg: #0b0b0c; --danger: #f87171; --success: #4ade80;
    --code-bg: #1f2125;
}
* { box-sizing: border-box; }
html, body, #main { margin: 0; padding: 0; height: 100%; }
body {
    background: var(--bg); color: var(--fg);
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI",
                 "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei",
                 sans-serif;
    font-size: 14px;
    line-height: 1.55;
}
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
button {
    cursor: pointer;
    background: var(--card); color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 6px; padding: 6px 12px;
    font-size: 13px;
}
button:hover { border-color: var(--accent); }
button.primary { background: var(--accent); color: var(--accent-fg); border-color: var(--accent); }
button.danger { color: var(--danger); border-color: var(--danger); }
button:disabled { opacity: 0.5; cursor: not-allowed; }
input[type=text], input[type=search], select, textarea {
    background: var(--bg); color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 10px;
    font-size: 14px;
    width: 100%;
}
code, pre {
    background: var(--code-bg);
    border-radius: 4px;
    padding: 2px 6px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
}
hr { border: none; border-top: 1px solid var(--border); }
"#;

#[allow(unused_imports)]
use AppState as _;