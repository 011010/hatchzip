//! ZIP compresses every entry on its own, so each entry is a unit.

use std::sync::Arc;

use ::zip::ZipArchive;
use ::zip::read::ZipArchiveMetadata;
use ::zip::result::ZipError;
use anyhow::{Context, Result, anyhow, bail};

use super::storage::{Reader, Storage, Tail};
use super::{Format, Item, Kind, Opened, PasswordNeeded, Sink, Unit, Worker, WrongPassword};
use crate::fsutil;

struct Zip {
    storage: Arc<Storage>,
    meta: Arc<ZipArchiveMetadata>,
    password: Option<String>,
}

pub fn open(
    storage: &Arc<Storage>,
    tail: Option<Arc<Tail>>,
    password: Option<&str>,
) -> Result<Opened> {
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
            encrypted: e.encrypted(),
        });
    }

    Ok(Opened {
        name: "zip",
        format: Box::new(Zip {
            storage: storage.clone(),
            meta,
            password: password.map(str::to_owned),
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
        Ok(Box::new(ZipWorker {
            zip,
            password: self.password.as_deref(),
        }))
    }
}

struct ZipWorker<'a> {
    zip: ZipArchive<Reader>,
    password: Option<&'a str>,
}

impl Worker for ZipWorker<'_> {
    fn extract(&mut self, unit: &Unit, sink: &mut dyn Sink) -> Result<()> {
        // The zip crate checks the CRC as the stream ends, so corruption
        // surfaces as a read error inside the sink.
        let opened = match (unit.encrypted, self.password) {
            (false, _) => self.zip.by_index(unit.key),
            (true, Some(pw)) => self.zip.by_index_decrypt(unit.key, pw.as_bytes()),
            (true, None) => return Err(PasswordNeeded.into()),
        };
        let mut data = opened.map_err(|e| match e {
            ZipError::InvalidPassword => anyhow::Error::new(WrongPassword),
            e => anyhow::Error::new(e),
        })?;
        sink.item(&unit.items[0], &mut data)
    }
}
