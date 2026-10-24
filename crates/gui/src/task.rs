//! The life of one run as the windows show it: checking, working, paused,
//! done or failed. Both windows drive the same state, so they never disagree
//! about what a button does.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use hatchzip_core::human;

use crate::text::{Rate, count, plan_view, reading_text};
use crate::work::{Counters, Update};
use crate::{Phase, PlanView, TaskView};

pub struct Task {
    pub view: TaskView,
    pub plan: PlanView,
    pub output: Option<PathBuf>,
    name: String,
    is_test: bool,
    restoring: bool,
    /// The last failure left a journal, so the archive is partly out.
    resumable: bool,
    /// The extraction keeps the archive: nothing to count down, nothing to
    /// restore.
    keep: bool,
    rate: Rate,
    /// When the current check began, as the ticks saw it.
    checking_since: Option<Instant>,
}

const CHECKING: &str = "Checking disk space…";

/// Opening reads through the archive; a read this short doesn't need a
/// number.
const READING_AFTER: Duration = Duration::from_millis(500);

impl Task {
    /// Stopping for good: the archive goes back to normal, holding what
    /// wasn't extracted.
    pub fn restore(name: &str) -> Self {
        let mut task = Self::new(false, name);
        task.restoring = true;
        task.view.detail =
            "Converting it back to a standard archive. Extracted files are kept.".into();
        task.view.heading = task.heading().into();
        task
    }

    /// `keep` is what was asked for; a half-extracted archive goes on the
    /// way it started, which the run reports once it is working.
    pub fn extract(name: &str, output: Option<&Path>, keep: bool) -> Self {
        let mut task = Self::new(false, name);
        task.keep = keep;
        if let Some(o) = output {
            task.set_output(o);
        }
        task
    }

    pub fn test(name: &str) -> Self {
        let mut task = Self::new(true, name);
        task.view.detail = "Verifying every entry. No files are written.".into();
        task
    }

    fn new(is_test: bool, name: &str) -> Self {
        let mut task = Self {
            view: TaskView {
                phase: Phase::Checking,
                is_test,
                busy: CHECKING.into(),
                ..Default::default()
            },
            plan: PlanView::default(),
            output: None,
            name: name.to_string(),
            is_test,
            restoring: false,
            resumable: false,
            keep: false,
            rate: Rate::default(),
            checking_since: None,
        };
        task.view.heading = task.heading().into();
        task
    }

    /// The archive's name, as the prompts and headings show it.
    pub fn name(&self) -> &str {
        &self.name
    }

    fn heading(&self) -> String {
        let n = &self.name;
        if self.restoring {
            return match self.view.phase {
                Phase::Done => "Restore complete".to_string(),
                Phase::Failed => "Restore failed".to_string(),
                _ => format!("Restoring {n}"),
            };
        }
        match (self.is_test, self.view.phase) {
            (true, Phase::Done) => "Test complete".to_string(),
            (true, Phase::Failed) => "Test failed".to_string(),
            (true, _) => format!("Testing {n}"),
            (false, Phase::Done) => "Extraction complete".to_string(),
            (false, Phase::Paused) => "Extraction paused".to_string(),
            (false, Phase::Failed) => "Extraction failed".to_string(),
            (false, Phase::NoSpace) => "Insufficient disk space".to_string(),
            (false, _) => format!("Extracting {n}"),
        }
    }

    fn set_output(&mut self, output: &Path) {
        self.view.detail = format!("Destination: {}", output.display()).into();
        self.output = Some(output.to_path_buf());
    }

    /// While this is true a worker thread owns the archive.
    pub fn running(&self) -> bool {
        matches!(
            self.view.phase,
            Phase::Checking | Phase::Verifying | Phase::Working | Phase::Pausing
        )
    }

    /// Only a run that is writing can pause; checking and testing only read,
    /// so closing the window is how to stop those.
    pub fn request_pause(&mut self) -> bool {
        if self.view.phase != Phase::Working {
            return false;
        }
        self.view.phase = Phase::Pausing;
        self.view.message = "Pausing after the current batch…".into();
        true
    }

