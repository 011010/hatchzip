//! The HatchZip desktop app. The binary in `main.rs` only parses arguments
//! and picks a window; the windows are built here so the integration test
//! can drive them without a display.

pub mod args;
pub mod backend;
mod entries;
pub mod main_window;
mod runner;
mod task;
pub mod task_window;
mod text;
mod work;

slint::include_modules!();
