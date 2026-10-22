//! What runs on the worker thread: the same steps as `hatchzip extract` and
//! `hatchzip test`, reporting through [`Update`]s instead of printing.
//!
//! Byte counts go through atomics that the UI samples on a timer, so a fast
//! extraction never floods the event loop with one message per write.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use hatchzip_core::engine::{self, Job, Outcome, Progress, Run};
use hatchzip_core::session::{self, SpacePlan, State};

#[derive(Default)]
pub struct Counters {
    pub done: AtomicU64,
    pub total: AtomicU64,
    /// What is left of the archive on disk; zero while only testing.
    pub archive: AtomicU64,
    pub stop: AtomicBool,
}

impl Counters {
    pub fn reset(&self) {
        self.done.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
        self.archive.store(0, Ordering::Relaxed);
        self.stop.store(false, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug)]
pub enum Request {
    Extract {
        archive: PathBuf,
        /// `None` keeps what the archive's journal or name says.
        output: Option<PathBuf>,
        keep_free: u64,
        verify_first: bool,
    },
    Test {
        archive: PathBuf,
    },
}

pub enum Update {
    /// Opening the archive and checking the space.
    Checking,
    NoSpace(SpacePlan),
    /// Reading every file to check it; nothing is written.
    Verifying,
    /// Extraction has started, into this folder.
    Working(PathBuf),
    Paused,
    Extracted(PathBuf),
    Tested(usize),
    Failed(Failure),
}

pub struct Failure {
    pub message: String,
    /// A journal exists, so running again continues instead of starting over.
    pub resumable: bool,
}

/// Runs a request to the end, a pause or an error. Errors are reported as an
/// update, never returned, so the UI hears about every way this ends.
pub fn run(request: &Request, counters: &Arc<Counters>, send: &dyn Fn(Update)) {
    let result = match request {
        Request::Extract {
            archive,
            output,
            keep_free,
            verify_first,
        } => extract(
            archive,
            output.clone(),
            *keep_free,
            *verify_first,
            counters,
            send,
        ),
        Request::Test { archive } => test(archive, counters, send),
    };
    if let Err(e) = result {
        let archive = match request {
            Request::Extract { archive, .. } | Request::Test { archive } => archive,
        };
        let resumable = matches!(request, Request::Extract { .. })
            && session::locate(archive).is_ok_and(|(_, paths)| paths.exists());
        send(Update::Failed(Failure {
            message: sentence(&format!("{e:#}")),
            resumable,
        }));
    }
}

fn extract(
    archive: &Path,
    output: Option<PathBuf>,
    keep_free: u64,
    verify_first: bool,
    counters: &Arc<Counters>,
    send: &dyn Fn(Update),
) -> Result<()> {
    send(Update::Checking);
    let (primary, paths) = session::locate(archive)?;
    let (job, journal) = match session::open(&primary, &paths, output, true, None)? {
        State::Orphan(journal) => {
            engine::finish_orphaned(&journal, &paths)?;
            send(Update::Extracted(journal.output));
            return Ok(());
        }
        State::Resume(job, journal) => {
            let plan = session::plan(&job, &journal.output, true, keep_free)?;
            if !plan.fits {
                send(Update::NoSpace(plan));
                return Ok(());
            }
            session::clear_uncommitted(&job, &journal.output)?;
            (job, journal)
        }
        State::Fresh(job, out) => {
            let plan = session::plan(&job, &out, false, keep_free)?;
            if !plan.fits {
                send(Update::NoSpace(plan));
                return Ok(());
            }
            if verify_first {
                verify(&job, counters, send)
                    .context("the archive is damaged, so nothing was extracted")?;
                if counters.stop.load(Ordering::Relaxed) {
                    send(Update::Paused);
                    return Ok(());
                }
            }
            let journal = session::begin(&job, &paths, out)?;
            (job, journal)
        }
    };

    let output = journal.output.clone();
    counters.total.store(journal.total_size, Ordering::Relaxed);
    counters
        .done
        .store(journal.total_size - job.remaining_size(), Ordering::Relaxed);
    counters
        .archive
        .store(journal.committed_len, Ordering::Relaxed);
    send(Update::Working(output.clone()));

    let (c1, c2) = (counters.clone(), counters.clone());
    let run = Run {
        job,
        journal,
        paths,
        stop: &counters.stop,
        progress: Progress {
            on_bytes: Box::new(move |n| {
                c1.done.fetch_add(n, Ordering::Relaxed);
            }),
            on_commit: Box::new(move |len| c2.archive.store(len, Ordering::Relaxed)),
        },
        keep_free,
    };
    match run.execute()? {
        Outcome::Finished => {
            counters.archive.store(0, Ordering::Relaxed);
            send(Update::Extracted(output));
        }
        Outcome::Paused => send(Update::Paused),
    }
    Ok(())
}

fn test(archive: &Path, counters: &Arc<Counters>, send: &dyn Fn(Update)) -> Result<()> {
    send(Update::Checking);
    let (primary, paths) = session::locate(archive)?;
    if paths.exists() {
        bail!("this archive is partly extracted, so it can't be tested; extract it to finish");
    }
    let job = Job::open(&primary, false, None)?;
    verify(&job, counters, send).context("the archive is damaged")?;
    send(Update::Tested(job.item_count()));
    Ok(())
}

fn verify(job: &Job, counters: &Arc<Counters>, send: &dyn Fn(Update)) -> Result<()> {
    counters
        .total
        .store(job.remaining_size(), Ordering::Relaxed);
    counters.done.store(0, Ordering::Relaxed);
    send(Update::Verifying);
    job.verify(&|n| {
        counters.done.fetch_add(n, Ordering::Relaxed);
    })
}

/// The engine's errors are written to follow "error:" on a terminal; on
/// their own they read better as a sentence.
pub fn sentence(message: &str) -> String {
    let mut chars = message.trim_end().chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return String::new(),
    };
    if out.ends_with(|c: char| c.is_alphanumeric() || c == ')') {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(sentence("not a zip"), "Not a zip.");
        assert_eq!(
            sentence("pick another folder; or not."),
            "Pick another folder; or not."
        );
        assert_eq!(sentence(""), "");
    }

