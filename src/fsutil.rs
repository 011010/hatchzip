use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::platform;

pub fn available_space(path: &Path) -> Result<u64> {
    let probe = existing_ancestor(path);
    fs4::available_space(&probe)
        .with_context(|| format!("reading free space of {}", probe.display()))
}

pub fn same_filesystem(a: &Path, b: &Path) -> bool {
    platform::same_filesystem(&existing_ancestor(a), &existing_ancestor(b))
}

/// The output folder may not exist yet, so ask about the nearest parent that
/// does.
fn existing_ancestor(path: &Path) -> PathBuf {
    let mut p = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    while !p.exists() {
        if !p.pop() {
            return PathBuf::from(".");
        }
    }
    p
}

/// Write `bytes` to `path` so that after a crash the file holds either the
/// old contents or the new ones, never a mix.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    sync_dir(path.parent().unwrap_or(Path::new(".")))
}

pub fn sync_dir(dir: &Path) -> Result<()> {
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    platform::sync_dir(dir).with_context(|| format!("syncing directory {}", dir.display()))
}

/// ZIP stores local wall-clock time with no zone. We read it as UTC, which is
/// what most cross-platform extractors settle on.
pub fn zip_time(dt: zip::DateTime) -> Option<SystemTime> {
    let (y, m, d) = (
        i64::from(dt.year()),
        i64::from(dt.month()),
        i64::from(dt.day()),
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400
        + i64::from(dt.hour()) * 3600
        + i64::from(dt.minute()) * 60
        + i64::from(dt.second());
    u64::try_from(secs)
        .ok()
        .map(|s| UNIX_EPOCH + Duration::from_secs(s))
}
