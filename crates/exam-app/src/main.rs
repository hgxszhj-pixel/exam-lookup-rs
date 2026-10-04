//! exam-app:Dioxus 0.7.9 desktop GUI(Phase E)。

mod app;
mod router;
mod state;
mod ui;
mod wrongbook;

use crate::app::App;

fn main() {
    dioxus::launch(App);
}