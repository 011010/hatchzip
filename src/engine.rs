//! In-place ZIP extraction.
//!
//! ZIP entries are compressed independently, so we can extract them from the
//! last one to the first and shrink the archive with a plain truncate after
//! each batch. Truncate works on every filesystem, FAT32 and exFAT included,
//! which is why this path needs no hole punching at all.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use rayon::prelude::*;
use zip::ZipArchive;
use zip::read::ZipArchiveMetadata;

use crate::journal::{Journal, PatchedArchive, Paths};
use crate::{fsutil, platform};

/// Larger batches extract in parallel better, smaller ones redo less work
/// after a crash and keep the archive shrinking in fine steps.
const BATCH_MAX_ENTRIES: usize = 512;
const BATCH_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const BUF_SIZE: usize = 1024 * 1024;

#[derive(Clone)]
pub struct Entry {
    pub index: usize,
    pub path: PathBuf,
    pub header_start: u64,
    pub size: u64,
    pub kind: Kind,
    mode: Option<u32>,
    mtime: Option<std::time::SystemTime>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
}

pub struct Archive {
    pub path: PathBuf,
    pub file: File,
    meta: Arc<ZipArchiveMetadata>,
    /// Extraction order: highest offset first.
    pub entries: Vec<Entry>,
    pub len: u64,
    pub tail_start: u64,
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("opening {} for reading and writing", path.display()))?;
        file.try_lock()
            .map_err(|_| anyhow!("{} is in use by another HatchZip run", path.display()))?;
        let len = file.metadata()?.len();
        let mut zip = ZipArchive::new(&mut file)
            .with_context(|| format!("{} is not a ZIP archive we can read", path.display()))?;
        if zip.has_overlapping_entries()? {
            bail!(
                "entries in this archive overlap each other, which in-place extraction cannot handle safely"
            );
        }
        let meta = zip.metadata();
        let mut tail_start = 0;
        for i in 0..zip.len() {
            let f = zip.by_index_raw(i)?;
            let end = f.data_start().unwrap_or(f.header_start()) + f.compressed_size();
            tail_start = tail_start.max(end);
        }
        let entries = collect_entries(&meta)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            meta,
            entries,
            len,
            tail_start,
        })
    }

    /// Reopen an archive we already started on, using the saved tail.
    pub fn reopen(path: &Path, journal: &Journal, tail: Vec<u8>) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        file.try_lock()
            .map_err(|_| anyhow!("{} is in use by another HatchZip run", path.display()))?;
        let len = file.metadata()?.len();
        if len < journal.committed_len || len > journal.archive_len {
            bail!(
                "{} changed since the last run; refusing to touch it",
                path.display()
            );
        }
        if len > journal.committed_len {
            // We crashed after committing a batch but before truncating.
            file.set_len(journal.committed_len)?;
        }
        let patched = PatchedArchive::new(file.try_clone()?, journal.tail_start, tail);
        let zip = ZipArchive::new(patched)
            .with_context(|| format!("re-reading the saved index of {}", path.display()))?;
        let meta = zip.metadata();
        let entries = collect_entries(&meta)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            meta,
            entries,
            len: journal.archive_len,
            tail_start: journal.tail_start,
        })
    }

    /// Output a crashed run wrote for entries it never committed. The output
    /// folder started empty, so anything at those paths is ours.
    pub fn uncommitted(&self, from: usize, out: &Path) -> Vec<(PathBuf, u64)> {
        let mut found = Vec::new();
        for e in &self.entries[from..] {
            if e.kind != Kind::File {
                continue;
            }
            let dest = out.join(&e.path);
            let mut part = dest.as_os_str().to_owned();
            part.push(".hatchzip-part");
            for p in [dest, PathBuf::from(part)] {
                if let Ok(m) = fs::symlink_metadata(&p)
                    && m.is_file()
                {
                    found.push((p, m.len().next_multiple_of(4096)));
                }
            }
        }
        found
    }

    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }

    /// Bytes freed once `entries[i]` is committed: everything from its local
    /// header up to whatever came after it.
    fn span(&self, i: usize) -> u64 {
        let end = if i == 0 {
            self.len
        } else {
            self.entries[i - 1].header_start
        };
        end - self.entries[i].header_start
    }

    /// Replays the whole extraction one entry at a time and returns
    /// `(peak, end)`: the most free space it ever consumes beyond what is free
    /// now, and how much it consumes once finished. `reclaims` says whether
    /// bytes freed from the archive land on the same disk as the output.
    pub fn space_needed(&self, from: usize, reclaims: bool) -> (u64, i128) {
        // The saved index is written before anything is reclaimed.
        let mut extra: i128 = if from == 0 {
            (self.len - self.tail_start) as i128
        } else {
            0
        };
        let mut peak = extra;
        for i in from..self.entries.len() {
            let on_disk = self.entries[i].size.next_multiple_of(4096) as i128;
            peak = peak.max(extra + on_disk);
            extra += on_disk;
            if reclaims {
                extra -= self.span(i) as i128;
            }
        }
        if reclaims {
            // Whatever precedes the first entry is deleted along with the
            // side files at the very end.
            let stub = self.entries.last().map_or(self.len, |e| e.header_start);
            extra -= (stub + self.len - self.tail_start) as i128;
        }
        (peak.max(0) as u64, extra)
    }

    fn reader(&self) -> io::Result<ZipArchive<File>> {
        let file = File::open(&self.path)?;
        // Safety: `meta` was parsed from this same file (or from its saved
        // tail, which is byte-identical), and we only read entries that sit
        // below the current truncation point.
        Ok(unsafe { ZipArchive::unsafe_new_with_metadata(file, self.meta.clone()) })
    }
}

