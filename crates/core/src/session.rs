//! Opening an archive for extraction and working out whether it fits. Shared
//! by the command line and the GUI, so the two never disagree.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::archive::storage::{self, Reads};
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
/// read-only media and never takes the lock. For a fresh extraction the
/// output folder may already hold other files, but nothing in the archive may
/// land on top of one. `password` only matters for encrypted archives; see
/// [`open_encrypted`] for the ask-and-retry loop front ends want. What
/// opening reads is counted in `reads`, which a front end can show from
/// another thread: a compressed tar is read in full here.
pub fn open(
    primary: &Path,
    paths: &Paths,
    output: Option<PathBuf>,
    writable: bool,
    password: Option<&str>,
    reads: &Arc<Reads>,
) -> Result<State> {
    if paths.exists() {
        let (mut journal, tail) = Journal::load(paths)?;
        if let Some(o) = &output
            && std::path::absolute(o)? != journal.output
        {
            bail!(
                "this archive is already partially extracted to {}. Complete or restore that extraction first",
                journal.output.display()
            );
        }
        if journal.restoring.is_some() {
            if !writable {
                bail!(
                    "a restore of this archive was interrupted. Run restore again to complete \
                     it, or run extract to resume the extraction"
                );
            }
            crate::restore::recover(&mut journal, paths)?;
        }
        if !journal.parts[0].0.exists() {
            if journal.keep {
                // Only the person could have removed an archive this run
                // promised to keep, and the rest of it is still to extract.
                bail!(
                    "{} was moved or deleted during an extraction that keeps it. Put it back \
                     to resume, or delete {} to discard the progress",
                    journal.parts[0].0.display(),
                    paths.journal.display()
                );
            }
            return Ok(State::Orphan(journal));
        }
        let job = Job::resume(&journal, tail, writable, password, reads.clone())?;
        return Ok(State::Resume(job, journal));
    }
    let job = Job::open(primary, writable, password, reads.clone())?;
    let output = match output {
        Some(o) => std::path::absolute(o)?,
        None => default_output(primary),
    };
    if writable {
        check_collisions(&job, &output)?;
    }
    Ok(State::Fresh(job, output))
}

/// Opens an archive that may be encrypted. `ask` is called whenever a
/// password is needed or the last one was wrong, and returns `None` to give
/// up. Archives with a password are proven with [`Job::check_password`]
/// before this returns, so nothing is deleted on a wrong guess. Returns the
/// state and the password that worked.
pub fn open_encrypted(
    primary: &Path,
    paths: &Paths,
    output: Option<PathBuf>,
    writable: bool,
    mut password: Option<String>,
    reads: &Arc<Reads>,
    mut ask: impl FnMut(bool) -> Option<String>,
) -> Result<(State, Option<String>)> {
    let mut wrong = false;
    loop {
        let attempt = open(
            primary,
            paths,
            output.clone(),
            writable,
            password.as_deref(),
            reads,
        );
        let failure = match attempt {
            Ok(State::Orphan(j)) => return Ok((State::Orphan(j), password)),
            Ok(state) => {
                let job = match &state {
                    State::Fresh(job, _) | State::Resume(job, _) => job,
                    State::Orphan(_) => unreachable!(),
                };
                if !job.encrypted() {
                    return Ok((state, password));
                }
                match job.check_password() {
                    Ok(()) => return Ok((state, password)),
                    Err(e) => e,
                }
            }
            Err(e) => e,
        };
        if failure.is::<crate::archive::WrongPassword>() {
            wrong = true;
        } else if !failure.is::<crate::archive::PasswordNeeded>() {
            return Err(failure);
        }
        // Drop the open archive before asking, so nothing stays locked while
        // a person types.
        match ask(wrong) {
            Some(p) => password = Some(p),
            None => return Err(failure),
        }
    }
}

/// HatchZip never overwrites a file. That is also what lets a crashed run
/// delete whatever sits at the paths of its uncommitted entries: before the
/// run started, nothing did.
fn check_collisions(job: &Job, out: &Path) -> Result<()> {
    if !out.exists() {
        return Ok(());
    }
    let mut checked = std::collections::HashSet::new();
    for item in job.units.iter().flat_map(|u| &u.items) {
        let dest = out.join(&item.path);
        if let Ok(meta) = fs::symlink_metadata(&dest)
            && !(meta.is_dir() && item.kind == crate::archive::Kind::Dir)
        {
            bail!(
                "{} already exists. HatchZip does not overwrite existing files. \
                 Move the file or choose another destination",
                dest.display()
            );
        }
        // A file where the archive needs a folder would only fail halfway.
        for parent in item
            .path
            .ancestors()
            .skip(1)
            .filter(|p| !p.as_os_str().is_empty())
        {
            if !checked.insert(parent.to_path_buf()) {
                break;
            }
            if let Ok(meta) = fs::symlink_metadata(out.join(parent))
                && !meta.is_dir()
            {
                bail!(
                    "{} is a file, but the archive requires a folder at that location",
                    out.join(parent).display()
                );
            }
        }
    }
    Ok(())
}

