//! Shared by Linux and macOS.

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

pub fn same_filesystem(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev(),
        _ => false,
    }
}

/// Opens the archive for writing and makes sure no other HatchZip run has it.
/// The lock is advisory, so it never gets in the way of our own readers.
pub fn open_exclusive(path: &Path) -> io::Result<File> {
    let file = fs::OpenOptions::new().read(true).write(true).open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err(io::ErrorKind::WouldBlock.into()),
        Err(fs::TryLockError::Error(e)) => Err(e),
    }
}

/// Bytes the file really occupies on disk. Punching a hole lowers it while
/// the length stays the same.
pub fn allocated_bytes(path: &Path) -> io::Result<u64> {
    Ok(fs::metadata(path)?.blocks() * 512)
}

/// Unix filesystems take any name without `/` or NUL, which path parsing
/// already rules out.
pub fn name_problem(_name: &str) -> Option<&'static str> {
    None
}

pub fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Never recreate setuid, setgid or sticky bits from an archive.
pub fn apply_mode(file: &File, mode: Option<u32>) -> io::Result<()> {
    match mode {
        Some(mode) => file.set_permissions(fs::Permissions::from_mode(mode & 0o777)),
        None => Ok(()),
    }
}

pub fn create_symlink(link: &Path, target: &Path) -> io::Result<()> {
    match fs::symlink_metadata(link) {
        Ok(_) => fs::remove_file(link)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::os::unix::fs::symlink(target, link)
}
