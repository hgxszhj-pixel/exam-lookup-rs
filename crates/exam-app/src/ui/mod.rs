//! UI 模块导出。

pub mod ask;
pub mod cards;
pub mod practice;
pub mod settings;
pub mod shell;
pub mod wrong;

pub use ask::AskPage;
pub use cards::CardsPage;
pub use practice::PracticePage;
pub use settings::SettingsPage;
pub use shell::{HelpBar, Shell};
pub use wrong::WrongBookPage;