/// `game.zip`, `game.7z.001` and `game.tar.gz` all extract into `game`, next
/// to them.
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
    for ext in [".zip", ".7z", ".tar", ".tar.gz", ".tgz", ".exe"] {
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

/// Whether the extraction of a half-extracted archive keeps it, given what
/// the person asked for now. The journal decides: an extraction that keeps
/// the archive goes on keeping it, and one that has already removed part of
/// it can't start keeping it.
pub fn keeps_archive(journal: &Journal, asked: bool) -> Result<bool> {
    if asked && !journal.keep {
        bail!(
            "this archive is partially extracted in place, so part of it is already removed and \
             it cannot be kept. Resume without --keep-archive, or run restore first"
        );
    }
    Ok(journal.keep)
}

/// Whether an extraction fits, in the numbers a person needs to decide.
pub struct SpacePlan {
    /// Usable free space, counting files a crashed run left that will be
    /// deleted before continuing.
    pub free: u64,
    /// Free space the extraction needs at its busiest moment, in the mode
    /// asked for.
    pub needed: u64,
    pub keep_free: u64,
    pub fits: bool,
    /// Free space once finished, if it fits.
    pub free_after: u64,
    /// The plan is for an extraction that keeps the archive.
    pub keep_archive: bool,
    /// What an extraction that keeps the archive needs free: every
    /// extracted file, as for any other extractor, plus the saved index.
    pub normal_needs: u64,
    /// What extracting in place needs free. Equal to `needed` unless the
    /// archive is kept.
    pub in_place_needs: u64,
    /// Nothing is extracted yet, so either mode can still be chosen. A
    /// half-extracted archive continues in the mode it started in.
    pub either: bool,
    /// False when the output is on another disk, so freeing the archive
    /// doesn't help it.
    pub reclaims: bool,
    /// The shortfall comes from one solid block that can only be freed once
    /// all of it is out.
    pub solid_block_bound: bool,
    /// The archive is one compressed stream (a `.tar.gz`) on a filesystem
    /// that can't free space inside a file, such as FAT32 or exFAT, so it
    /// stays whole until the extraction completes: `needed` is room for
    /// every extracted file next to it.
    pub archive_kept_whole: bool,
}

impl SpacePlan {
    /// Whether an extraction that keeps the archive would fit too.
    pub fn normal_fits(&self) -> bool {
        self.free >= self.normal_needs + self.keep_free
    }

    pub fn in_place_fits(&self) -> bool {
        self.free >= self.in_place_needs + self.keep_free
    }
}

/// The space plan for extracting `job` into `out`, keeping the archive or
/// not. For a half-extracted archive, `keep_archive` must be what its
/// journal says; see [`keeps_archive`].
pub fn plan(
    job: &Job,
    out: &Path,
    resuming: bool,
    keep_free: u64,
    keep_archive: bool,
) -> Result<SpacePlan> {
    let reclaims = fsutil::same_filesystem(job.storage.primary(), out);
    let stale: u64 =
        job.uncommitted(out).iter().map(|(_, n)| n).sum::<u64>() + job.past_resume_point(out);
    let free = fsutil::available_space(out)? + stale;
    // Keeping the archive frees nothing, as if the output were on another
    // disk: the replay counts every file and gives nothing back.
    let (normal_needs, normal_end) = job.space_needed(!resuming, false);
    let (in_place_needs, in_place_end) = if keep_archive && resuming {
        (normal_needs, normal_end)
    } else {
        job.space_needed(!resuming, reclaims)
    };
    // The saved index is deleted at the end, when the archive is kept too.
    let tail = (job.storage.original_len() - job.tail_start) as i128;
    let normal_end = normal_end - if reclaims { tail } else { 0 };
    let (needed, end) = if keep_archive {
        (normal_needs, normal_end)
    } else {
        (in_place_needs, in_place_end)
    };
    let fits = free >= needed + keep_free;
    Ok(SpacePlan {
        free,
        needed,
        keep_free,
        fits,
        free_after: (free as i128 - end).max(0) as u64,
        keep_archive,
        normal_needs,
        in_place_needs,
        either: !resuming,
        reclaims,
        solid_block_bound: !fits
            && !keep_archive
            && !job.is_stream()
            && job
                .units
                .iter()
                .any(|u| u.items.len() > 1 && u.size() > free),
        archive_kept_whole: !keep_archive && job.kept_whole(reclaims),
    })
}

/// Commits to a fresh extraction: saves the archive's tail and writes the
/// journal. From here on the archive may be truncated, unless `keep_archive`
/// is set, in which case nothing ever changes it.
pub fn begin(job: &Job, paths: &Paths, output: PathBuf, keep_archive: bool) -> Result<Journal> {
    if keep_archive {
        job.storage.protect();
    }
    let plan = Plan {
        format: job.format_name,
        tail_start: job.tail_start,
        output,
        total_size: job.remaining_size(),
        keep: keep_archive,
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

#[cfg(test)]
mod tests {
    use std::io::Write;

    use ::zip::write::{SimpleFileOptions, ZipWriter};

    use super::*;

    /// Eight stored entries, so extracting in place frees the archive entry
    /// by entry and needs far less than keeping it.
    fn sample_zip(dir: &Path) -> PathBuf {
        let path = dir.join("plan.zip");
        let mut w = ZipWriter::new(fs::File::create(&path).unwrap());
        let opts =
            SimpleFileOptions::default().compression_method(::zip::CompressionMethod::Stored);
        for i in 0..8u8 {
            w.start_file(format!("data/{i}.bin"), opts).unwrap();
            w.write_all(&vec![i; 200_000 + usize::from(i)]).unwrap();
        }
        w.finish().unwrap();
        path
    }

    #[test]
    fn keeping_the_archive_plans_for_every_file() {
        let dir = std::env::temp_dir().join(format!("hatchzip-plan-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let archive = sample_zip(&dir);
        let out = dir.join("out");
        let job = Job::open(&archive, false, None, Default::default()).unwrap();
        let files: u64 = job
            .units
            .iter()
            .flat_map(|u| &u.items)
            .map(|i| i.size.next_multiple_of(4096))
            .sum();
        let tail = job.storage.original_len() - job.tail_start;

        let keep = plan(&job, &out, false, 0, true).unwrap();
        assert!(keep.keep_archive && keep.either);
        // Every file, plus the copy of the index a resume would need.
        assert_eq!(keep.needed, files + tail);
        assert_eq!(keep.normal_needs, keep.needed);
        assert!(!keep.solid_block_bound && !keep.archive_kept_whole);
        // Nothing of the archive comes back; the saved index does.
        assert_eq!(keep.free.saturating_sub(keep.free_after), files);

        let in_place = plan(&job, &out, false, 0, false).unwrap();
        assert!(!in_place.keep_archive);
        assert_eq!(in_place.normal_needs, keep.needed);
        assert_eq!(in_place.in_place_needs, in_place.needed);
        assert_eq!(keep.in_place_needs, in_place.needed);
        assert!(
            in_place.needed < keep.needed / 2,
            "in place {} vs kept {}",
            in_place.needed,
            keep.needed
        );
        drop(job);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Once an extraction that keeps the archive has begun, nothing can
    /// change it, whatever the caller does.
    #[test]
    fn a_kept_archive_refuses_every_change() {
        let dir = std::env::temp_dir().join(format!("hatchzip-protect-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let archive = sample_zip(&dir);
        let before = fs::read(&archive).unwrap();
        let (primary, paths) = locate(&archive).unwrap();
        let reads = Arc::default();
        let State::Fresh(job, out) = open(&primary, &paths, None, true, None, &reads).unwrap()
        else {
            panic!("a new archive opens fresh");
        };
        let journal = begin(&job, &paths, out, true).unwrap();
        assert!(journal.keep);
        let len = job.storage.original_len();
        assert!(job.storage.set_len(len / 2).is_err());
        assert!(job.storage.punch(0, len / 2).is_err());
        assert!(job.storage.write_at(0, b"PK").is_err());
        assert!(job.storage.cut_to(0).is_err());
        drop(job);
        assert_eq!(fs::read(&archive).unwrap(), before);

        // The journal says so, and a resumed job is protected as well.
        let (journal, tail) = Journal::load(&paths).unwrap();
        let job = Job::resume(&journal, tail, true, None, Default::default()).unwrap();
        assert!(job.storage.set_len(0).is_err());
        assert_eq!(job.units.len(), 8, "nothing is committed yet");
        drop(job);
        assert_eq!(fs::read(&archive).unwrap(), before);
        assert!(keeps_archive(&journal, false).unwrap());
        assert!(keeps_archive(&journal, true).unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }
}
