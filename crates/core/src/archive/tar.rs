//! An uncompressed tar is a run of header plus data records with nothing
//! shared between them, so every entry is a unit. There is no index; the
//! headers are read in one pass. On resume the same pass over what is left
//! stops at the truncation point, where the zeros read like the end of the
//! archive.

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use ::tar::EntryType;
use anyhow::{Context, Result, anyhow, bail};

use super::storage::{Reader, Storage, Tail};
use super::{Codec, Format, Item, Kind, Opened, Sink, Stream, Unit, Worker, safe_path};

struct Entry {
    data_start: u64,
    size: u64,
    /// For symlinks: the target, which tar keeps in the header, not the data.
    target: Option<Vec<u8>>,
}

struct Tar {
    storage: Arc<Storage>,
    entries: Vec<Entry>,
}

pub fn open(storage: &Arc<Storage>, tail: Option<Arc<Tail>>) -> Result<Opened> {
    let resuming = tail.as_ref().map(|t| t.start);
    let mut archive = ::tar::Archive::new(storage.reader(tail));
    let mut units = Vec::new();
    let mut entries = Vec::new();
    let mut sizes = HashMap::new();
    let mut end = 0;

    for entry in archive
        .entries_with_seek()
        .context("reading the tar headers")?
    {
        let e = entry.context("reading the tar headers")?;
        let header = e.header();
        let name = String::from_utf8_lossy(&e.path_bytes()).into_owned();
        // The unit starts where the previous entry ended, so any long-name
        // or pax records in between belong to it.
        let start = end;
        end = e.raw_file_position() + e.size().next_multiple_of(512);

        let kind = match header.entry_type() {
            EntryType::Regular | EntryType::Continuous => Kind::File,
            EntryType::Directory => Kind::Dir,
            EntryType::Symlink => Kind::Symlink,
            EntryType::Link => {
                let target = e
                    .link_name()?
                    .map(|t| t.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Kind::HardLink(safe_path(&target).ok_or_else(|| {
                    anyhow!("hard link '{name}' points outside the output folder; refusing")
                })?)
            }
            EntryType::GNUSparse => {
                bail!("'{name}' is a sparse file, which HatchZip doesn't support yet")
            }
            other => bail!("'{name}' is a {other:?} entry, which HatchZip can't create"),
        };
        let Some(path) = safe_path(&name) else {
            // "./" at the top of many tarballs names the output folder itself.
            if kind == Kind::Dir {
                continue;
            }
            bail!("'{name}' would be written outside the output folder; refusing");
        };
        let target = match kind {
            Kind::Symlink => Some(
                e.link_name_bytes()
                    .map(|b| b.into_owned())
                    .unwrap_or_default(),
            ),
            _ => None,
        };
        let size = match &kind {
            Kind::File => e.size(),
            Kind::Symlink => target.as_ref().map_or(0, |t| t.len() as u64),
            // Linked files are copied if the filesystem can't hard link, so
            // the space check counts them at the target's size.
            Kind::HardLink(t) => sizes.get(t).copied().unwrap_or(0),
            Kind::Dir => 0,
        };
        sizes.insert(path.clone(), size);
        let mtime = header
            .mtime()
            .ok()
            .map(|s| UNIX_EPOCH + Duration::from_secs(s));
        let mode = header.mode().ok();
        units.push(Unit {
            start,
            key: entries.len(),
            items: vec![Item {
                path,
                size,
                kind,
                mode,
                mtime,
            }],
            encrypted: false,
        });
        entries.push(Entry {
            data_start: e.raw_file_position(),
            size: e.size(),
            target,
        });
    }

    Ok(Opened {
        name: "tar",
        format: Box::new(Tar {
            storage: storage.clone(),
            entries,
        }),
        units,
        tail_start: resuming.unwrap_or(end),
    })
}

impl Format for Tar {
    fn worker(&self, _threads: u32) -> Result<Box<dyn Worker + '_>> {
        Ok(Box::new(TarWorker {
            fmt: self,
            reader: self.storage.reader(None),
        }))
    }

    fn stream(&self, unit: &Unit) -> Result<Option<Stream>> {
        let entry = &self.entries[unit.key];
        if unit.items[0].kind != Kind::File {
            return Ok(None);
        }
        Ok(Some(Stream {
            data_start: entry.data_start,
            packed: entry.size,
            codec: Codec::Stored,
            crc: None,
        }))
    }
}

struct TarWorker<'a> {
    fmt: &'a Tar,
    reader: Reader,
}

impl Worker for TarWorker<'_> {
    fn extract(&mut self, unit: &Unit, sink: &mut dyn Sink) -> Result<()> {
        let entry = &self.fmt.entries[unit.key];
        let item = &unit.items[0];
        if let Some(target) = &entry.target {
            return sink.item(item, &mut Cursor::new(target));
        }
        if !matches!(item.kind, Kind::File) {
            return sink.item(item, &mut std::io::empty());
        }
        self.reader.seek(SeekFrom::Start(entry.data_start))?;
        sink.item(item, &mut (&mut self.reader).take(entry.size))
    }
}

/// What ends a restored tar. Hard links whose targets are still in the
/// archive get their entries back, since the originals were cut off with the
/// extracted part and the link can't be made before its target exists. Then
/// the two zero blocks that mark the end.
pub fn closing(hard_links: &[(&Path, &Path)]) -> Result<Vec<u8>> {
    let mut b = ::tar::Builder::new(Vec::new());
    for (path, target) in hard_links {
        let mut h = ::tar::Header::new_gnu();
        h.set_entry_type(EntryType::Link);
        h.set_size(0);
        h.set_mode(0o644);
        b.append_link(&mut h, path, target)
            .with_context(|| format!("writing the tar entry for '{}'", path.display()))?;
    }
    Ok(b.into_inner()?)
}
