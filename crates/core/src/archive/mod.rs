//! What the engine needs from an archive format: the archive split into
//! units, byte ranges that can be truncated away once every file in them is
//! safely on disk, and a way to decode one unit.

mod sevenz;
pub mod storage;
mod tar;
mod zip;

use std::io::Read;
use std::path::{Component, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Result, bail};

use storage::{Storage, Tail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    /// The link target is the item's data.
    Symlink,
    /// Hard links point at an earlier entry, which is extracted later than
    /// the link when going from the end, so they are made at the very end.
    HardLink(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub size: u64,
    pub kind: Kind,
    pub mode: Option<u32>,
    pub mtime: Option<SystemTime>,
}

pub struct Unit {
    /// Where the unit's bytes begin. Once it is committed the archive is
    /// truncated to this offset.
    pub start: u64,
    /// The format's own handle for the unit: a ZIP entry index, a 7z block.
    pub key: usize,
    pub items: Vec<Item>,
    /// Decoding it needs the archive's password.
    pub encrypted: bool,
}

/// The archive is encrypted and nothing can be read without its password.
/// Front ends look for this error to ask for one.
#[derive(Debug)]
pub struct PasswordNeeded;

impl std::fmt::Display for PasswordNeeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("this archive is password protected")
    }
}

impl std::error::Error for PasswordNeeded {}

/// The password doesn't open the archive.
#[derive(Debug)]
pub struct WrongPassword;

impl std::fmt::Display for WrongPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("that password doesn't open this archive")
    }
}

impl std::error::Error for WrongPassword {}

impl Unit {
    pub fn size(&self) -> u64 {
        self.items.iter().map(|i| i.size).sum()
    }
}

pub trait Format: Send + Sync {
    /// A worker decodes units on one thread. `threads` is a hint for codecs
    /// that can decode a single stream on several cores.
    fn worker(&self, threads: u32) -> Result<Box<dyn Worker + '_>>;
}

pub trait Worker {
    /// Decodes `unit`, handing each item to `sink` in order.
    fn extract(&mut self, unit: &Unit, sink: &mut dyn Sink) -> Result<()>;
}

pub trait Sink {
    fn item(&mut self, item: &Item, data: &mut dyn Read) -> Result<()>;
}

pub struct Opened {
    pub name: &'static str,
    pub format: Box<dyn Format>,
    pub units: Vec<Unit>,
    /// Everything from here to the end is saved before anything is reclaimed.
    pub tail_start: u64,
}

/// Parses the archive. `resume` carries the format recorded by an earlier
/// run and the tail it saved; a fresh run sniffs the format instead. The
/// password is only needed for encrypted archives; whether it is right is
/// found out when a unit is decoded (see `Job::check_password`).
pub fn open(
    storage: &Arc<Storage>,
    resume: Option<(&str, Arc<Tail>)>,
    password: Option<&str>,
) -> Result<Opened> {
    let (name, tail) = match resume {
        Some((name, tail)) => (name.to_string(), Some(tail)),
        None => (sniff(storage)?.to_string(), None),
    };
    match name.as_str() {
        "zip" => zip::open(storage, tail, password),
        "7z" => sevenz::open(storage, tail, password),
        "tar" => tar::open(storage, tail),
        other => bail!("unknown archive format '{other}' in the journal"),
    }
}

fn sniff(storage: &Storage) -> Result<&'static str> {
    let mut head = [0u8; 512];
    let mut r = storage.reader(None);
    let n = read_up_to(&mut r, &mut head)?;
    let head = &head[..n];
    let starts = |magic: &[u8]| head.starts_with(magic);
    if starts(b"7z\xBC\xAF\x27\x1C") {
        return Ok("7z");
    }
    if head.len() >= 263 && &head[257..262] == b"ustar" {
        return Ok("tar");
    }
    if starts(b"\x1F\x8B")
        || starts(b"\x28\xB5\x2F\xFD")
        || starts(b"\xFD7zXZ\x00")
        || starts(b"BZh")
    {
        bail!(
            "this is a compressed stream (.gz, .zst, .xz or .bz2). In-place extraction of those \
             needs a different technique that HatchZip doesn't have yet"
        );
    }
    if starts(b"Rar!\x1A\x07") {
        bail!("RAR archives aren't supported yet");
    }
    let ext = storage.primary().to_string_lossy().to_ascii_lowercase();
    if ext.ends_with(".tar") || ext.contains(".tar.") {
        return Ok("tar");
    }
    // Anything else gets a ZIP parse: self-extracting archives put an
    // executable in front, so the signature isn't at the start.
    Ok("zip")
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// Turns an archive member name into a relative path that can't leave the
/// output folder. Leading slashes are dropped, as tar does; `..`, drive
/// letters and NUL bytes are refused.
pub fn safe_path(name: &str) -> Option<PathBuf> {
    if name.contains('\0') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            p if p.contains(':') => return None,
            p => out.push(p),
        }
    }
    let clean = out.components().all(|c| matches!(c, Component::Normal(_)));
    (clean && !out.as_os_str().is_empty()).then_some(out)
}

/// Unix permission bits as 7-Zip and Info-ZIP store them in the high half of
/// the Windows attributes.
fn unix_mode_from_attributes(attrs: u32) -> Option<u32> {
    (attrs & 0x8000 != 0).then_some(attrs >> 16)
}

const S_IFMT: u32 = 0o170000;
const S_IFLNK: u32 = 0o120000;

fn is_symlink_mode(mode: Option<u32>) -> bool {
    mode.is_some_and(|m| m & S_IFMT == S_IFLNK)
}
