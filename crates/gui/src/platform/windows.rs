use std::path::PathBuf;

/// Explorer, through the per-user registry hive.
pub const SHELL_MENU: bool = true;

/// `%APPDATA%\HatchZip`, which roams with the user's profile.
pub fn config_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(appdata).join("HatchZip"))
}
