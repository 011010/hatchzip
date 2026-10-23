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
pub use crate::archive::targz::{StreamPoint, TarAt};
use crate::fsutil;

/// Version 3 added `resume`. A version 2 journal can't hold one, so it reads
/// as is. Older HatchZip builds refuse version 3, which matters: they would
/// delete a half-extracted file whose archive bytes are already freed.
const VERSION: u32 = 3;
const READABLE: [u32; 2] = [2, 3];

#[derive(Serialize, Deserialize, Clone)]
pub struct Link {
    pub path: PathBuf,
    pub target: PathBuf,
    pub hard: bool,
}

/// Where extraction of a large file can pick up again after a crash. Its
/// output up to `written` is flushed to disk, and the archive bytes before
/// the one holding `bit` may already be freed, so this is the only way back.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Resume {
    /// The unit, by the offset it starts at.
    pub unit: u64,
    /// Bytes of output in `<name>.hatchzip-part`, flushed. Anything the file
    /// holds past this was written after the point and is thrown away.
    pub written: u64,
    /// Where decoding restarts, in bits from the start of the archive. Only
    /// deflate streams ever stop between two bytes.
    pub bit: u64,
    /// CRC-32 of the first `written` bytes of output. Unused for a
    /// compressed tar, whose checksums are per gzip member, in `stream`.
    pub crc: u32,
    /// For a compressed tar, where one stream holds every entry: the
    /// decoder's and the tar parser's state at `bit`. `written` is then how
    /// much of the entry being extracted is in its part file, if the point
    /// lies inside a file. Older builds don't know the format and refuse the
    /// journal before reading this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<Box<StreamPoint>>,
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
    /// The large file being extracted while its archive bytes are freed, if
    /// one is in progress. It always belongs to the highest uncommitted unit.
    #[serde(default)]
    pub resume: Option<Resume>,
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
            resume: None,
            restoring: None,
        };
        journal.save(paths)?;
        Ok(journal)
    }

    pub fn load(paths: &Paths) -> Result<(Self, Arc<Tail>)> {
        let bytes = fs::read(&paths.journal)
            .with_context(|| format!("reading {}", paths.journal.display()))?;
        // The version is checked before the rest is parsed, so a newer layout
        // gets the version message rather than a confusing parse error.
        #[derive(Deserialize)]
        struct Versioned {
            version: u32,
        }
        let version = serde_json::from_slice::<Versioned>(&bytes)
            .with_context(|| format!("{} is damaged", paths.journal.display()))?
            .version;
        if !READABLE.contains(&version) {
            let which = if version > VERSION {
                "a newer"
            } else {
                "an older"
            };
            bail!(
                "{} was written by {which} HatchZip version (journal format {version}); \
                 complete the extraction with that version",
                paths.journal.display()
            );
        }
        let mut journal: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is damaged", paths.journal.display()))?;
        journal.version = VERSION;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, version: u32) -> Paths {
        let paths = Paths::for_archive(&dir.join("game.zip"));
        let tail = b"saved tail";
        fs::write(&paths.tail, tail).unwrap();
        let json = format!(
            r#"{{"version":{version},"format":"zip","parts":[[{},{}]],"tail_start":0,
                "tail_crc":{},"output":"out","total_size":0,"committed_len":0,"links":[]}}"#,
            serde_json::to_string(&dir.join("game.zip")).unwrap(),
            tail.len(),
            crc32fast::hash(tail)
        );
        fs::write(&paths.journal, json).unwrap();
        paths
    }

    /// A run started by the previous release resumes; one started by a
    /// release that knows more is refused before anything is touched.
    #[test]
    fn reads_version_2_and_refuses_newer_journals() {
        let dir = std::env::temp_dir().join(format!("hatchzip-journal-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        let (journal, _) = Journal::load(&write(&dir, 2)).unwrap();
        assert_eq!(journal.resume, None);
        assert_eq!(journal.version, VERSION);

        let err = Journal::load(&write(&dir, VERSION + 1)).err().unwrap();
        assert!(format!("{err:#}").contains("newer HatchZip"), "{err:#}");
        let err = Journal::load(&write(&dir, 1)).err().unwrap();
        assert!(format!("{err:#}").contains("older HatchZip"), "{err:#}");

        fs::remove_dir_all(&dir).unwrap();
    }
}
