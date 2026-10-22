//! Turning numbers into the short phrases the windows show. Kept apart from
//! the UI wiring so it can be tested without a window.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use hatchzip_core::human;
use hatchzip_core::session::SpacePlan;

use crate::PlanView;

/// `3412` as `3,412`.
pub fn count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

/// A time left, rounded the way people say it.
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=4 => "a few seconds".to_string(),
        5..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s.div_ceil(60)),
        _ => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

/// The "keep free" box takes gigabytes, like `2` or `0.5`.
pub fn parse_gb(text: &str) -> Option<u64> {
    let text = text.trim().replace(',', ".");
    if text.is_empty() {
        return Some(0);
    }
    let gb: f64 = text.parse().ok()?;
    // Under a million gigabytes: more is a typo, and would overflow.
    (0.0..1e6).contains(&gb).then_some((gb * 1e9) as u64)
}

pub fn gb_text(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb.fract() == 0.0 {
        format!("{gb:.0}")
    } else {
        format!("{gb}")
    }
}

pub fn plan_view(plan: &SpacePlan) -> PlanView {
    let verdict = if plan.fits {
        "It fits.".to_string()
    } else {
        let short = plan.needed + plan.keep_free - plan.free;
        format!("Not enough space. Free up {} more.", human(short))
    };
    let mut notes = Vec::new();
    if !plan.fits && plan.solid_block_bound {
        notes.push(
            "Most of this is one solid block, which can only be freed once all of it is out. \
             Archives packed with smaller blocks need far less.",
        );
    }
    if !plan.reclaims {
        notes.push("The folder is on another disk, so deleting the archive won't make room there.");
    }
    let used = plan.needed + plan.keep_free;
    PlanView {
        ready: true,
        free: human(plan.free).into(),
        normal: human(plan.normal_needs).into(),
        needed: human(plan.needed).into(),
        keep: human(plan.keep_free).into(),
        after: if plan.fits {
            human(plan.free_after).into()
        } else {
            "-".into()
        },
        fits: plan.fits,
        verdict: verdict.into(),
        note: notes.join(" ").into(),
        meter: if plan.free == 0 {
            1.0
        } else {
            (used as f64 / plan.free as f64).min(1.0) as f32
        },
    }
}

/// Speed over the last few seconds, so one slow batch flush doesn't make
/// the estimate jump around.
pub struct Rate {
    samples: VecDeque<(Instant, u64)>,
    window: Duration,
}

impl Default for Rate {
    fn default() -> Self {
        Self {
            samples: VecDeque::new(),
            window: Duration::from_secs(5),
        }
    }
}

impl Rate {
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn add(&mut self, at: Instant, done: u64) {
        while let Some(&(t, _)) = self.samples.front() {
            if at.duration_since(t) > self.window && self.samples.len() > 1 {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        self.samples.push_back((at, done));
    }

    /// Bytes per second, once there is a second of history.
    pub fn per_second(&self) -> Option<f64> {
        let (&(t0, b0), &(t1, b1)) = (self.samples.front()?, self.samples.back()?);
        let secs = t1.duration_since(t0).as_secs_f64();
        (secs >= 1.0).then(|| b1.saturating_sub(b0) as f64 / secs)
    }

    /// "120.5 MB/s, 3 min left"
    pub fn describe(&self, left: u64) -> String {
        match self.per_second() {
            Some(r) if r >= 1.0 => format!(
                "{}/s, {} left",
                human(r as u64),
                duration(Duration::from_secs_f64(left as f64 / r))
            ),
            Some(_) => "Waiting for the disk".to_string(),
            None => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_group_thousands() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1000), "1,000");
        assert_eq!(count(3_412_001), "3,412,001");
        assert_eq!(plural(1, "file", "files"), "1 file");
        assert_eq!(plural(1200, "file", "files"), "1,200 files");
    }

    #[test]
    fn durations_round_like_people_do() {
        assert_eq!(duration(Duration::from_secs(2)), "a few seconds");
        assert_eq!(duration(Duration::from_secs(42)), "42 s");
        assert_eq!(duration(Duration::from_secs(61)), "2 min");
        assert_eq!(duration(Duration::from_secs(3 * 3600 + 125)), "3 h 2 min");
    }

    #[test]
    fn keep_free_takes_gigabytes() {
        assert_eq!(parse_gb("2"), Some(2_000_000_000));
        assert_eq!(parse_gb(" 0.5 "), Some(500_000_000));
        assert_eq!(parse_gb("1,5"), Some(1_500_000_000));
        assert_eq!(parse_gb(""), Some(0));
        assert_eq!(parse_gb("-1"), None);
        assert_eq!(parse_gb("two"), None);
        assert_eq!(gb_text(2_000_000_000), "2");
        assert_eq!(gb_text(500_000_000), "0.5");
    }

    #[test]
    fn rate_needs_a_second_of_history() {
        let mut rate = Rate::default();
        let t = Instant::now();
        rate.add(t, 0);
        rate.add(t + Duration::from_millis(500), 50_000_000);
        assert_eq!(rate.per_second(), None);
        assert_eq!(rate.describe(1), "");
        rate.add(t + Duration::from_secs(2), 200_000_000);
        assert_eq!(rate.per_second(), Some(100_000_000.0));
        assert_eq!(rate.describe(6_000_000_000), "100.0 MB/s, 1 min left");
    }

    #[test]
    fn rate_forgets_old_samples() {
        let mut rate = Rate::default();
        let t = Instant::now();
        rate.add(t, 0);
        // A burst long ago, then a steady 10 MB/s.
        rate.add(t + Duration::from_secs(1), 1_000_000_000);
        for i in 2..=20u64 {
            rate.add(
                t + Duration::from_secs(i),
                1_000_000_000 + (i - 1) * 10_000_000,
            );
        }
        let r = rate.per_second().unwrap();
        assert!((r - 10_000_000.0).abs() < 1.0, "{r}");
    }

    fn plan(free: u64, needed: u64, keep: u64) -> SpacePlan {
        SpacePlan {
            free,
            needed,
            keep_free: keep,
            fits: free >= needed + keep,
            free_after: free.saturating_sub(needed),
            normal_needs: needed * 2,
            reclaims: true,
            solid_block_bound: false,
        }
    }

    #[test]
    fn plans_read_plainly() {
        let fits = plan_view(&plan(10_000_000_000, 3_000_000_000, 2_000_000_000));
        assert!(fits.fits);
        assert_eq!(fits.verdict, "It fits.");
        assert_eq!(fits.needed, "3.0 GB");
        assert_eq!(fits.note, "");
        assert!((fits.meter - 0.5).abs() < 1e-6);

        let mut tight = plan(4_000_000_000, 3_000_000_000, 2_000_000_000);
        tight.solid_block_bound = true;
        tight.reclaims = false;
        let v = plan_view(&tight);
        assert!(!v.fits);
        assert_eq!(v.verdict, "Not enough space. Free up 1.0 GB more.");
        assert_eq!(v.after, "-");
        assert!(v.note.contains("solid block"));
        assert!(v.note.contains("another disk"));
        assert_eq!(v.meter, 1.0);
    }
}
