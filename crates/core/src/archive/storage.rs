//! The archive's bytes, whether one file or a split set (`.001`, `.002`, ...),
//! and the only code allowed to shrink them.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};

use crate::platform;

#[derive(Clone, Debug)]
pub struct Part {
    pub path: PathBuf,
    /// Where this part begins in the joined archive.
    pub start: u64,
    /// The part's length before extraction started.
    pub len: u64,
}

/// Bytes from the end of the archive saved to a side file, served back at
/// their original offsets once the real ones are truncated away.
pub struct Tail {
    pub start: u64,
    pub bytes: Vec<u8>,
}

pub struct Storage {
    parts: Arc<[Part]>,
    /// The first part stays open and locked for the whole run, so two runs
    /// can't eat the same archive.
    first: File,
}

impl Storage {
    /// Opens `path`, plus its sibling volumes if it is part of a split set.
    pub fn open(path: &Path, writable: bool) -> Result<Self> {
        let paths = volume_set(path)?;
        let mut parts = Vec::with_capacity(paths.len());
        let mut start = 0;
        for p in paths {
            let len = fs::metadata(&p)
                .with_context(|| format!("reading {}", p.display()))?
                .len();
            parts.push(Part {
                path: p,
                start,
                len,
            });
            start += len;
        }
        Self::with_parts(parts, writable)
    }

    /// Reopens a set whose original layout was recorded in the journal.
    pub fn from_parts(recorded: &[(PathBuf, u64)], writable: bool) -> Result<Self> {
        let mut parts = Vec::with_capacity(recorded.len());
        let mut start = 0;
        for (path, len) in recorded {
            parts.push(Part {
                path: path.clone(),
                start,
                len: *len,
            });
            start += len;
        }
        Self::with_parts(parts, writable)
    }

    fn with_parts(parts: Vec<Part>, writable: bool) -> Result<Self> {
        let path = &parts[0].path;
        let first = if writable {
            platform::open_exclusive(path).map_err(|e| match e.kind() {
                io::ErrorKind::WouldBlock => {
                    anyhow!(
                        "{} is in use by another program or HatchZip run",
                        path.display()
                    )
                }
                _ => anyhow!(e).context(format!(
                    "opening {} for reading and writing",
                    path.display()
                )),
            })?
        } else {
            File::open(path).with_context(|| format!("opening {}", path.display()))?
        };
        Ok(Self {
            parts: parts.into(),
            first,
        })
    }

