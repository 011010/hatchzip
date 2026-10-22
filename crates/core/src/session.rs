//! Opening an archive for extraction and working out whether it fits. Shared
//! by the command line and the GUI, so the two never disagree.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::archive::storage;
use crate::engine::Job;
use crate::fsutil;
use crate::journal::{Journal, Paths, Plan};

/// The first volume of the archive and where its journal lives. Paths are
/// made absolute so a resume works from any directory.
pub fn locate(archive: &Path) -> Result<(PathBuf, Paths)> {
    let primary = std::path::absolute(storage::first_volume(archive))
        .with_context(|| format!("resolving {}", archive.display()))?;
    let paths = Paths::for_archive(&primary);
    Ok((primary, paths))
}

pub enum State {
    /// Nothing touched yet. Nothing changes on disk until [`begin`] writes
    /// the journal.
    Fresh(Job, PathBuf),
    Resume(Job, Journal),
    /// The archive is gone but the run died before cleaning up.
    Orphan(Journal),
}

/// `writable` is false for anything that only looks, so it works on
/// read-only media and never takes the lock. `output` is only checked for a
/// fresh extraction, where it must be empty or not exist.
pub fn open(
    primary: &Path,
    paths: &Paths,
    output: Option<PathBuf>,
    writable: bool,
) -> Result<State> {
    if paths.exists() {
        let (journal, tail) = Journal::load(paths)?;
        if let Some(o) = &output
            && std::path::absolute(o)? != journal.output
        {
            bail!(
                "this archive is already half extracted into {}; finish it there first",
                journal.output.display()
            );
        }
        if !journal.parts[0].0.exists() {
            return Ok(State::Orphan(journal));
        }
        let job = Job::resume(&journal, tail, writable)?;
        return Ok(State::Resume(job, journal));
    }
    let job = Job::open(primary, writable)?;
    let output = match output {
        Some(o) => std::path::absolute(o)?,
        None => default_output(primary),
    };
    if writable && output.read_dir().is_ok_and(|mut d| d.next().is_some()) {
        bail!(
            "{} already exists and is not empty; pick another folder",
            output.display()
        );
    }
    Ok(State::Fresh(job, output))
}

/// `game.zip` and `game.7z.001` both extract into `game`, next to them.
pub fn default_output(primary: &Path) -> PathBuf {
    let file_name = primary
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut name = file_name.clone();
    if let Some((stem, digits)) = name.rsplit_once('.')
        && digits.len() >= 3
        && digits.bytes().all(|b| b.is_ascii_digit())
    {
        name = stem.to_string();
    }
    let lower = name.to_ascii_lowercase();
    for ext in [".zip", ".7z", ".tar", ".exe"] {
        if lower.ends_with(ext) && name.len() > ext.len() {
            name.truncate(name.len() - ext.len());
            break;
        }
    }
    if name == file_name {
        name.push_str("-extracted");
    }
    primary.with_file_name(name)
}

/// `game.zip`, or `game.7z.001 (+4 more parts)` for a split archive.
pub fn display_name(job: &Job) -> String {
    let parts = job.storage.parts();
    let first = parts[0]
        .path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    match parts.len() {
        1 => first,
        n => format!("{first} (+{} more parts)", n - 1),
    }
}

/// Whether an extraction fits, in the numbers a person needs to decide.
pub struct SpacePlan {
    /// Usable free space, counting files a crashed run left that will be
    /// deleted before continuing.
    pub free: u64,
    /// Free space HatchZip needs at its busiest moment.
    pub needed: u64,
    pub keep_free: u64,
    pub fits: bool,
    /// Free space once finished, if it fits.
    pub free_after: u64,
    /// What a normal extractor would need free.
    pub normal_needs: u64,
    /// False when the output is on another disk, so freeing the archive
    /// doesn't help it.
    pub reclaims: bool,
    /// The shortfall comes from one solid block that can only be freed once
    /// all of it is out.
    pub solid_block_bound: bool,
}

pub fn plan(job: &Job, out: &Path, resuming: bool, keep_free: u64) -> Result<SpacePlan> {
    let reclaims = fsutil::same_filesystem(job.storage.primary(), out);
    let stale: u64 = job.uncommitted(out).iter().map(|(_, n)| n).sum();
    let free = fsutil::available_space(out)? + stale;
    let (needed, end) = job.space_needed(!resuming, reclaims);
    let fits = free >= needed + keep_free;
    Ok(SpacePlan {
        free,
        needed,
        keep_free,
        fits,
        free_after: (free as i128 - end).max(0) as u64,
        normal_needs: job.remaining_size(),
        reclaims,
        solid_block_bound: !fits
            && job
                .units
                .iter()
                .any(|u| u.items.len() > 1 && u.size() > free),
    })
}

/// Commits to a fresh extraction: saves the archive's tail and writes the
/// journal. From here on the archive may be truncated.
pub fn begin(job: &Job, paths: &Paths, output: PathBuf) -> Result<Journal> {
    let plan = Plan {
        format: job.format_name,
        tail_start: job.tail_start,
        output,
        total_size: job.remaining_size(),
    };
    Journal::create(paths, &job.storage, plan)
}

/// Before continuing a crashed run, deletes what it wrote for batches it
/// never committed.
pub fn clear_uncommitted(job: &Job, out: &Path) -> Result<()> {
    for (p, _) in job.uncommitted(out) {
        fs::remove_file(&p).with_context(|| format!("removing stale {}", p.display()))?;
    }
    Ok(())
}
