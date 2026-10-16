//! egui windows for ShotCraft. Each public module's `run` owns one process's window(s):
//! the capture overlay, editor, Quick Access, pin, scrolling capture, history and settings.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod dialogs;
mod dragout;
pub mod editor;
pub mod history_view;
pub mod notify;
pub mod overlay;
pub mod pin;
mod platform;
pub mod quick;
pub mod scrolling;
pub mod settings_view;
pub mod theme;
