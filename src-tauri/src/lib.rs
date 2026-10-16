pub mod app;
pub mod content;
pub mod context;
#[cfg(feature = "desktop")]
mod desktop;
pub mod drafts;
pub mod feishu;
pub mod generation;
pub mod model;
pub mod reminders;
pub mod sessions;
pub mod types;
#[cfg(feature = "desktop")]
pub use desktop::run;
