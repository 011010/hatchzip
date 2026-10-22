//! In-place extraction, independent of the archive format.
//!
//! Units are processed from the highest offset down. After each batch the
//! archive is truncated to where the batch began, which every filesystem
//! supports, FAT32 and exFAT included, so this path needs no hole punching.
//!
//! A unit too large for the free space would need room for all of its
//! output before any of its archive bytes could go. Where the filesystem can
//! punch holes and the format can resume decoding mid-file, such a unit is
//! streamed instead: every [`stream_step`] of output it is flushed, a resume
//! point is journaled, and only then are the archive bytes before that point
//! punched out. The archive's length stays the same until the unit commits
//! through the ordinary path.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, anyhow, bail};
use rayon::prelude::*;

use crate::archive::resumable::{self, Decoder};
use crate::archive::storage::{Storage, Tail};
use crate::archive::{self, Codec, Format, Item, Kind, Sink, Stream, Unit};
use crate::journal::{Journal, Link, Paths, Resume};
use crate::{fsutil, platform};

/// Larger batches extract in parallel better, smaller ones redo less work
/// after a crash and keep the archive shrinking in fine steps.
const BATCH_MAX_UNITS: usize = 512;
const BATCH_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const BUF_SIZE: usize = 1024 * 1024;
const MAX_SYMLINK: u64 = 64 * 1024;

/// Output written between two resume points of a streamed unit. Each point
/// costs a flush and a journal write; the space check counts one step as the
/// most a streamed unit holds that its archive bytes don't offset.
const STREAM_STEP: u64 = 256 * 1024 * 1024;

fn stream_step() -> u64 {
    test_knob("HATCHZIP_STREAM_STEP").map_or(STREAM_STEP, u64::from)
}

/// Units smaller than this are always extracted whole: streaming one would
/// save less than a couple of steps.
fn stream_threshold() -> u64 {
    2 * stream_step()
}

/// The test suite streams every unit it can, whether it fits or not, so the
/// streaming path runs on small archives.
fn always_stream() -> bool {
    test_knob("HATCHZIP_STREAM_STEP").is_some()
}

fn on_disk(u: &Unit) -> u64 {
    u.items.iter().map(|i| i.size.next_multiple_of(4096)).sum()
}

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
    /// Whether the archive's filesystem can free bytes inside a file, and
    /// how much of a punched range it may keep allocated at each end. Probed
    /// once, on first use.
    punch: OnceLock<Option<u64>>,
    /// The streamed unit a previous run left half done, if any.
    resume: Option<Resume>,
}

impl Job {
    pub fn open(path: &Path, writable: bool) -> Result<Self> {
        let storage = Arc::new(Storage::open(path, writable)?);
        let opened = archive::open(&storage, None)?;
        let top = storage.original_len();
        Self::build(storage, opened, top)
    }

