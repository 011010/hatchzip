//! In-place extraction, independent of the archive format.
//!
//! Units are processed from the highest offset down. After each batch the
//! archive is truncated to where the batch began, which every filesystem
//! supports, FAT32 and exFAT included, so this path needs no hole punching.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use rayon::prelude::*;

use crate::archive::storage::{Storage, Tail};
use crate::archive::{self, Format, Item, Kind, Sink, Unit};
use crate::journal::{Journal, Link, Paths};
use crate::{fsutil, platform};

/// Larger batches extract in parallel better, smaller ones redo less work
/// after a crash and keep the archive shrinking in fine steps.
const BATCH_MAX_UNITS: usize = 512;
const BATCH_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const BUF_SIZE: usize = 1024 * 1024;
const MAX_SYMLINK: u64 = 64 * 1024;

pub struct Job {
    pub storage: Arc<Storage>,
    pub format_name: &'static str,
    format: Box<dyn Format>,
    /// Units still to do, highest offset first.
    pub units: Vec<Unit>,
    pub tail_start: u64,
    /// Where the archive currently ends: its original length, or wherever
    /// the last commit truncated it.
    pub top: u64,
}

impl Job {
    /// `password` is only needed for encrypted archives. A missing one shows
    /// up as [`archive::PasswordNeeded`], from here or from
    /// [`Job::check_password`].
    pub fn open(path: &Path, writable: bool, password: Option<&str>) -> Result<Self> {
        let storage = Arc::new(Storage::open(path, writable)?);
        let opened = archive::open(&storage, None, password)?;
        let top = storage.original_len();
        Self::build(storage, opened, top)
    }

    /// Reopens an archive a previous run already started on. A read-only
    /// open leaves a pending truncation for the next writable one. The
    /// password is never stored, so an encrypted archive needs it again.
    pub fn resume(
        journal: &Journal,
        tail: Arc<Tail>,
        writable: bool,
        password: Option<&str>,
    ) -> Result<Self> {
        let storage = Arc::new(Storage::from_parts(&journal.parts, writable)?);
        let current = storage.current_len()?;
        if current < journal.committed_len {
            bail!(
                "{} is shorter than HatchZip left it; refusing to touch it",
                storage.primary().display()
            );
        }
        if current > journal.committed_len && writable {
            // The last run committed a batch but stopped before truncating.
            storage.set_len(journal.committed_len)?;
        }
        let opened = archive::open(&storage, Some((&journal.format, tail)), password)?;
        Self::build(storage, opened, journal.committed_len)
    }

    fn build(storage: Arc<Storage>, opened: archive::Opened, top: u64) -> Result<Self> {
        let mut units = opened.units;
        units.retain(|u| u.start < top);
        units.sort_by_key(|u| std::cmp::Reverse(u.start));

        let mut seen = HashSet::new();
        for item in units.iter().flat_map(|u| &u.items) {
            // Case-insensitive filesystems would let two items race for one
            // file inside a parallel batch.
            if item.kind != Kind::Dir && !seen.insert(item.path.to_string_lossy().to_lowercase()) {
                bail!(
                    "'{}' appears twice in the archive (ignoring case); refusing to guess which one wins",
                    item.path.display()
                );
            }
        }
        Ok(Self {
            storage,
            format_name: opened.name,
            format: opened.format,
            units,
            tail_start: opened.tail_start,
            top,
        })
    }

    pub fn encrypted(&self) -> bool {
        self.units.iter().any(|u| u.encrypted)
    }

    /// Proves the password before anything is deleted: decrypts the smallest
    /// encrypted unit, which checks its CRC, up to a limit so a huge solid
    /// block doesn't stall. ZipCrypto accepts about one wrong password in 256
    /// at first sight; reading the data catches those.
    pub fn check_password(&self) -> Result<()> {
        const ENOUGH: u64 = 64 * 1024 * 1024;
        let Some(unit) = self
            .units
            .iter()
            .filter(|u| u.encrypted)
            .min_by_key(|u| u.size())
        else {
            return Ok(());
        };
        let mut worker = self.format.worker(1)?;
        let mut sink = ProbeSink {
            left: ENOUGH,
            buf: vec![0; BUF_SIZE],
        };
        match worker.extract(unit, &mut sink) {
            Ok(()) => Ok(()),
            Err(e) if e.chain().any(|c| c.is::<ProbeDone>()) => Ok(()),
            Err(e) if e.chain().any(|c| c.is::<archive::PasswordNeeded>()) => Err(e),
            Err(_) => Err(archive::WrongPassword.into()),
        }
    }

