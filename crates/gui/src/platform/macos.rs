use std::path::PathBuf;

/// hatchzip-shell can't add Finder entries yet, so the setting is hidden.
pub const SHELL_MENU: bool = false;

/// `~/Library/Application Support/HatchZip`.
pub fn config_dir() -> Option<PathBuf> {
    Some(std::env::home_dir()?.join("Library/Application Support/HatchZip"))
}
