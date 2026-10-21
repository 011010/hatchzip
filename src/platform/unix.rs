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
