//! exam-app:Dioxus 0.7.9 desktop GUI(Phase E)。

mod app;
mod router;
mod state;
mod ui;
mod wrongbook;

use crate::app::App;

fn main() {
    // Step 1.1 (2026-10-12): initialize tracing subscriber with RUST_LOG filter.
    // `.ok()` swallows "already initialized" errors (e.g. test harnesses).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init()
        .ok();

    dioxus::launch(App);
}