fn collect_entries(meta: &ZipArchiveMetadata) -> Result<Vec<Entry>> {
    let mut entries = Vec::with_capacity(meta.len());
    let mut seen = HashSet::new();
    for index in 0..meta.len() {
        let e = meta.entry(index)?;
        let name = e.name().map(|n| n.into_owned()).unwrap_or_default();
        if e.encrypted() {
            bail!("'{name}' is password protected, which HatchZip does not support yet");
        }
        let path = e.enclosed_name().ok_or_else(|| {
            anyhow!("'{name}' would be written outside the output folder; refusing")
        })?;
        let kind = if e.is_dir() {
            Kind::Dir
        } else if e.is_symlink() {
            Kind::Symlink
        } else {
            Kind::File
        };
        // Case-insensitive filesystems would let two entries race for one
        // file inside a parallel batch.
        if kind != Kind::Dir && !seen.insert(path.to_string_lossy().to_lowercase()) {
            bail!(
                "'{name}' appears twice in the archive (ignoring case); refusing to guess which one wins"
            );
        }
        entries.push(Entry {
            index,
            path,
            header_start: e.header_start(),
            size: e.size(),
            kind,
            mode: e.unix_mode(),
            mtime: e.last_modified().and_then(fsutil::zip_time),
        });
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.header_start));
    Ok(entries)
}

pub struct Progress {
    pub on_bytes: Box<dyn Fn(u64) + Sync>,
    pub on_commit: Box<dyn Fn(u64) + Sync>,
}

pub enum Outcome {
    Finished,
    Paused,
}

pub struct Run<'a> {
    pub archive: Archive,
    pub journal: Journal,
    pub paths: Paths,
    pub stop: &'a AtomicBool,
    pub progress: Progress,
    /// Free space the run must never eat into.
    pub keep_free: u64,
}