    fn make_zip(dir: &Path) -> PathBuf {
        let path = dir.join("game.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        w.add_directory("data/", opts).unwrap();
        for i in 0..20 {
            w.start_file(format!("data/file{i}.bin"), opts).unwrap();
            w.write_all(&vec![i as u8; 10_000 + i * 100]).unwrap();
        }
        w.finish().unwrap();
        path
    }

    fn collect(request: &Request, counters: &Arc<Counters>) -> Vec<String> {
        let seen = Mutex::new(Vec::new());
        run(request, counters, &|u| {
            seen.lock().unwrap().push(match u {
                Update::Checking => "checking".to_string(),
                Update::NoSpace(_) => "no space".to_string(),
                Update::Verifying => "verifying".to_string(),
                Update::Working(_) => "working".to_string(),
                Update::Paused => "paused".to_string(),
                Update::Extracted(_) => "extracted".to_string(),
                Update::Tested(n) => format!("tested {n}"),
                Update::Failed(f) => format!("failed: {}", f.message),
            })
        });
        seen.into_inner().unwrap()
    }

    #[test]
    fn test_then_extract_a_real_archive() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_zip(dir.path());
        let counters = Arc::new(Counters::default());

        let updates = collect(
            &Request::Test {
                archive: archive.clone(),
            },
            &counters,
        );
        assert_eq!(updates, ["checking", "verifying", "tested 21"]);
        assert_eq!(
            counters.done.load(Ordering::Relaxed),
            counters.total.load(Ordering::Relaxed)
        );

        counters.reset();
        let out = dir.path().join("out");
        let request = Request::Extract {
            archive: archive.clone(),
            output: Some(out.clone()),
            keep_free: 0,
            verify_first: true,
        };
        let updates = collect(&request, &counters);
        assert_eq!(updates, ["checking", "verifying", "working", "extracted"]);
        assert!(!archive.exists());
        assert_eq!(
            std::fs::read(out.join("data/file3.bin")).unwrap(),
            vec![3u8; 10_300]
        );
        assert_eq!(
            counters.done.load(Ordering::Relaxed),
            counters.total.load(Ordering::Relaxed)
        );
    }

    #[test]
    fn a_stop_before_the_first_batch_pauses_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_zip(dir.path());
        let out = dir.path().join("out");
        let counters = Arc::new(Counters::default());
        let request = Request::Extract {
            archive: archive.clone(),
            output: Some(out.clone()),
            keep_free: 0,
            verify_first: false,
        };

        counters.stop.store(true, Ordering::Relaxed);
        assert_eq!(
            collect(&request, &counters),
            ["checking", "working", "paused"]
        );
        // Paused before anything was truncated: the journal is there to
        // continue from.
        assert!(archive.exists());
        assert!(session::locate(&archive).unwrap().1.exists());

        counters.reset();
        assert_eq!(
            collect(&request, &counters),
            ["checking", "working", "extracted"]
        );
        assert!(!archive.exists());
        assert_eq!(std::fs::read_dir(out.join("data")).unwrap().count(), 20);
    }

    #[test]
    fn not_enough_space_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_zip(dir.path());
        let counters = Arc::new(Counters::default());
        let request = Request::Extract {
            archive: archive.clone(),
            output: None,
            keep_free: u64::MAX / 4,
            verify_first: false,
        };
        assert_eq!(collect(&request, &counters), ["checking", "no space"]);
        assert!(!session::locate(&archive).unwrap().1.exists());
    }

    #[test]
    fn failures_say_what_happened() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("notes.zip");
        std::fs::write(&bogus, b"not an archive").unwrap();
        let counters = Arc::new(Counters::default());
        let updates = collect(&Request::Test { archive: bogus }, &counters);
        assert_eq!(updates.len(), 2);
        assert!(updates[1].starts_with("failed: "), "{updates:?}");
    }
}
