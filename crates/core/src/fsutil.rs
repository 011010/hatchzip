use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::platform;

pub fn available_space(path: &Path) -> Result<u64> {
    // Lets the test suite put a disk exactly between two plans. Release
    // builds ignore it.
    if cfg!(debug_assertions)
        && let Some(free) = std::env::var("HATCHZIP_FREE")
            .ok()
            .and_then(|v| v.parse().ok())
    {
        return Ok(free);
    }
    let probe = existing_ancestor(path);
    fs4::available_space(&probe)
        .with_context(|| format!("reading free space of {}", probe.display()))
}

pub fn same_filesystem(a: &Path, b: &Path) -> bool {
    platform::same_filesystem(&existing_ancestor(a), &existing_ancestor(b))
}

/// Whether two absolute paths name the same location. Windows and macOS
/// file systems ignore case by default, so a path typed in another case is
/// still the same one there.
pub fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
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

/// Whether punching holes in files in `dir` really gives space back. The
/// answer depends on the filesystem, and a call can succeed without freeing
/// anything (NTFS on a file not marked sparse), so it is tried on a scratch
/// file and measured rather than assumed. The verdict is logged on debug
/// builds: turning in-place freeing off silently was exactly what made the
/// macOS punch flake in the TODO undiagnosable.
pub fn can_punch(dir: &Path) -> bool {
    let probe = dir.join(format!(".hatchzip-probe-{}", std::process::id()));
    let verdict = punch_probe(&probe);
    let _ = fs::remove_file(&probe);
    if cfg!(debug_assertions) {
        eprintln!(
            "hatchzip: punch probe in {}: {}",
            dir.display(),
            verdict.describe()
        );
    }
    matches!(verdict, Probe::Freed { .. })
}

/// What the scratch-file punch did, kept apart because the causes need
/// different answers: a refusal is the filesystem saying it cannot punch at
/// all, a punch that succeeds without the allocation dropping means the space
/// is held (by a snapshot, or a clone) or reported late, and a failing call
/// is an error in our own probe.
enum Probe {
    Freed { before: u64, after: u64 },
    Refused,
    NotFreed { before: u64, after: u64, len: u64 },
    Failed(std::io::Error),
}

impl Probe {
    fn describe(&self) -> String {
        match self {
            Probe::Freed { before, after } => {
                format!("ok, freed {} of {} bytes", before - after, before)
            }
            Probe::Refused => "refused: the filesystem will not punch".into(),
            Probe::NotFreed { before, after, len } => format!(
                "the punch ran but {after} of {before} bytes are still allocated \
                 (length {len}): the space is held or reported late"
            ),
            Probe::Failed(e) => format!("the probe itself failed: {e}"),
        }
    }
}

fn punch_probe(path: &Path) -> Probe {
    const LEN: u64 = 1 << 20;
    let mut f = match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(f) => f,
        Err(e) => return Probe::Failed(e),
    };
    // Noise, because btrfs, ZFS and NTFS compress zeros or repeats into
    // almost nothing, and then there would be nothing to see freed.
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let noise: Vec<u8> = (0..LEN)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    if let Err(e) = f.write_all(&noise) {
        return Probe::Failed(e);
    }
    if let Err(e) = f.sync_all() {
        return Probe::Failed(e);
    }
    let before = match platform::allocated_bytes(path) {
        Ok(b) => b,
        Err(e) => return Probe::Failed(e),
    };
    match platform::punch_hole(&f, 0, LEN) {
        Ok(true) => {}
        Ok(false) => return Probe::Refused,
        Err(e) => return Probe::Failed(e),
    }
    if let Err(e) = f.sync_all() {
        return Probe::Failed(e);
    }
    let after = match platform::allocated_bytes(path) {
        Ok(a) => a,
        Err(e) => return Probe::Failed(e),
    };
    let len = match f.metadata() {
        Ok(m) => m.len(),
        Err(e) => return Probe::Failed(e),
    };
    if len == LEN && after + LEN / 2 <= before {
        Probe::Freed { before, after }
    } else {
        Probe::NotFreed { before, after, len }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the probe says punching works, a punched range must read as
    /// zeros, give its space back, and leave the length and every other byte
    /// alone. Where it says it doesn't, the engine never punches.
    #[test]
    fn punched_ranges_free_space_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("hatchzip-punch-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data");
        let len = 8usize << 20;
        let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8 + 1).collect();
        fs::write(&path, &data).unwrap();
        // Windows refuses to flush through a read-only handle.
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .sync_all()
            .unwrap();

        let supported = can_punch(&dir);
        eprintln!("punching supported in {}: {supported}", dir.display());
        // CI runs on ext4, APFS and NTFS, which all free punched space, so
        // there a "not supported" would be a bug hiding as a skip.
        if std::env::var_os("HATCHZIP_EXPECT_PUNCH").is_some() {
            assert!(supported, "this filesystem should free punched ranges");
        }
        if supported {
            let before = platform::allocated_bytes(&path).unwrap();
            let f = fs::OpenOptions::new().write(true).open(&path).unwrap();
            // Deliberately unaligned, as the engine's ranges are.
            let (from, to) = (1_000_003u64, 5_000_011u64);
            assert!(platform::punch_hole(&f, from, to - from).unwrap());
            f.sync_all().unwrap();
            let after = platform::allocated_bytes(&path).unwrap();
            let granule = platform::punch_granule(&dir);
            eprintln!("allocated before {before}, after {after}");
            assert!(
                after + (to - from) - 2 * granule <= before,
                "allocation went from {before} to {after}"
            );
            let got = fs::read(&path).unwrap();
            assert_eq!(got.len(), len);
            assert_eq!(got[..from as usize], data[..from as usize]);
            assert_eq!(got[to as usize..], data[to as usize..]);
            // Platforms that align inwards leave the edges as they were.
            assert!(
                got[from as usize..to as usize]
                    .iter()
                    .zip(&data[from as usize..to as usize])
                    .all(|(g, d)| *g == 0 || g == d)
            );
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
