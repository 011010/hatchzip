//! Progress record that makes an interrupted extraction resumable.
//!
//! The archive is destroyed from the end as we go, and the end is where a ZIP
//! keeps its central directory. So before the first byte is reclaimed we copy
//! everything after the last entry's data (the "tail") into a side file, and
//! from then on we parse the archive through [`PatchedArchive`], which serves
//! the saved tail at its original offsets.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::fsutil;

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct Journal {
    pub version: u32,
    pub archive_len: u64,
    pub tail_start: u64,
    pub tail_crc: u32,
    pub output: PathBuf,
    /// Entries already committed, counted in extraction order.
    pub done: usize,
    /// What the archive is truncated to once `done` entries are committed.
    pub committed_len: u64,
    /// Symlinks are created last, so nothing we extract can be written
    /// through one.
    pub symlinks: Vec<(PathBuf, PathBuf)>,
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

impl Journal {
    pub fn create(
        paths: &Paths,
        archive: &mut File,
        tail_start: u64,
        output: PathBuf,
    ) -> Result<Self> {
        let archive_len = archive.metadata()?.len();
        let mut tail = Vec::new();
        archive.seek(SeekFrom::Start(tail_start))?;
        archive.read_to_end(&mut tail)?;
        fsutil::write_atomic(&paths.tail, &tail)?;
        let journal = Self {
            version: VERSION,
            archive_len,
            tail_start,
            tail_crc: crc32fast::hash(&tail),
            output,
            done: 0,
            committed_len: archive_len,
            symlinks: Vec::new(),
        };
        journal.save(paths)?;
        Ok(journal)
    }

    pub fn load(paths: &Paths) -> Result<(Self, Vec<u8>)> {
        let bytes = fs::read(&paths.journal)
            .with_context(|| format!("reading {}", paths.journal.display()))?;
        let journal: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is damaged", paths.journal.display()))?;
        if journal.version != VERSION {
            bail!(
                "{} was written by a different HatchZip version",
                paths.journal.display()
            );
        }
        let tail =
            fs::read(&paths.tail).with_context(|| format!("reading {}", paths.tail.display()))?;
        if crc32fast::hash(&tail) != journal.tail_crc
            || journal.tail_start + tail.len() as u64 != journal.archive_len
        {
            bail!("{} does not match its journal", paths.tail.display());
        }
        Ok((journal, tail))
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        fsutil::write_atomic(&paths.journal, &serde_json::to_vec(self)?)
    }
}

/// The archive as it was before we started eating it: the live file for
/// everything below `tail_start`, the saved tail for everything above.
pub struct PatchedArchive {
    file: File,
    tail_start: u64,
    tail: Vec<u8>,
    pos: u64,
}

impl PatchedArchive {
    pub fn new(file: File, tail_start: u64, tail: Vec<u8>) -> Self {
        Self {
            file,
            tail_start,
            tail,
            pos: 0,
        }
    }

    fn len(&self) -> u64 {
        self.tail_start + self.tail.len() as u64
    }
}

impl Read for PatchedArchive {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.tail_start {
            let at = (self.pos - self.tail_start) as usize;
            let src = self.tail.get(at..).unwrap_or(&[]);
            let n = src.len().min(buf.len());
            buf[..n].copy_from_slice(&src[..n]);
            self.pos += n as u64;
            return Ok(n);
        }
        let room = (self.tail_start - self.pos).min(buf.len() as u64) as usize;
        self.file.seek(SeekFrom::Start(self.pos))?;
        let mut n = self.file.read(&mut buf[..room])?;
        if n == 0 {
            // Already truncated away. The zip crate scans backwards over this
            // region while looking for the end record, so it must read as
            // something; zeros never match a signature.
            buf[..room].fill(0);
            n = room;
        }
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for PatchedArchive {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len().checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad seek"))?;
        Ok(self.pos)
    }
}
