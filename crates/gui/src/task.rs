//! The life of one run as the windows show it: checking, working, paused,
//! done or failed. Both windows drive the same state, so they never disagree
//! about what a button does.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;

use hatchzip_core::human;

use crate::text::{Rate, count, plan_view};
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
    rate: Rate,
}

impl Task {
    /// Stopping for good: the archive goes back to normal, holding what
    /// wasn't extracted.
    pub fn restore(name: &str) -> Self {
        let mut task = Self::new(false, name);
        task.restoring = true;
        task.view.detail = "Making it a normal archive again. The files already out stay.".into();
        task.view.heading = task.heading().into();
        task
    }

    pub fn extract(name: &str, output: Option<&Path>) -> Self {
        let mut task = Self::new(false, name);
        if let Some(o) = output {
            task.set_output(o);
        }
        task
    }

    pub fn test(name: &str) -> Self {
        let mut task = Self::new(true, name);
        task.view.detail = "Reading every file to check it. Nothing is written.".into();
        task
    }

    fn new(is_test: bool, name: &str) -> Self {
        let mut task = Self {
            view: TaskView {
                phase: Phase::Checking,
                is_test,
                ..Default::default()
            },
            plan: PlanView::default(),
            output: None,
            name: name.to_string(),
            is_test,
            restoring: false,
            resumable: false,
            rate: Rate::default(),
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
                Phase::Done => format!("Restored {n}"),
                Phase::Failed => format!("Could not restore {n}"),
                _ => format!("Restoring {n}"),
            };
        }
        match (self.is_test, self.view.phase) {
            (true, Phase::Done) => format!("Tested {n}"),
            (true, Phase::Failed) => format!("Could not test {n}"),
            (true, _) => format!("Testing {n}"),
            (false, Phase::Done) => format!("Extracted {n}"),
            (false, Phase::Failed) if self.resumable => format!("Stopped extracting {n}"),
            (false, Phase::Failed) => format!("Could not extract {n}"),
            (false, Phase::NoSpace) => format!("Not enough space for {n}"),
            (false, _) => format!("Extracting {n}"),
        }
    }

    fn set_output(&mut self, output: &Path) {
        self.view.detail = format!("Into {}", output.display()).into();
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
        self.view.message = "Pausing after the files being written now.".into();
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
                    "Needs {} free, {} is free now.",
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
                    v.message = "Checking every file before deleting anything.".into();
                }
                self.rate.clear();
            }
            Update::Working(output) => {
                v.phase = Phase::Working;
                self.rate.clear();
                self.set_output(&output);
            }
            Update::Paused => {
                v.phase = Phase::Paused;
                v.rate = "".into();
                v.message = "Paused. Nothing is lost: resume now or later, and it picks up where it stopped.".into();
            }
            Update::Extracted(output) => {
                v.phase = Phase::Done;
                v.fraction = 1.0;
                v.rate = "".into();
                v.archive_left = "".into();
                v.message = format!("Done. Everything is in {}.", output.display()).into();
                v.can_open_folder = true;
                self.set_output(&output);
            }
            Update::Tested(entries) => {
                v.phase = Phase::Done;
                v.fraction = 1.0;
                v.rate = "".into();
                v.message = match entries {
                    1 => "No damage found. The one entry is intact.".to_string(),
                    n => format!("No damage found. All {} entries are intact.", count(n)),
                }
                .into();
            }
            Update::Restored { entries, output } => {
                v.phase = Phase::Done;
                v.fraction = 0.0;
                v.amounts = "".into();
                v.rate = "".into();
                v.archive_left = "".into();
                v.space = "".into();
                let held = match entries {
                    1 => "the one entry".to_string(),
                    n => format!("the {} entries", count(n)),
                };
                v.message = format!(
                    "Done. {} is a normal archive again, holding {held} not yet extracted. \
                     The files already out stay in {}.",
                    self.name,
                    output.display()
                )
                .into();
            }
            Update::Failed(f) => {
                v.phase = Phase::Failed;
                v.rate = "".into();
                v.message = if f.resumable {
                    format!("{} Nothing is lost: try again to continue.", f.message)
                } else {
                    f.message
                }
                .into();
                v.can_retry = !self.is_test;
                self.resumable = f.resumable;
            }
        }
        self.view.heading = self.heading().into();
        // Half extracted and not running: the archive can be put back.
        self.view.can_give_back = !self.restoring
            && !self.is_test
            && (self.view.phase == Phase::Paused
                || (self.view.phase == Phase::Failed && self.resumable));
    }

    /// Reads the counters the worker bumps and refreshes the numbers.
    pub fn tick(&mut self, now: Instant, counters: &Counters) {
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
        let verb = if v.phase == Phase::Verifying {
            "Checked"
        } else {
            "Extracted"
        };
        v.amounts = format!("{verb} {} of {}", human(done), human(total)).into();
        v.rate = self.rate.describe(total.saturating_sub(done)).into();
        if !self.is_test && v.phase != Phase::Verifying {
            let left = counters.archive.load(Ordering::Relaxed);
            v.archive_left = format!("Archive left on disk: {}", human(left)).into();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hatchzip_core::session::SpacePlan;

    use super::*;
    use crate::work::Failure;

    #[test]
    fn an_extraction_runs_pauses_and_finishes() {
        let mut t = Task::extract("game.zip", None);
        assert_eq!(t.view.phase, Phase::Checking);
        assert_eq!(t.view.heading, "Extracting game.zip");
        assert!(t.running());
        assert!(!t.request_pause(), "nothing to pause while checking");

        t.apply(Update::Planned(SpacePlan {
            free: 7_800_000_000,
            needed: 3_500_000,
            keep_free: 2_000_000_000,
            fits: true,
            free_after: 7_700_000_000,
            normal_needs: 41_500_000,
            reclaims: true,
            solid_block_bound: false,
        }));
        assert_eq!(t.view.space, "Needs 3.5 MB free, 7.8 GB is free now.");
        assert_eq!(t.view.phase, Phase::Checking, "it fits, so it carries on");

        t.apply(Update::Working("/games/game".into()));
        assert_eq!(t.view.detail, "Into /games/game");
        let counters = Counters::default();
        counters.total.store(4_000_000_000, Ordering::Relaxed);
        counters.archive.store(3_000_000_000, Ordering::Relaxed);
        let start = Instant::now();
        t.tick(start, &counters);
        counters.done.store(1_000_000_000, Ordering::Relaxed);
        t.tick(start + Duration::from_secs(10), &counters);
        assert_eq!(t.view.fraction, 0.25);
        assert_eq!(t.view.amounts, "Extracted 1.0 GB of 4.0 GB");
        assert_eq!(t.view.rate, "100.0 MB/s, 30 s left");
        assert_eq!(t.view.archive_left, "Archive left on disk: 3.0 GB");

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
        t.apply(Update::Working("/games/game".into()));
        t.apply(Update::Extracted("/games/game".into()));
        assert_eq!(t.view.phase, Phase::Done);
        assert_eq!(t.view.heading, "Extracted game.zip");
        assert!(t.view.can_open_folder);
        assert_eq!(t.view.message, "Done. Everything is in /games/game.");
        assert_eq!(t.output.as_deref(), Some(Path::new("/games/game")));
    }

    #[test]
    fn giving_the_archive_back_says_what_stays_where() {
        let mut t = Task::restore("game.zip");
        assert_eq!(t.view.heading, "Restoring game.zip");
        t.apply(Update::Restored {
            entries: 12,
            output: "/games/game".into(),
        });
        assert_eq!(t.view.phase, Phase::Done);
        assert_eq!(t.view.heading, "Restored game.zip");
        assert!(!t.view.can_open_folder);
        assert_eq!(
            t.view.message,
            "Done. game.zip is a normal archive again, holding the 12 entries not yet \
             extracted. The files already out stay in /games/game."
        );
    }

    #[test]
    fn no_space_offers_a_recheck() {
        let mut t = Task::extract("game.zip", Some(Path::new("/games")));
        t.apply(Update::NoSpace(SpacePlan {
            free: 1_000_000_000,
            needed: 5_000_000_000,
            keep_free: 2_000_000_000,
            fits: false,
            free_after: 0,
            normal_needs: 9_000_000_000,
            reclaims: true,
            solid_block_bound: true,
        }));
        assert_eq!(t.view.phase, Phase::NoSpace);
        assert!(t.view.can_retry);
        assert_eq!(t.view.message, "Not enough space. Free up 6.0 GB more.");
        assert!(t.plan.note.contains("solid block"));
        assert!(!t.running());
    }

    #[test]
    fn failures_say_whether_progress_is_kept() {
        let mut t = Task::extract("game.zip", None);
        t.apply(Update::Failed(Failure {
            message: "Disk full.".into(),
            resumable: true,
        }));
        assert_eq!(t.view.phase, Phase::Failed);
        assert_eq!(t.view.heading, "Stopped extracting game.zip");
        assert!(t.view.can_retry);
        assert_eq!(
            t.view.message,
            "Disk full. Nothing is lost: try again to continue."
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
        assert_eq!(t.view.amounts, "Checked 50 B of 100 B");
        assert_eq!(t.view.archive_left, "");
        assert!(!t.request_pause());
        t.apply(Update::Tested(1));
        assert_eq!(t.view.message, "No damage found. The one entry is intact.");
    }
}