    /// Reopens an archive a previous run already started on. A read-only
    /// open leaves a pending truncation for the next writable one.
    pub fn resume(journal: &Journal, tail: Arc<Tail>, writable: bool) -> Result<Self> {
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
        let opened = archive::open(&storage, Some((&journal.format, tail)))?;
        let mut job = Self::build(storage, opened, journal.committed_len)?;
        if let Some(r) = &journal.resume {
            // Only the highest uncommitted unit is ever streamed, so a point
            // for any other one means the journal and archive disagree.
            if job.units.first().map(|u| u.start) != Some(r.unit) {
                bail!(
                    "the journal of {} records a half-extracted file that isn't the next one \
                     in the archive; refusing to touch it",
                    job.storage.primary().display()
                );
            }
            job.resume = Some(r.clone());
        }
        Ok(job)
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
            punch: OnceLock::new(),
            resume: None,
        })
    }

    /// `Some(granule)` if holes punched in the archive give its space back,
    /// where `granule` is how much may stay allocated at each end of a
    /// punched range. `None` on filesystems that can only truncate, such as
    /// FAT32 and exFAT, which keep today's whole-unit behavior.
    pub fn punch_support(&self) -> Option<u64> {
        *self.punch.get_or_init(|| {
            if test_knob("HATCHZIP_NO_PUNCH").is_some() {
                return None;
            }
            let dir = match self.storage.primary().parent() {
                Some(d) if !d.as_os_str().is_empty() => d,
                _ => Path::new("."),
            };
            fsutil::can_punch(dir).then(|| platform::punch_granule(dir))
        })
    }

    pub fn item_count(&self) -> usize {
        self.units.iter().map(|u| u.items.len()).sum()
    }

    pub fn remaining_size(&self) -> u64 {
        let done = self.resume.as_ref().map_or(0, |r| r.written);
        self.units.iter().map(Unit::size).sum::<u64>() - done
    }

    /// The resume point recorded for `units[i]`, if it is half streamed.
    fn resumed(&self, i: usize) -> Option<&Resume> {
        self.resume
            .as_ref()
            .filter(|r| r.unit == self.units[i].start)
    }

    /// Where the data of `units[i]` lies, if it is one file whose decoding
    /// can be resumed from the middle.
    fn stream_of(&self, i: usize) -> Option<Stream> {
        let unit = &self.units[i];
        match unit.items.as_slice() {
            [item] if item.kind == Kind::File => self.format.stream(unit).ok().flatten(),
            _ => None,
        }
    }

    /// Whether `units[i]` may free its own archive bytes while it is
    /// extracted. A half-streamed unit always continues streaming: the bytes
    /// before its resume point may be gone.
    fn streams(&self, i: usize, reclaims: bool) -> Option<Stream> {
        if self.resumed(i).is_some() {
            return self.stream_of(i);
        }
        if !reclaims || self.units[i].size() < stream_threshold() {
            return None;
        }
        self.punch_support()?;
        self.stream_of(i)
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
            let span = self.span(i) as i128;
            if let Some(s) = self.streams(i, reclaims) {
                let (rise, net) = self.stream_cost(i, &s, span, reclaims);
                peak = peak.max(extra + rise);
                extra += net;
                continue;
            }
            let on_disk = on_disk(u) as i128;
            peak = peak.max(extra + on_disk);
            extra += on_disk;
            if reclaims {
                extra -= span;
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

    /// What streaming `units[i]` costs in free space: `(rise, net)`, the most
    /// it consumes at any moment and what it consumes once committed.
    ///
    /// At any moment the unit holds the output written since the last resume
    /// point (at most a step, plus one read buffer or deflate block), the
    /// archive bytes at either end of the punched range the filesystem keeps
    /// (a granule each), and however far the output so far has outgrown the
    /// input it came from. For stored data that last part is zero. For
    /// deflate it only grows as decoding goes on, since encoders fall back to
    /// stored blocks rather than expand data, so the end state bounds it; a
    /// 1/1024 margin covers block headers.
    ///
    /// Without punching (the filesystem stopped supporting it after a run
    /// began), the unit holds all of its remaining output until it commits.
    fn stream_cost(&self, i: usize, s: &Stream, span: i128, reclaims: bool) -> (i128, i128) {
        let at = self.resumed(i);
        // What a crashed run already wrote is on disk, so the free space
        // measured now has paid for it.
        let done = at.map_or(0, |r| r.written / 4096 * 4096);
        let out_left = (on_disk(&self.units[i]) - done) as i128;
        let read_to = at.map_or(s.data_start, |r| r.bit / 8);
        let data_left = (s.data_start + s.packed).saturating_sub(read_to) as i128;
        // At most this much was punched already. It is free now, so the
        // final truncation gives back that much less; assuming the most
        // keeps the estimate on the safe side.
        let freed = read_to.saturating_sub(s.data_start) as i128;
        let net = if reclaims {
            out_left - (span - freed)
        } else {
            out_left
        };
        let rise = match self.punch_support() {
            Some(granule) if reclaims => {
                let slack = match s.codec {
                    Codec::Stored => 0,
                    Codec::Deflate => out_left / 1024,
                };
                let step = (stream_step() + BUF_SIZE as u64 + 2 * granule) as i128;
                ((out_left - data_left).max(0) + slack + step).min(out_left)
            }
            _ => out_left,
        };
        (rise, net)
    }

    /// Output a crashed run wrote for units it never committed. The output
    /// folder started empty, so anything at those paths is ours. A
    /// half-streamed file is not in the list: its archive bytes may be gone,
    /// so it is the only copy of its first part.
    pub fn uncommitted(&self, out: &Path) -> Vec<(PathBuf, u64)> {
        let mut found = Vec::new();
        for (i, u) in self.units.iter().enumerate() {
            if self.resumed(i).is_some() {
                continue;
            }
            for item in &u.items {
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
        }
        found
    }

    /// Output a crashed stream wrote past its last resume point. It is cut
    /// off before the stream continues, so it counts as free.
    pub fn past_resume_point(&self, out: &Path) -> u64 {
        let Some(r) = self.resume.as_ref() else {
            return 0;
        };
        let Some(item) = self.units.first().and_then(|u| u.items.first()) else {
            return 0;
        };
        let dest = out.join(&item.path);
        [part_path(&dest), dest]
            .iter()
            .find_map(|p| fs::metadata(p).ok())
            .map_or(0, |m| {
                (m.len().next_multiple_of(4096)).saturating_sub(r.written.next_multiple_of(4096))
            })
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

/// Opens a streamed file's part file positioned at `written`, cutting off
/// anything written after that point.
fn open_part(dest: &Path, part: &Path, written: u64) -> Result<File> {
    if written == 0 {
        return OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(part)
            .with_context(|| format!("creating {}", part.display()));
    }
    if !part.exists() && dest.exists() {
        // The previous run finished the file and renamed it, then stopped
        // before committing. The point it journaled still holds.
        fs::rename(dest, part).with_context(|| format!("reopening {}", dest.display()))?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(part)
        .with_context(|| {
            format!(
                "{} is missing, and the archive bytes it was extracted from are already \
                 freed, so this file can't be finished",
                part.display()
            )
        })?;
    if file.metadata()?.len() < written {
        bail!(
            "{} is shorter than when its progress was saved; refusing to continue",
            part.display()
        );
    }
    file.set_len(written)?;
    file.seek(SeekFrom::Start(written))?;
    Ok(file)
}

/// The output just before `written`, which a deflate stream may refer back
/// to. It is on disk, flushed before the point was journaled, so the
/// journal doesn't have to carry it.
fn read_window(file: &mut File, written: u64) -> Result<Vec<u8>> {
    let n = written.min(resumable::WINDOW as u64);
    let mut window = vec![0; n as usize];
    file.seek(SeekFrom::Start(written - n))?;
    file.read_exact(&mut window)?;
    file.seek(SeekFrom::Start(written))?;
    Ok(window)
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

        let reclaims = fsutil::same_filesystem(self.job.storage.primary(), &out);
        let mut next = 0;
        while next < self.job.units.len() {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(Outcome::Paused);
            }
            if let Some(stream) = self.stream_now(next, reclaims, &out)? {
                if !self.stream_unit(next, &stream, &out)? {
                    return Ok(Outcome::Paused);
                }
                self.commit(next, next + 1, Vec::new(), &out)?;
                next += 1;
                continue;
            }
            let end = self.next_batch(next, &out, reclaims)?;
            let batch = &self.job.units[next..end];

            let Some(links) = self.extract_batch(batch, &out)? else {
                return Ok(Outcome::Paused);
            };
            self.commit(next, end, links, &out)?;
            next = end;
        }

        self.finish(&out)?;
        Ok(Outcome::Finished)
    }

    /// Commits `units[start..end]`, all extracted, and truncates the archive
    /// behind them.
    fn commit(&mut self, start: usize, end: usize, links: Vec<Link>, out: &Path) -> Result<()> {
        let batch = &self.job.units[start..end];
        // The journal must never claim a file that a power cut could still
        // take back, since its archive bytes are about to be truncated.
        platform::sync_batch(out).context("flushing extracted files to disk")?;
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
        // A streamed unit's point is spent once the unit itself is claimed;
        // both change in the same atomic write.
        self.journal.resume = None;
        self.job.resume = None;
        self.journal.save(&self.paths)?;
        crash_point("after-journal");

        self.job
            .storage
            .set_len(new_len)
            .context("shrinking the archive")?;
        (self.progress.on_commit)(new_len);
        Ok(())
    }

    /// Whether `units[i]` is extracted by streaming: always if it is half
    /// streamed already, otherwise if it can be and its output wouldn't fit
    /// whole in the space there is right now. When it fits, the ordinary
    /// path is faster, since it extracts many units in parallel.
    fn stream_now(&self, i: usize, reclaims: bool, out: &Path) -> Result<Option<Stream>> {
        if self.job.resumed(i).is_some() {
            return match self.job.stream_of(i) {
                Some(s) => Ok(Some(s)),
                None => bail!(
                    "the journal says '{}' is half extracted, but it can't be resumed",
                    self.job.units[i].items[0].path.display()
                ),
            };
        }
        let Some(stream) = self.job.streams(i, reclaims) else {
            return Ok(None);
        };
        if always_stream() {
            return Ok(Some(stream));
        }
        let avail = fsutil::available_space(out)?.saturating_sub(self.keep_free);
        Ok((on_disk(&self.job.units[i]) > avail).then_some(stream))
    }

    /// Extracts the one file of `units[i]` while freeing the archive bytes
    /// behind it. Returns false if a stop request interrupted it; the last
    /// resume point stays in the journal and the next run continues there.
    ///
    /// The order at every point is what makes a crash harmless: the output
    /// up to the point is flushed, then the point is journaled, and only then
    /// is the archive punched, never past the byte the point restarts in. A
    /// crash before the journal write leaves the previous point in force,
    /// and everything it needs is still in the archive.
    fn stream_unit(&mut self, i: usize, stream: &Stream, out: &Path) -> Result<bool> {
        let unit_start = self.job.units[i].start;
        let item = self.job.units[i].items[0].clone();
        let dest = out.join(&item.path);
        let part = part_path(&dest);
        let at = self.job.resumed(i).cloned().unwrap_or(Resume {
            unit: unit_start,
            written: 0,
            bit: stream.data_start * 8,
            crc: 0,
        });
        let started_here = at.written == 0;
        let result = self
            .stream_file(&item, stream, at, &dest, &part, out)
            .with_context(|| format!("extracting '{}'", item.path.display()));
        if result.is_err() && started_here && self.journal.resume.is_none() {
            // Nothing was punched yet, so the next run starts this file over.
            let _ = fs::remove_file(&part);
        }
        result
    }

    fn stream_file(
        &mut self,
        item: &Item,
        stream: &Stream,
        at: Resume,
        dest: &Path,
        part: &Path,
        out: &Path,
    ) -> Result<bool> {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = open_part(dest, part, at.written)?;
        let window = match stream.codec {
            Codec::Stored => Vec::new(),
            Codec::Deflate => read_window(&mut file, at.written)?,
        };
        let mut decoder = Decoder::new(self.job.storage.reader(None), stream, at.bit, &window)?;
        let mut crc = crc32fast::Hasher::new_with_initial(at.crc);
        let mut written = at.written;
        let mut since_point = 0;
        let step = stream_step();
        let mut punching = self.job.punch_support();
        self.ensure_room(out, item, written, punching)?;

        let mut buf = vec![0; BUF_SIZE];
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let n = decoder.read(&mut buf)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > item.size {
                bail!("it is larger than the archive says it is");
            }
            file.write_all(&buf[..n])?;
            crc.update(&buf[..n]);
            (self.progress.on_bytes)(n as u64);
            since_point += n as u64;

            if since_point >= step
                && written < item.size
                && let Some(bit) = decoder.restart_point()
            {
                let point = Resume {
                    unit: at.unit,
                    written,
                    bit,
                    crc: crc.clone().finalize(),
                };
                punching = self.save_point(&file, part, out, point, stream, punching)?;
                since_point = 0;
                self.ensure_room(out, item, written, punching)?;
            }
        }
        if written != item.size {
            bail!("it is smaller than the archive says it is");
        }
        if let Some(expected) = stream.crc
            && crc.finalize() != expected
        {
            bail!("its data is damaged (CRC mismatch)");
        }
        if let Some(t) = item.mtime {
            file.set_modified(t)?;
        }
        platform::apply_mode(&file, item.mode)?;
        file.sync_all()?;
        drop(file);
        // From here a crash leaves the finished file under its real name
        // while the journal still holds the last point; `open_part` takes it
        // back as the part file.
        fs::rename(part, dest)?;
        Ok(true)
    }

    /// Makes `point` the one a crash resumes from, then frees the archive
    /// bytes it no longer needs. Returns whether to keep punching.
    fn save_point(
        &mut self,
        file: &File,
        part: &Path,
        out: &Path,
        point: Resume,
        stream: &Stream,
        punching: Option<u64>,
    ) -> Result<Option<u64>> {
        file.sync_all()
            .context("flushing the extracted file to disk")?;
        // The part file and the folders above it may be new; a point that
        // outlives its file would resume into nothing.
        let mut dir = part.parent();
        while let Some(d) = dir {
            fsutil::sync_dir(d)?;
            if d == out {
                break;
            }
            dir = d.parent();
        }
        crash_point("stream-after-sync");

        self.journal.resume = Some(point.clone());
        self.journal.save(&self.paths)?;
        self.job.resume = Some(point.clone());
        crash_point("stream-after-journal");

        // Decoding restarts inside the byte holding `point.bit`, so that byte
        // and everything after it must stay.
        if punching.is_some()
            && !self
                .job
                .storage
                .punch(stream.data_start, point.bit / 8)
                .context("freeing extracted archive bytes")?
        {
            // The probe said this works; if the filesystem disagrees after
            // all, the run continues without freeing anything mid-file.
            return Ok(None);
        }
        crash_point("stream-after-punch");
        Ok(punching)
    }

    /// Stops a stream at a resume point, before it writes what doesn't fit.
    fn ensure_room(
        &self,
        out: &Path,
        item: &Item,
        written: u64,
        punching: Option<u64>,
    ) -> Result<()> {
        let avail = fsutil::available_space(out)?.saturating_sub(self.keep_free);
        let left = (item.size - written).next_multiple_of(4096);
        let need = match punching {
            Some(granule) => (stream_step() + BUF_SIZE as u64 + 2 * granule).min(left),
            None => left,
        };
        if need > avail {
            bail!(
                "stopped inside '{}': the next part needs {} and only {} is free (keeping {} \
                 spare).\nNothing is lost. Free up {} and run the same command again to continue.",
                item.path.display(),
                human(need),
                human(avail),
                human(self.keep_free),
                human(need - avail),
            );
        }
        Ok(())
    }

    /// Pick units starting at `start` whose output fits in the space there is
    /// right now. Always takes at least one, or explains why it can't.
    fn next_batch(&self, start: usize, out: &Path, reclaims: bool) -> Result<usize> {
        let avail = fsutil::available_space(out)?.saturating_sub(self.keep_free);
        let units = &self.job.units;
        let need = on_disk;
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
            if end > start && always_stream() && self.job.streams(end, reclaims).is_some() {
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