    pub fn item_count(&self) -> usize {
        self.units.iter().map(|u| u.items.len()).sum()
    }

    pub fn remaining_size(&self) -> u64 {
        self.units.iter().map(Unit::size).sum()
    }

    /// Bytes freed once `units[i]` is committed: everything from its start up
    /// to whatever came after it.
    fn span(&self, i: usize) -> u64 {
        let end = if i == 0 {
            self.top
        } else {
            self.units[i - 1].start
        };
        end - self.units[i].start
    }

    /// Replays the extraction one unit at a time and returns `(peak, end)`:
    /// the most free space it ever consumes beyond what is free now, and how
    /// much it consumes once finished. `fresh` means the tail hasn't been
    /// saved yet; `reclaims` that freed archive space lands on the output's
    /// disk.
    pub fn space_needed(&self, fresh: bool, reclaims: bool) -> (u64, i128) {
        let tail = (self.storage.original_len() - self.tail_start) as i128;
        let mut extra: i128 = if fresh { tail } else { 0 };
        let mut peak = extra;
        for (i, u) in self.units.iter().enumerate() {
            let on_disk: u64 = u
                .items
                .iter()
                .map(|it| it.size.next_multiple_of(4096))
                .sum();
            peak = peak.max(extra + on_disk as i128);
            extra += on_disk as i128;
            if reclaims {
                extra -= self.span(i) as i128;
            }
        }
        if reclaims {
            // Whatever precedes the first unit is deleted at the very end,
            // along with the saved tail.
            let stub = self.units.last().map_or(self.top, |u| u.start);
            extra -= stub as i128 + tail;
        }
        (peak.max(0) as u64, extra)
    }

    /// Output a crashed run wrote for units it never committed. The output
    /// folder started empty, so anything at those paths is ours.
    pub fn uncommitted(&self, out: &Path) -> Vec<(PathBuf, u64)> {
        let mut found = Vec::new();
        for item in self.units.iter().flat_map(|u| &u.items) {
            if item.kind != Kind::File {
                continue;
            }
            let dest = out.join(&item.path);
            for p in [part_path(&dest), dest] {
                if let Ok(m) = fs::symlink_metadata(&p)
                    && m.is_file()
                {
                    found.push((p, m.len().next_multiple_of(4096)));
                }
            }
        }
        found
    }

    /// Reads every unit and lets the decoders check their checksums,
    /// writing nothing.
    pub fn verify(&self, on_bytes: &(dyn Fn(u64) + Sync)) -> Result<()> {
        let threads = threads_per_unit(self.units.len());
        let failures: Vec<String> = self
            .units
            .par_iter()
            .map_init(
                || {
                    (
                        self.format.worker(threads),
                        VerifySink {
                            on_bytes,
                            buf: vec![0; BUF_SIZE],
                        },
                    )
                },
                |(worker, sink), unit| {
                    let mut run = || -> Result<()> {
                        let w = worker.as_mut().map_err(|e| anyhow!("{e:#}"))?;
                        w.extract(unit, sink)
                    };
                    run().err().map(|e| format!("{e:#}"))
                },
            )
            .flatten()
            .collect();
        if failures.is_empty() {
            return Ok(());
        }
        let mut msg = format!("{} damaged:", failures.len());
        for f in failures.iter().take(10) {
            msg.push_str("\n  ");
            msg.push_str(f);
        }
        Err(anyhow!(msg))
    }
}

fn threads_per_unit(units: usize) -> u32 {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    (cores / units.max(1)).max(1) as u32
}