impl Run<'_> {
    pub fn execute(mut self) -> Result<Outcome> {
        let out = self.journal.output.clone();
        fs::create_dir_all(&out)
            .with_context(|| format!("creating output folder {}", out.display()))?;

        while self.journal.done < self.archive.entries.len() {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(Outcome::Paused);
            }
            let start = self.journal.done;
            let end = self.next_batch(start, &out)?;
            let batch = &self.archive.entries[start..end];

            let links = self.extract_batch(batch, &out)?;
            if self.stop.load(Ordering::Relaxed) && links.is_none() {
                return Ok(Outcome::Paused);
            }
            let links = links.expect("batch completed");

            // The journal must never claim a file that a power cut could still
            // take back, since its archive bytes are about to be truncated.
            platform::sync_batch(&out).context("flushing extracted files to disk")?;
            if platform::SYNC_EACH_FILE {
                let dirs: HashSet<PathBuf> = batch
                    .iter()
                    .filter_map(|e| out.join(&e.path).parent().map(Path::to_path_buf))
                    .collect();
                for d in &dirs {
                    fsutil::sync_dir(d)?;
                }
            }
            crash_point("after-extract");

            let new_len = batch.last().map(|e| e.header_start).unwrap();
            self.journal.done = end;
            self.journal.committed_len = new_len;
            self.journal.symlinks.extend(links);
            self.journal.save(&self.paths)?;
            crash_point("after-journal");

            self.archive
                .file
                .set_len(new_len)
                .context("shrinking the archive")?;
            (self.progress.on_commit)(new_len);
        }

        self.finish(&out)?;
        Ok(Outcome::Finished)
    }

    /// Pick entries starting at `start` whose output fits in the space we
    /// have right now. Always takes at least one, or explains why it can't.
    fn next_batch(&self, start: usize, out: &Path) -> Result<usize> {
        let avail = fsutil::available_space(out)?.saturating_sub(self.keep_free);
        let entries = &self.archive.entries;
        let first = &entries[start];
        let first_size = first.size.next_multiple_of(4096);
        if first_size > avail {
            // The upfront check makes this rare: it takes something else on
            // the machine filling the disk while we run.
            bail!(
                "stopped before '{}': it needs {} and only {} is free (keeping {} spare).\n\
                 Nothing is lost. Free up {} and run the same command again to continue.",
                first.path.display(),
                human(first_size),
                human(avail),
                human(self.keep_free),
                human(first_size - avail),
            );
        }
        let budget = avail.min(BATCH_MAX_BYTES.max(first_size));
        let max_entries =
            test_knob("HATCHZIP_BATCH_ENTRIES").map_or(BATCH_MAX_ENTRIES, |n| n as usize);
        let mut used = 0;
        let mut end = start;
        while end < entries.len() && end - start < max_entries {
            let size = entries[end].size.next_multiple_of(4096);
            if end > start && used + size > budget {
                break;
            }
            used += size;
            end += 1;
        }
        Ok(end)
    }

    /// Returns `None` if a stop request interrupted the batch. Nothing in the
    /// archive has been touched at that point, so the batch simply reruns.
    fn extract_batch(
        &self,
        batch: &[Entry],
        out: &Path,
    ) -> Result<Option<Vec<(PathBuf, PathBuf)>>> {
        let results: Vec<Result<Option<(PathBuf, PathBuf)>>> = batch
            .par_iter()
            .map_init(
                || (self.archive.reader(), vec![0u8; BUF_SIZE]),
                |(reader, buf), entry| {
                    let reader = reader
                        .as_mut()
                        .map_err(|e| anyhow!("reopening archive: {e}"))?;
                    self.extract_one(reader, buf, entry, out)
                        .with_context(|| format!("extracting '{}'", entry.path.display()))
                },
            )
            .collect();

        let mut links = Vec::new();
        let mut interrupted = false;
        for r in results {
            match r {
                Ok(Some(link)) => links.push(link),
                Ok(None) => {}
                Err(e) if is_interrupt(&e) => interrupted = true,
                Err(e) => return Err(e),
            }
        }
        Ok(if interrupted { None } else { Some(links) })
    }

    fn extract_one(
        &self,
        zip: &mut ZipArchive<File>,
        buf: &mut [u8],
        entry: &Entry,
        out: &Path,
    ) -> Result<Option<(PathBuf, PathBuf)>> {
        let dest = out.join(&entry.path);
        match entry.kind {
            Kind::Dir => {
                fs::create_dir_all(&dest)?;
                return Ok(None);
            }
            Kind::Symlink => {
                let mut target = Vec::new();
                zip.by_index(entry.index)?.read_to_end(&mut target)?;
                let target = PathBuf::from(String::from_utf8(target)?);
                return Ok(Some((entry.path.clone(), target)));
            }
            Kind::File => {}
        }

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut part = dest.as_os_str().to_owned();
        part.push(".hatchzip-part");
        let part = PathBuf::from(part);

        let result = (|| -> Result<()> {
            let mut src = zip.by_index(entry.index)?;
            let mut dst = File::create(&part)?;
            let mut written = 0u64;
            loop {
                if self.stop.load(Ordering::Relaxed) {
                    return Err(io::Error::from(io::ErrorKind::Interrupted).into());
                }
                // The zip crate checks the CRC when the stream ends, so a
                // read error here is how corruption surfaces.
                let n = src.read(buf)?;
                if n == 0 {
                    break;
                }
                written += n as u64;
                if written > entry.size {
                    bail!("entry is larger than the archive says it is");
                }
                dst.write_all(&buf[..n])?;
                (self.progress.on_bytes)(n as u64);
            }
            if written != entry.size {
                bail!("entry is smaller than the archive says it is");
            }
            if let Some(t) = entry.mtime {
                dst.set_modified(t)?;
            }
            platform::apply_mode(&dst, entry.mode)?;
            if platform::SYNC_EACH_FILE {
                dst.sync_all()?;
            }
            Ok(())
        })();

        if let Err(e) = result {
            let _ = fs::remove_file(&part);
            return Err(e);
        }
        fs::rename(&part, &dest)?;
        Ok(None)
    }

    fn finish(&mut self, out: &Path) -> Result<()> {
        for (link, target) in &self.journal.symlinks {
            let link = out.join(link);
            if let Some(parent) = link.parent() {
                fs::create_dir_all(parent)?;
            }
            platform::create_symlink(&link, target)
                .with_context(|| format!("creating symlink '{}'", link.display()))?;
        }
        // What is left is at most a self-extractor stub in front of the
        // first entry. Our copy of the index goes with it.
        fs::remove_file(&self.archive.path)
            .with_context(|| format!("removing the empty {}", self.archive.path.display()))?;
        self.paths.remove()
    }
}

