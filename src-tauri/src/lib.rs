pub mod app;
pub mod assistant_references;
pub mod authorized_tools;
pub mod automatic_context;
pub mod content;
pub mod content_url;
pub mod context;
pub mod citation;
pub mod context_bridge;
#[cfg(feature = "desktop")]
mod desktop;
pub mod drafts;
pub mod feishu;
pub mod generation;
pub mod learning_skills;
pub mod learning_tools;
pub mod model;
pub mod native_session;
pub mod reminders;
pub mod sessions;
pub mod time_context;
pub mod types;
#[cfg(feature = "desktop")]
pub use desktop::run;

pub mod assets;
pub mod exercise_references;
pub mod exercises;
pub mod library_tree;
pub mod review;

pub mod qa;

pub mod exploration;
pub mod trash;

pub mod grading;

pub mod anki;

pub mod feishu_bot;
#[path = "../../tools/feishu-bot/bridge.rs"]
pub mod feishu_ws;
