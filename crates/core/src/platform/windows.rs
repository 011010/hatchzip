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

/// Why Windows can't create a file or folder with this name, if it can't.
/// Checked for every entry before the archive is touched, so a name made on
/// another system fails up front instead of halfway through a run.
pub fn name_problem(name: &str) -> Option<&'static str> {
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    // `CON.txt` is just as reserved as `CON`.
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return Some("it is a reserved device name on Windows");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Some("Windows drops a trailing dot or space from names");
    }
    if name
        .chars()
        .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (c as u32) < 32)
    {
        return Some("it contains a character Windows doesn't allow in names");
    }
    None
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