fn part_path(dest: &Path) -> PathBuf {
    let mut part = dest.as_os_str().to_owned();
    part.push(".hatchzip-part");
    PathBuf::from(part)
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
    pub job: Job,
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

        let mut next = 0;
        while next < self.job.units.len() {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(Outcome::Paused);
            }
            let end = self.next_batch(next, &out)?;
            let batch = &self.job.units[next..end];

            let Some(links) = self.extract_batch(batch, &out)? else {
                return Ok(Outcome::Paused);
            };

            // The journal must never claim a file that a power cut could still
            // take back, since its archive bytes are about to be truncated.
            platform::sync_batch(&out).context("flushing extracted files to disk")?;
            if platform::SYNC_EACH_FILE {
                // Links are only recorded here and made at the end, so their
                // folders may not exist yet.
                let dirs: HashSet<PathBuf> = batch
                    .iter()
                    .flat_map(|u| &u.items)
                    .filter(|it| matches!(it.kind, Kind::File | Kind::Dir))
                    .filter_map(|it| out.join(&it.path).parent().map(Path::to_path_buf))
                    .collect();
                for d in &dirs {
                    fsutil::sync_dir(d)?;
                }
            }
            crash_point("after-extract");

            let new_len = batch.last().expect("batches are never empty").start;
            self.journal.committed_len = new_len;
            self.journal.links.extend(links);
            self.journal.save(&self.paths)?;
            crash_point("after-journal");

            self.job
                .storage
                .set_len(new_len)
                .context("shrinking the archive")?;
            (self.progress.on_commit)(new_len);
            next = end;
        }

        self.finish(&out)?;
        Ok(Outcome::Finished)
    }

    /// Pick units starting at `start` whose output fits in the space there is
    /// right now. Always takes at least one, or explains why it can't.
    fn next_batch(&self, start: usize, out: &Path) -> Result<usize> {
        let avail = fsutil::available_space(out)?.saturating_sub(self.keep_free);
        let units = &self.job.units;
        let need = |u: &Unit| {
            u.items
                .iter()
                .map(|i| i.size.next_multiple_of(4096))
                .sum::<u64>()
        };
        let first = need(&units[start]);
        if first > avail {
            // The upfront check makes this rare: it takes something else on
            // the machine filling the disk while we run.
            let what = match units[start].items.as_slice() {
                [one] => format!("'{}'", one.path.display()),
                many => format!("the next block of {} files", many.len()),
            };
            bail!(
                "stopped before {what}: it needs {} and only {} is free (keeping {} spare).\n\
                 Nothing is lost. Free up {} and run the same command again to continue.",
                human(first),
                human(avail),
                human(self.keep_free),
                human(first - avail),
            );
        }
        let budget = avail.min(BATCH_MAX_BYTES.max(first));
        let max_units = test_knob("HATCHZIP_BATCH_ENTRIES").map_or(BATCH_MAX_UNITS, |n| n as usize);
        let mut used = 0;
        let mut end = start;
        while end < units.len() && end - start < max_units {
            let size = need(&units[end]);
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
    fn extract_batch(&self, batch: &[Unit], out: &Path) -> Result<Option<Vec<Link>>> {
        let threads = threads_per_unit(batch.len());
        let on_bytes = &*self.progress.on_bytes;
        let results: Vec<Result<Vec<Link>>> = batch
            .par_iter()
            .map_init(
                || {
                    let sink = DiskSink {
                        out,
                        stop: self.stop,
                        on_bytes,
                        buf: vec![0; BUF_SIZE],
                        links: Vec::new(),
                    };
                    (self.job.format.worker(threads), sink)
                },
                |(worker, sink), unit| {
                    let w = worker
                        .as_mut()
                        .map_err(|e| anyhow!("opening the archive: {e:#}"))?;
                    w.extract(unit, sink)?;
                    Ok(std::mem::take(&mut sink.links))
                },
            )
            .collect();

        let mut links = Vec::new();
        let mut interrupted = false;
        for r in results {
            match r {
                Ok(l) => links.extend(l),
                Err(e) if is_interrupt(&e) => interrupted = true,
                Err(e) => return Err(e),
            }
        }
        Ok((!interrupted).then_some(links))
    }

    fn finish(self, out: &Path) -> Result<()> {
        make_links(&self.journal.links, out)?;
        let Self { job, paths, .. } = self;
        let Job {
            storage, format, ..
        } = job;
        // The format holds its own handle on the storage; it has to go first.
        drop(format);
        let storage = Arc::try_unwrap(storage)
            .map_err(|_| anyhow!("archive still in use at the end of extraction"))?;
        // What is left is at most a self-extractor stub in front of the
        // first entry. The saved index goes with it.
        storage.remove()?;
        paths.remove()
    }
}

/// A run that crashed after deleting the archive but before cleaning up only
/// has its links and side files left to deal with.
pub fn finish_orphaned(journal: &Journal, paths: &Paths) -> Result<()> {
    make_links(&journal.links, &journal.output)?;
    paths.remove()
}

fn make_links(links: &[Link], out: &Path) -> Result<()> {
    for link in links {
        let path = out.join(&link.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let made = if link.hard {
            let target = out.join(&link.target);
            let _ = fs::remove_file(&path);
            // FAT32 and exFAT have no hard links; a copy is the next best thing.
            fs::hard_link(&target, &path).or_else(|_| fs::copy(&target, &path).map(|_| ()))
        } else {
            platform::create_symlink(&path, &link.target)
        };
        made.with_context(|| format!("creating link '{}'", link.path.display()))?;
    }
    Ok(())
}

struct DiskSink<'a> {
    out: &'a Path,
    stop: &'a AtomicBool,
    on_bytes: &'a (dyn Fn(u64) + Sync),
    buf: Vec<u8>,
    links: Vec<Link>,
}

impl Sink for DiskSink<'_> {
    fn item(&mut self, item: &Item, data: &mut dyn Read) -> Result<()> {
        self.write(item, data)
            .with_context(|| format!("extracting '{}'", item.path.display()))
    }
}