    pub fn primary(&self) -> &Path {
        &self.parts[0].path
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    pub fn record(&self) -> Vec<(PathBuf, u64)> {
        self.parts.iter().map(|p| (p.path.clone(), p.len)).collect()
    }

    /// The archive's original length.
    pub fn original_len(&self) -> u64 {
        self.parts.last().map_or(0, |p| p.start + p.len)
    }

    /// How much of the archive is still on disk, checking that what is left
    /// is an untouched prefix of the original.
    pub fn current_len(&self) -> Result<u64> {
        let mut end = 0;
        for (i, p) in self.parts.iter().enumerate() {
            let actual = match fs::metadata(&p.path) {
                Ok(m) => m.len(),
                Err(e) if e.kind() == io::ErrorKind::NotFound && i > 0 => 0,
                Err(e) => return Err(e).with_context(|| format!("reading {}", p.path.display())),
            };
            if actual > p.len {
                bail!(
                    "{} grew since extraction started; refusing to touch it",
                    p.path.display()
                );
            }
            if actual == 0 && i > 0 {
                break;
            }
            if end != p.start {
                bail!(
                    "{} is missing a piece before {}",
                    self.primary().display(),
                    p.path.display()
                );
            }
            end = p.start + actual;
            if actual < p.len {
                break;
            }
        }
        Ok(end)
    }

    /// Shrinks the archive to its first `len` bytes: whole volumes past that
    /// point are deleted, the one it falls in is truncated.
    pub fn set_len(&self, len: u64) -> Result<()> {
        for (i, p) in self.parts.iter().enumerate().rev() {
            if p.start + p.len <= len {
                break;
            }
            if i == 0 {
                self.first.set_len(len)?;
            } else if p.start >= len {
                match fs::remove_file(&p.path) {
                    Err(e) if e.kind() != io::ErrorKind::NotFound => {
                        return Err(e).with_context(|| format!("deleting {}", p.path.display()));
                    }
                    _ => {}
                }
            } else {
                let f = OpenOptions::new().write(true).open(&p.path)?;
                if f.metadata()?.len() > len - p.start {
                    f.set_len(len - p.start)?;
                }
            }
        }
        Ok(())
    }

    /// Deletes whatever is left of every part.
    pub fn remove(self) -> Result<()> {
        let Self { parts, first } = self;
        drop(first);
        for p in parts.iter() {
            match fs::remove_file(&p.path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => {
                    return Err(e).with_context(|| format!("deleting {}", p.path.display()));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn reader(&self, tail: Option<Arc<Tail>>) -> Reader {
        Reader {
            parts: self.parts.clone(),
            open: None,
            tail,
            len: self.original_len(),
            pos: 0,
        }
    }
}

/// The path of the first volume, if `path` names any volume of a split set.
pub fn first_volume(path: &Path) -> PathBuf {
    match split_suffix(path) {
        Some((base, width, _)) => numbered(&base, width, 1),
        None => path.to_path_buf(),
    }
}

fn volume_set(path: &Path) -> Result<Vec<PathBuf>> {
    let Some((base, width, n)) = split_suffix(path) else {
        return Ok(vec![path.to_path_buf()]);
    };
    if n != 1 {
        bail!(
            "{} is not the first part; open {} instead",
            path.display(),
            numbered(&base, width, 1).display()
        );
    }
    let mut parts = vec![path.to_path_buf()];
    loop {
        let next = numbered(&base, width, parts.len() as u64 + 1);
        if !next.exists() {
            return Ok(parts);
        }
        parts.push(next);
    }
}

/// `game.7z.001` is `("game.7z", 3, 1)`. Only names with a real extension
/// before the number count, so a file that just happens to end in digits
/// isn't mistaken for a volume.
fn split_suffix(path: &Path) -> Option<(PathBuf, usize, u64)> {
    let name = path.file_name()?.to_str()?;
    let (stem, digits) = name.rsplit_once('.')?;
    if digits.len() < 3 || !digits.bytes().all(|b| b.is_ascii_digit()) || !stem.contains('.') {
        return None;
    }
    Some((
        path.with_file_name(stem),
        digits.len(),
        digits.parse().ok()?,
    ))
}

fn numbered(base: &Path, width: usize, n: u64) -> PathBuf {
    let mut s = base.as_os_str().to_owned();
    s.push(format!(".{n:0width$}"));
    PathBuf::from(s)
}

/// Reads the joined archive. Parts or regions that were already truncated
/// away read as zeros: format parsers scan backwards over them looking for
/// end records, and zeros never match a signature.
pub struct Reader {
    parts: Arc<[Part]>,
    open: Option<(usize, File)>,
    tail: Option<Arc<Tail>>,
    len: u64,
    pos: u64,
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let live_end = match &self.tail {
            Some(t) if self.pos >= t.start => {
                let at = (self.pos - t.start) as usize;
                let src = t.bytes.get(at..).unwrap_or(&[]);
                let n = src.len().min(buf.len());
                buf[..n].copy_from_slice(&src[..n]);
                self.pos += n as u64;
                return Ok(n);
            }
            Some(t) => t.start,
            None => self.len,
        };
        let i = self.parts.partition_point(|p| p.start <= self.pos) - 1;
        let part = &self.parts[i];
        let room = (part.start + part.len).min(live_end) - self.pos;
        let room = room.min(buf.len() as u64) as usize;
        if self.open.as_ref().is_none_or(|(j, _)| *j != i) {
            self.open = File::open(&part.path).ok().map(|f| (i, f));
        }
        let n = match &mut self.open {
            Some((_, f)) => {
                f.seek(SeekFrom::Start(self.pos - part.start))?;
                f.read(&mut buf[..room])?
            }
            None => 0,
        };
        let n = if n == 0 {
            buf[..room].fill(0);
            room
        } else {
            n
        };
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Reader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad seek"))?;
        Ok(self.pos)
    }
}