    pub fn apply(&mut self, update: Update) {
        let v = &mut self.view;
        v.message = "".into();
        v.can_retry = false;
        v.can_open_folder = false;
        match update {
            Update::Checking => {
                v.phase = Phase::Checking;
                v.busy = CHECKING.into();
                self.checking_since = None;
                v.fraction = 0.0;
                v.amounts = "".into();
                v.rate = "".into();
                v.archive_left = "".into();
                v.space = "".into();
            }
            // The runner keeps the password; nothing to show.
            Update::Unlocked(_) => {}
            Update::Planned(plan) => {
                self.plan = plan_view(&plan);
                v.space = format!(
                    "Required: {}. Available: {}.",
                    human(plan.needed),
                    human(plan.free)
                )
                .into();
            }
            Update::NoSpace(plan) => {
                self.plan = plan_view(&plan);
                v.phase = Phase::NoSpace;
                v.message = self.plan.verdict.clone();
                v.can_retry = true;
            }
            Update::Verifying => {
                v.phase = Phase::Verifying;
                if !self.is_test {
                    v.message = "Verifying archive before extracting…".into();
                }
                self.rate.clear();
            }
            Update::Working { output, keep } => {
                v.phase = Phase::Working;
                self.keep = keep;
                self.rate.clear();
                self.set_output(&output);
            }
            Update::Paused => {
                v.phase = Phase::Paused;
                v.rate = "".into();
                v.message = "Progress is saved. The extraction can be resumed now or later.".into();
            }
            Update::Extracted { output, keep } => {
                v.phase = Phase::Done;
                v.fraction = 1.0;
                v.rate = "".into();
                v.archive_left = "".into();
                v.message = if keep {
                    "All entries were extracted. The archive was kept."
                } else {
                    "All entries were extracted. The archive was removed."
                }
                .into();
                v.can_open_folder = true;
                self.set_output(&output);
            }
            Update::Tested(entries) => {
                v.phase = Phase::Done;
                v.fraction = 1.0;
                v.rate = "".into();
                v.message = match entries {
                    1 => "Test passed: 1 entry verified.".to_string(),
                    n => format!("Test passed: {} entries verified.", count(n)),
                }
                .into();
            }
            Update::Restored {
                entries,
                output,
                kept,
            } => {
                v.phase = Phase::Done;
                v.fraction = 0.0;
                v.amounts = "".into();
                v.rate = "".into();
                v.archive_left = "".into();
                v.space = "".into();
                let held = match entries {
                    1 => "the 1 entry".to_string(),
                    n => format!("the {} entries", count(n)),
                };
                v.message = if kept {
                    format!(
                        "Extraction progress discarded. The archive was not modified. \
                         Extracted files remain in {}.",
                        output.display()
                    )
                } else {
                    format!(
                        "Archive restored. It contains {held} not yet extracted. Extracted \
                         files remain in {}.",
                        output.display()
                    )
                }
                .into();
            }
            Update::Failed(f) => {
                v.phase = Phase::Failed;
                v.rate = "".into();
                v.message = if f.resumable {
                    format!(
                        "{} Progress is saved; the extraction can be resumed.",
                        f.message
                    )
                } else {
                    f.message
                }
                .into();
                v.can_retry = !self.is_test;
                self.resumable = f.resumable;
            }
        }
        self.view.heading = self.heading().into();
        // Half extracted and not running: the archive can be put back. A
        // kept archive was never taken apart.
        self.view.can_give_back = !self.restoring
            && !self.is_test
            && !self.keep
            && (self.view.phase == Phase::Paused
                || (self.view.phase == Phase::Failed && self.resumable));
    }