impl DiskSink<'_> {
    fn write(&mut self, item: &Item, data: &mut dyn Read) -> Result<()> {
        let dest = self.out.join(&item.path);
        match &item.kind {
            Kind::Dir => {
                fs::create_dir_all(&dest)?;
                return Ok(());
            }
            Kind::Symlink => {
                let mut target = Vec::new();
                data.take(MAX_SYMLINK).read_to_end(&mut target)?;
                let target = PathBuf::from(String::from_utf8(target)?);
                self.links.push(Link {
                    path: item.path.clone(),
                    target,
                    hard: false,
                });
                return Ok(());
            }
            Kind::HardLink(target) => {
                self.links.push(Link {
                    path: item.path.clone(),
                    target: target.clone(),
                    hard: true,
                });
                return Ok(());
            }
            Kind::File => {}
        }

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let part = part_path(&dest);
        let result = self.copy(item, data, &part);
        if result.is_err() {
            let _ = fs::remove_file(&part);
            return result;
        }
        fs::rename(&part, &dest)?;
        Ok(())
    }

    fn copy(&mut self, item: &Item, data: &mut dyn Read, part: &Path) -> Result<()> {
        let mut dst = File::create(part)?;
        let mut written = 0u64;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Err(io::Error::from(io::ErrorKind::Interrupted).into());
            }
            // Decoders check their CRC when the stream ends, so a read error
            // here is how corruption surfaces.
            let n = data.read(&mut self.buf)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > item.size {
                bail!("it is larger than the archive says it is");
            }
            dst.write_all(&self.buf[..n])?;
            (self.on_bytes)(n as u64);
        }
        if written != item.size {
            bail!("it is smaller than the archive says it is");
        }
        if let Some(t) = item.mtime {
            dst.set_modified(t)?;
        }
        platform::apply_mode(&dst, item.mode)?;
        if platform::SYNC_EACH_FILE {
            dst.sync_all()?;
        }
        Ok(())
    }
}

/// Reads at most `left` bytes, then stops the decoder with [`ProbeDone`].
struct ProbeSink {
    left: u64,
    buf: Vec<u8>,
}

#[derive(Debug)]
struct ProbeDone;

impl std::fmt::Display for ProbeDone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("read enough to prove the password")
    }
}

impl std::error::Error for ProbeDone {}

impl Sink for ProbeSink {
    fn item(&mut self, _item: &Item, data: &mut dyn Read) -> Result<()> {
        loop {
            let n = data.read(&mut self.buf)?;
            if n == 0 {
                return Ok(());
            }
            self.left = self.left.saturating_sub(n as u64);
            if self.left == 0 {
                return Err(ProbeDone.into());
            }
        }
    }
}

struct VerifySink<'a> {
    on_bytes: &'a (dyn Fn(u64) + Sync),
    buf: Vec<u8>,
}

impl Sink for VerifySink<'_> {
    fn item(&mut self, item: &Item, data: &mut dyn Read) -> Result<()> {
        let mut read = 0;
        loop {
            let n = data
                .read(&mut self.buf)
                .with_context(|| format!("'{}'", item.path.display()))?;
            if n == 0 {
                break;
            }
            read += n as u64;
            (self.on_bytes)(n as u64);
        }
        if item.kind == Kind::File && read != item.size {
            bail!(
                "'{}' is shorter than the archive says it is",
                item.path.display()
            );
        }
        Ok(())
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
