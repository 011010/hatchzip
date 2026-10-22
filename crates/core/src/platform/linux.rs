use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

pub use super::unix::{apply_mode, create_symlink, open_exclusive, same_filesystem, sync_dir};

/// One `syncfs` per batch replaces an fsync per file, which matters when a
/// batch holds thousands of small files.
pub const SYNC_EACH_FILE: bool = false;

pub fn sync_batch(output: &Path) -> io::Result<()> {
    let f = File::open(output)?;
    if unsafe { libc::syncfs(f.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
