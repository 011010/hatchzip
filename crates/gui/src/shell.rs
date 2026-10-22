//! The right-click menu entries, always pointed at the running executable,
//! so an app moved or updated in place re-registers itself correctly.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// What the menu entries should run. Inside an AppImage the executable sits
/// in a mount that disappears when the app exits; the AppImage runtime puts
/// the real file's path in `$APPIMAGE`.
fn launcher() -> Result<PathBuf> {
    if let Some(appimage) = std::env::var_os("APPIMAGE").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(appimage));
    }
    std::env::current_exe().context("finding hatchzip-gui itself")
}

/// Returns a line for each place an entry was written.
pub fn register() -> Result<Vec<String>> {
    let dirs = hatchzip_shell::Dirs::current()?;
    hatchzip_shell::register(&dirs, &launcher()?)
}

pub fn unregister() -> Result<()> {
    hatchzip_shell::unregister(&hatchzip_shell::Dirs::current()?)
}

pub fn set_menu(on: bool) -> Result<()> {
    if on {
        register().map(drop)
    } else {
        unregister()
    }
}
