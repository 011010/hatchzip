//! The output folders a person chose for the extractions they started, kept
//! in their own profile rather than next to the archive. A journal next to
//! the archive may have been put there by whoever supplied the archive; this
//! record can't have been, so a folder found in it is the person's own
//! choice, and a resume may follow it without being told again.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsutil;

#[derive(Serialize, Deserialize)]
struct Record {
    archive: PathBuf,
    output: PathBuf,
}

/// `%LOCALAPPDATA%\HatchZip`, `~/Library/Application Support/HatchZip`, or
/// `$XDG_STATE_HOME/hatchzip`, which defaults to `~/.local/state/hatchzip`.
fn dir() -> Option<PathBuf> {
    // The test suite keeps its records away from the person running it.
    if cfg!(debug_assertions)
        && let Some(d) = std::env::var_os("HATCHZIP_STATE_DIR")
    {
        return Some(PathBuf::from(d));
    }
    let var = |name| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")?.join("HatchZip")
    } else if cfg!(target_os = "macos") {
        var("HOME")?.join("Library/Application Support/HatchZip")
    } else {
        var("XDG_STATE_HOME")
            .or_else(|| Some(var("HOME")?.join(".local/state")))?
            .join("hatchzip")
    };
    Some(base.join("outputs"))
}

/// One file per archive, named after its path. Two paths sharing a name
/// only cost one of them its record, since each file says whose it is.
fn file(archive: &Path) -> Option<PathBuf> {
    let hash = crc32fast::hash(archive.as_os_str().as_encoded_bytes());
    Some(dir()?.join(format!("{hash:08x}.json")))
}

fn read(archive: &Path) -> Option<(PathBuf, Record)> {
    let file = file(archive)?;
    let record: Record = serde_json::from_slice(&fs::read(&file).ok()?).ok()?;
    fsutil::same_path(&record.archive, archive).then_some((file, record))
}

/// Notes that the person chose `output` for `archive`. If it can't be
/// noted, resuming the extraction just needs the folder named again.
pub(crate) fn remember(archive: &Path, output: &Path) {
    let Some(file) = file(archive) else { return };
    let record = Record {
        archive: archive.to_path_buf(),
        output: output.to_path_buf(),
    };
    if let (Some(parent), Ok(bytes)) = (file.parent(), serde_json::to_vec(&record)) {
        let _ = fs::create_dir_all(parent).map(|_| fsutil::write_atomic(&file, &bytes));
    }
}

pub(crate) fn chose(archive: &Path, output: &Path) -> bool {
    read(archive).is_some_and(|(_, r)| fsutil::same_path(&r.output, output))
}

pub(crate) fn forget(archive: &Path) {
    if let Some((file, _)) = read(archive) {
        let _ = fs::remove_file(file);
    }
}
