//! Progress record that makes an interrupted extraction resumable.
//!
//! ZIP and 7z keep their index at the end of the archive, which is the first
//! thing truncation destroys. So before the first byte is reclaimed the tail
//! of the archive is copied to a side file, and from then on the archive is
//! parsed through a reader that serves the saved tail at its old offsets.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::archive::storage::{Storage, Tail};
use crate::fsutil;

const VERSION: u32 = 2;

#[derive(Serialize, Deserialize, Clone)]
pub struct Link {
    pub path: PathBuf,
    pub target: PathBuf,
    pub hard: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Journal {
    pub version: u32,
    pub format: String,
    /// Every part of the archive with its original length.
    pub parts: Vec<(PathBuf, u64)>,
    pub tail_start: u64,
    pub tail_crc: u32,
    pub output: PathBuf,
    pub total_size: u64,
    /// The archive is truncated to this once the latest batch is committed.
    /// Every unit starting at or after it is done.
    pub committed_len: u64,
    /// Links are made last, so nothing extracted can be written through one
    /// and every hard link's target already exists.
    pub links: Vec<Link>,
    /// Set while `restore` turns the archive back into a normal one: the
    /// bytes it overwrites below `committed_len`, so that an interrupted
    /// restore can be undone by putting them back and cutting off whatever it
    /// appended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restoring: Option<Vec<(u64, Vec<u8>)>>,
}

pub struct Paths {
    pub journal: PathBuf,
    pub tail: PathBuf,
}

impl Paths {
    pub fn for_archive(archive: &Path) -> Self {
        let with = |suffix: &str| {
            let mut s = archive.as_os_str().to_owned();
            s.push(suffix);
            PathBuf::from(s)
        };
        Self {
            journal: with(".hatchzip"),
            tail: with(".hatchzip-tail"),
        }
    }

    pub fn exists(&self) -> bool {
        self.journal.exists()
    }

    pub fn remove(&self) -> Result<()> {
        for p in [&self.tail, &self.journal] {
            match fs::remove_file(p) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => {
                    return Err(e).with_context(|| format!("removing {}", p.display()));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

pub struct Plan {
    pub format: &'static str,
    pub tail_start: u64,
    pub output: PathBuf,
    pub total_size: u64,
}

impl Journal {
    pub fn create(paths: &Paths, storage: &Storage, plan: Plan) -> Result<Self> {
        let mut tail = Vec::new();
        let mut r = storage.reader(None);
        r.seek(SeekFrom::Start(plan.tail_start))?;
        r.read_to_end(&mut tail)?;
        fsutil::write_atomic(&paths.tail, &tail)?;
        let journal = Self {
            version: VERSION,
            format: plan.format.to_string(),
            parts: storage.record(),
            tail_start: plan.tail_start,
            tail_crc: crc32fast::hash(&tail),
            output: plan.output,
            total_size: plan.total_size,
            committed_len: storage.original_len(),
            links: Vec::new(),
            restoring: None,
        };
        journal.save(paths)?;
        Ok(journal)
    }

    pub fn load(paths: &Paths) -> Result<(Self, Arc<Tail>)> {
        let bytes = fs::read(&paths.journal)
            .with_context(|| format!("reading {}", paths.journal.display()))?;
        let journal: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is damaged", paths.journal.display()))?;
        if journal.version != VERSION {
            bail!(
                "{} was written by a different HatchZip version; finish it with that version",
                paths.journal.display()
            );
        }
        let tail =
            fs::read(&paths.tail).with_context(|| format!("reading {}", paths.tail.display()))?;
        let len: u64 = journal.parts.iter().map(|(_, l)| l).sum();
        if crc32fast::hash(&tail) != journal.tail_crc
            || journal.tail_start + tail.len() as u64 != len
        {
            bail!("{} does not match its journal", paths.tail.display());
        }
        let tail = Arc::new(Tail {
            start: journal.tail_start,
            bytes: tail,
        });
        Ok((journal, tail))
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        fsutil::write_atomic(&paths.journal, &serde_json::to_vec(self)?)
    }
}
