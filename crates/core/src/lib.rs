//! The engine behind HatchZip: extracts ZIP, 7z and tar archives, split or
//! not, while truncating the archive behind itself, so the disk only ever has
//! to hold about the output.
//!
//! Front ends start in [`session`]: locate an archive, open it (fresh or
//! half-extracted), get a [`session::SpacePlan`], then hand a [`engine::Run`]
//! the job.

pub mod archive;
pub mod engine;
mod fsutil;
pub mod journal;
mod platform;
pub mod session;

pub use engine::human;