fn is_interrupt(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<io::Error>())
        .any(|io| io.kind() == io::ErrorKind::Interrupted)
}

/// Settings only the test suite uses. Release builds ignore them.
fn test_knob(name: &str) -> Option<u32> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(name).ok()?.parse().ok()
}

/// Lets the test suite kill the process at the two moments that matter for
/// crash safety: after a batch is on disk, and after it is journaled.
fn crash_point(name: &str) {
    if !cfg!(debug_assertions) || std::env::var("HATCHZIP_CRASH_AT").map_or(true, |v| v != name) {
        return;
    }
    static PASSED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if PASSED.fetch_add(1, Ordering::Relaxed) >= test_knob("HATCHZIP_CRASH_AFTER").unwrap_or(0) {
        std::process::abort();
    }
}

pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// Read every entry and check its CRC without writing anything.
pub fn verify(archive: &Archive, on_bytes: &(dyn Fn(u64) + Sync)) -> Result<()> {
    let failures: Vec<String> = archive
        .entries
        .par_iter()
        .filter(|e| e.kind == Kind::File)
        .map_init(
            || (archive.reader(), vec![0u8; BUF_SIZE]),
            |(reader, buf), entry| -> Option<String> {
                let mut check = || -> Result<()> {
                    let zip = reader.as_mut().map_err(|e| anyhow!("{e}"))?;
                    let mut src = zip.by_index(entry.index)?;
                    loop {
                        let n = src.read(buf)?;
                        if n == 0 {
                            return Ok(());
                        }
                        on_bytes(n as u64);
                    }
                };
                check()
                    .err()
                    .map(|e| format!("{}: {e}", entry.path.display()))
            },
        )
        .flatten()
        .collect();
    if failures.is_empty() {
        return Ok(());
    }
    let mut msg = format!("{} damaged entries:", failures.len());
    for f in failures.iter().take(10) {
        msg.push_str("\n  ");
        msg.push_str(f);
    }
    Err(anyhow!(msg))
}
