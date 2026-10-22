//! ZIP compresses every entry on its own, so each entry is a unit.

use std::sync::Arc;

use ::zip::read::ZipArchiveMetadata;
use ::zip::{CompressionMethod, ZipArchive};
use anyhow::{Context, Result, anyhow, bail};

use super::storage::{Reader, Storage, Tail};
use super::{Codec, Format, Item, Kind, Opened, Sink, Stream, Unit, Worker};
use crate::fsutil;

struct Zip {
    storage: Arc<Storage>,
    meta: Arc<ZipArchiveMetadata>,
}

pub fn open(storage: &Arc<Storage>, tail: Option<Arc<Tail>>) -> Result<Opened> {
    let resuming = tail.clone();
    let mut zip = ZipArchive::new(storage.reader(tail))
        .context("this is not a ZIP archive HatchZip can read")?;

    let tail_start = match resuming {
        Some(t) => t.start,
        None => {
            if zip.has_overlapping_entries()? {
                bail!(
                    "entries in this archive overlap each other, which in-place extraction \
                     can't handle safely"
                );
            }
            let mut end = 0;
            for i in 0..zip.len() {
                let f = zip.by_index_raw(i)?;
                end = end.max(f.data_start().unwrap_or(f.header_start()) + f.compressed_size());
            }
            end
        }
    };

    let meta = zip.metadata();
    let mut units = Vec::with_capacity(meta.len());
    for index in 0..meta.len() {
        let e = meta.entry(index)?;
        let name = e.name().map(|n| n.into_owned()).unwrap_or_default();
        if e.encrypted() {
            bail!("'{name}' is password protected, which HatchZip doesn't support yet");
        }
        let path = e.enclosed_name().ok_or_else(|| {
            anyhow!("'{name}' would be written outside the output folder; refusing")
        })?;
        let kind = if e.is_dir() {
            Kind::Dir
        } else if e.is_symlink() {
            Kind::Symlink
        } else {
            Kind::File
        };
        units.push(Unit {
            start: e.header_start(),
            key: index,
            items: vec![Item {
                path,
                size: e.size(),
                kind,
                mode: e.unix_mode(),
                mtime: e.last_modified().and_then(fsutil::zip_time),
            }],
        });
    }

    Ok(Opened {
        name: "zip",
        format: Box::new(Zip {
            storage: storage.clone(),
            meta,
        }),
        units,
        tail_start,
    })
}

impl Format for Zip {
    fn worker(&self, _threads: u32) -> Result<Box<dyn Worker + '_>> {
        // Safety: the metadata was parsed from this same archive, and workers
        // only read entries below the current truncation point.
        let zip = unsafe {
            ZipArchive::unsafe_new_with_metadata(self.storage.reader(None), self.meta.clone())
        };
        Ok(Box::new(ZipWorker { zip }))
    }

    fn stream(&self, unit: &Unit) -> Result<Option<Stream>> {
        let e = self.meta.entry(unit.key)?;
        let codec = match e.compression() {
            CompressionMethod::Stored => Codec::Stored,
            _ => return Ok(None),
        };
        // Safety: as for workers. The local header is read to find where the
        // data begins, and it is never freed before the entry is committed.
        let mut zip = unsafe {
            ZipArchive::unsafe_new_with_metadata(self.storage.reader(None), self.meta.clone())
        };
        let data_start = zip
            .by_index_raw(unit.key)?
            .data_start()
            .ok_or_else(|| anyhow!("can't find where '{}' begins", unit.items[0].path.display()))?;
        Ok(Some(Stream {
            data_start,
            packed: e.compressed_size(),
            codec,
            crc: Some(e.crc32()),
        }))
    }
}

struct ZipWorker {
    zip: ZipArchive<Reader>,
}

impl Worker for ZipWorker {
    fn extract(&mut self, unit: &Unit, sink: &mut dyn Sink) -> Result<()> {
        // The zip crate checks the CRC as the stream ends, so corruption
        // surfaces as a read error inside the sink.
        let mut data = self.zip.by_index(unit.key)?;
        sink.item(&unit.items[0], &mut data)
    }
}
