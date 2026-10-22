use std::fs::{self, File};
use std::io;
use std::path::{Component, Path};

const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_DELETE: u32 = 0x4;
const ERROR_SHARING_VIOLATION: i32 = 32;

/// Opens the archive for writing so that nobody else can write to it. A
/// byte-range lock would be mandatory here and block our own readers, so the
/// share mode does the job instead: other handles may read, never write.
pub fn open_exclusive(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(path)
        .map_err(|e| match e.raw_os_error() {
            Some(ERROR_SHARING_VIOLATION) => io::ErrorKind::WouldBlock.into(),
            _ => e,
        })
}

/// `FlushFileBuffers` per file is the only durable flush Windows offers.
pub const SYNC_EACH_FILE: bool = true;

pub fn sync_batch(_output: &Path) -> io::Result<()> {
    Ok(())
}

/// Directory handles can't be flushed here; NTFS journals renames itself.
pub fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

pub fn same_filesystem(a: &Path, b: &Path) -> bool {
    let volume = |p: &Path| {
        fs::canonicalize(p)
            .ok()
            .and_then(|p| match p.components().next() {
                Some(Component::Prefix(prefix)) => {
                    Some(prefix.as_os_str().to_string_lossy().to_lowercase())
                }
                _ => None,
            })
    };
    volume(a).is_some_and(|v| Some(v) == volume(b))
}

pub fn apply_mode(_file: &File, _mode: Option<u32>) -> io::Result<()> {
    Ok(())
}

/// Windows symlinks need admin rights or developer mode, so the target is
/// stored in a small text file instead, the way Git does it.
pub fn create_symlink(link: &Path, target: &Path) -> io::Result<()> {
    fs::write(link, target.to_string_lossy().as_bytes())
}
