use std::io;
use std::path::Path;

pub use super::unix::{apply_mode, create_symlink, open_exclusive, same_filesystem, sync_dir};

/// macOS has no `syncfs`. `File::sync_all` issues `F_FULLFSYNC` there, which
/// is the only call that gets data past the drive's own cache.
pub const SYNC_EACH_FILE: bool = true;

pub fn sync_batch(_output: &Path) -> io::Result<()> {
    Ok(())
}