    /// Reads the counters the worker bumps and refreshes the numbers.
    pub fn tick(&mut self, now: Instant, counters: &Counters) {
        if self.view.phase == Phase::Checking {
            let since = *self.checking_since.get_or_insert(now);
            self.view.busy = match counters.reading.fraction() {
                Some(f) if f < 1.0 && now.duration_since(since) >= READING_AFTER => {
                    reading_text(f).into()
                }
                _ => CHECKING.into(),
            };
            return;
        }
        if !matches!(
            self.view.phase,
            Phase::Verifying | Phase::Working | Phase::Pausing
        ) {
            return;
        }
        let done = counters.done.load(Ordering::Relaxed);
        let total = counters.total.load(Ordering::Relaxed).max(1);
        self.rate.add(now, done);
        let v = &mut self.view;
        v.fraction = (done as f64 / total as f64).min(1.0) as f32;
        v.amounts = format!("{} of {}", human(done), human(total)).into();
        v.rate = self.rate.describe(total.saturating_sub(done)).into();
        if !self.is_test && !self.keep && v.phase != Phase::Verifying {
            let left = counters.archive.load(Ordering::Relaxed);
            v.archive_left = format!("Archive remaining: {}", human(left)).into();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hatchzip_core::session::SpacePlan;

    use super::*;
    use crate::work::Failure;

    fn space(free: u64, needed: u64, keep_free: u64, normal: u64) -> SpacePlan {
        SpacePlan {
            free,
            needed,
            keep_free,
            fits: free >= needed + keep_free,
            free_after: free.saturating_sub(needed),
            keep_archive: false,
            normal_needs: normal,
            in_place_needs: needed,
            either: true,
            reclaims: true,
            solid_block_bound: false,
            archive_kept_whole: false,
        }
    }

    fn working(keep: bool) -> Update {
        Update::Working {
            output: "/games/game".into(),
            keep,
        }
    }

    #[test]
    fn an_extraction_runs_pauses_and_finishes() {
        let mut t = Task::extract("game.zip", None, false);
        assert_eq!(t.view.phase, Phase::Checking);
        assert_eq!(t.view.heading, "Extracting game.zip");
        assert!(t.running());
        assert!(!t.request_pause(), "nothing to pause while checking");

        t.apply(Update::Planned(space(
            7_800_000_000,
            3_500_000,
            2_000_000_000,
            41_500_000,
        )));
        assert_eq!(t.view.space, "Required: 3.5 MB. Available: 7.8 GB.");
        assert_eq!(t.view.phase, Phase::Checking, "it fits, so it carries on");

        t.apply(working(false));
        assert_eq!(t.view.detail, "Destination: /games/game");
        let counters = Counters::default();
        counters.total.store(4_000_000_000, Ordering::Relaxed);
        counters.archive.store(3_000_000_000, Ordering::Relaxed);
        let start = Instant::now();
        t.tick(start, &counters);
        counters.done.store(1_000_000_000, Ordering::Relaxed);
        t.tick(start + Duration::from_secs(10), &counters);
        assert_eq!(t.view.fraction, 0.25);
        assert_eq!(t.view.amounts, "1.0 GB of 4.0 GB");
        assert_eq!(t.view.rate, "100.0 MB/s, 30 seconds remaining");
        assert_eq!(t.view.archive_left, "Archive remaining: 3.0 GB");

        assert!(t.request_pause());
        assert_eq!(t.view.phase, Phase::Pausing);
        assert!(
            t.running(),
            "the worker still owns the archive until it stops"
        );
        assert!(!t.request_pause());
        t.apply(Update::Paused);
        assert_eq!(t.view.phase, Phase::Paused);
        assert!(!t.running());
        assert!(t.view.can_give_back);

        t.apply(Update::Checking);
        assert!(!t.view.can_give_back);
        t.apply(working(false));
        t.apply(Update::Extracted {
            output: "/games/game".into(),
            keep: false,
        });
        assert_eq!(t.view.phase, Phase::Done);
        assert_eq!(t.view.heading, "Extraction complete");
        assert!(t.view.can_open_folder);
        assert_eq!(
            t.view.message,
            "All entries were extracted. The archive was removed."
        );
        assert_eq!(t.output.as_deref(), Some(Path::new("/games/game")));
    }

    /// Nothing counts down and nothing needs restoring when the archive is
    /// kept, whatever the run was asked: a resumed run reports its mode.
    #[test]
    fn keeping_the_archive_shows_no_countdown_and_no_restore() {
        let mut t = Task::extract("game.zip", None, false);
        t.apply(working(true));
        let counters = Counters::default();
        counters.total.store(4_000_000_000, Ordering::Relaxed);
        counters.done.store(1_000_000_000, Ordering::Relaxed);
        t.tick(Instant::now(), &counters);
        assert_eq!(t.view.archive_left, "");
        assert!(t.request_pause());
        t.apply(Update::Paused);
        assert!(!t.view.can_give_back);
        t.apply(Update::Failed(Failure {
            message: "Disk full.".into(),
            resumable: true,
        }));
        assert!(!t.view.can_give_back);
        assert!(t.view.can_retry);
        t.apply(Update::Extracted {
            output: "/games/game".into(),
            keep: true,
        });
        assert_eq!(
            t.view.message,
            "All entries were extracted. The archive was kept."
        );
    }

    #[test]
    fn a_long_open_says_how_far_it_has_read() {
        // Keeping the archive opens it the same way.
        let mut t = Task::extract("data.tar.gz", None, true);
        assert_eq!(t.view.busy, "Checking disk space…");
        let counters = Counters::default();
        counters.reading.total.store(1000, Ordering::Relaxed);
        counters.reading.done.store(370, Ordering::Relaxed);
        let start = Instant::now();
        t.tick(start, &counters);
        assert_eq!(t.view.busy, "Checking disk space…", "too soon for a number");
        t.tick(start + Duration::from_secs(1), &counters);
        assert_eq!(t.view.busy, "Reading archive… 37%");
        counters.reading.done.store(999, Ordering::Relaxed);
        t.tick(start + Duration::from_secs(2), &counters);
        assert_eq!(t.view.busy, "Reading archive… 99%");
        // The open is done; the rest of the check reads nothing.
        counters.reading.reset();
        t.tick(start + Duration::from_secs(3), &counters);
        assert_eq!(t.view.busy, "Checking disk space…");

        t.apply(Update::Working {
            output: "/data".into(),
            keep: true,
        });
        t.apply(Update::Checking);
        assert_eq!(t.view.busy, "Checking disk space…");
    }

    #[test]
    fn giving_the_archive_back_says_what_stays_where() {
        let mut t = Task::restore("game.zip");
        assert_eq!(t.view.heading, "Restoring game.zip");
        t.apply(Update::Restored {
            entries: 12,
            output: "/games/game".into(),
            kept: false,
        });
        assert_eq!(t.view.phase, Phase::Done);
        assert_eq!(t.view.heading, "Restore complete");
        assert!(!t.view.can_open_folder);
        assert_eq!(
            t.view.message,
            "Archive restored. It contains the 12 entries not yet extracted. Extracted files \
             remain in /games/game."
        );

        let mut t = Task::restore("game.zip");
        t.apply(Update::Restored {
            entries: 12,
            output: "/games/game".into(),
            kept: true,
        });
        assert_eq!(
            t.view.message,
            "Extraction progress discarded. The archive was not modified. Extracted files \
             remain in /games/game."
        );
    }

    #[test]
    fn no_space_offers_a_recheck() {
        let mut t = Task::extract("game.zip", Some(Path::new("/games")), false);
        let mut plan = space(1_000_000_000, 5_000_000_000, 2_000_000_000, 9_000_000_000);
        plan.solid_block_bound = true;
        t.apply(Update::NoSpace(plan));
        assert_eq!(t.view.phase, Phase::NoSpace);
        assert!(t.view.can_retry);
        assert_eq!(
            t.view.message,
            "Insufficient disk space. 6.0 GB more is required."
        );
        assert!(t.plan.note.contains("solid block"));
        assert!(!t.running());
    }

    #[test]
    fn failures_say_whether_progress_is_kept() {
        let mut t = Task::extract("game.zip", None, false);
        t.apply(Update::Failed(Failure {
            message: "Disk full.".into(),
            resumable: true,
        }));
        assert_eq!(t.view.phase, Phase::Failed);
        assert_eq!(t.view.heading, "Extraction failed");
        assert!(t.view.can_retry);
        assert_eq!(
            t.view.message,
            "Disk full. Progress is saved; the extraction can be resumed."
        );

        let mut t = Task::test("game.zip");
        t.apply(Update::Failed(Failure {
            message: "The archive is damaged".into(),
            resumable: false,
        }));
        assert!(!t.view.can_retry);
        assert_eq!(t.view.message, "The archive is damaged");
    }

    #[test]
    fn a_test_reports_entries_and_no_archive_size() {
        let mut t = Task::test("game.7z");
        t.apply(Update::Verifying);
        let counters = Counters::default();
        counters.total.store(100, Ordering::Relaxed);
        counters.done.store(50, Ordering::Relaxed);
        t.tick(Instant::now(), &counters);
        assert_eq!(t.view.amounts, "50 B of 100 B");
        assert_eq!(t.view.archive_left, "");
        assert!(!t.request_pause());
        t.apply(Update::Tested(1));
        assert_eq!(t.view.message, "Test passed: 1 entry verified.");
    }
}
