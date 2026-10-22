//! 7z keeps its header at the end, like ZIP, and stores its packed data as
//! blocks in order. A solid block can only be decoded from its start, so the
//! unit is the whole block, and it is freed once all its files are out.

use std::io::{self, Read};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use sevenz_rust2::{Archive, ArchiveEntry, BlockDecoder, Password, SIGNATURE_HEADER_SIZE};

use super::storage::{Reader, Storage, Tail};
use super::{
    Format, Item, Kind, Opened, PasswordNeeded, Sink, Unit, Worker, WrongPassword, is_symlink_mode,
    safe_path,
};

/// Files with no data (directories, empty files) live only in the header.
/// They get a unit of their own that starts where the header does, so it's
/// done first and truncating it frees nothing that's still needed.
const NO_DATA: usize = usize::MAX;

const AES: &[u8] = &[0x06, 0xF1, 0x07, 0x01];

struct SevenZ {
    storage: Arc<Storage>,
    archive: Archive,
    password: Password,
}

pub fn open(
    storage: &Arc<Storage>,
    tail: Option<Arc<Tail>>,
    password: Option<&str>,
) -> Result<Opened> {
    use sevenz_rust2::Error;

    let resuming = tail.as_ref().map(|t| t.start);
    let mut reader = storage.reader(tail);
    let password = password.map_or_else(Password::empty, Password::new);
    let archive = Archive::read(&mut reader, &password).map_err(|e| match e {
        Error::PasswordRequired => anyhow::Error::new(PasswordNeeded),
        Error::BadSignature(_) | Error::UnsupportedVersion { .. } => {
            anyhow!("this is not a 7z archive HatchZip can read: {e}")
        }
        // An encrypted header that won't parse is almost always the wrong
        // password; damage can't be told apart from it.
        _ if !password.is_empty() => anyhow::Error::new(WrongPassword),
        e => anyhow!("this is not a 7z archive HatchZip can read: {e}"),
    })?;

    if archive.files.iter().any(|f| f.is_anti_item()) {
        bail!(
            "this 7z archive holds update deletions (anti-items), which HatchZip doesn't support"
        );
    }

    let packed = SIGNATURE_HEADER_SIZE + archive.pack_pos();
    let data_end = packed + archive.pack_sizes().iter().sum::<u64>();
    let tail_start = resuming.unwrap_or(data_end);

    let first_pack = archive.stream_map.block_first_pack_stream_index();
    let offsets = archive.stream_map.pack_stream_offsets();
    let mut covered = vec![false; archive.files.len()];
    let mut units = Vec::with_capacity(archive.blocks.len() + 1);
    for block in 0..archive.blocks.len() {
        let start = packed + offsets[first_pack[block]];
        // Exactly the slice the decoder will walk, so items line up with
        // what it hands back.
        let first = archive.stream_map.block_first_file_index[block];
        let count = BlockDecoder::new(1, block, &archive, &password, &mut reader).entry_count();
        let files = archive
            .files
            .get(first..first + count)
            .ok_or_else(|| anyhow!("this 7z archive's header is inconsistent"))?;
        covered[first..first + count].fill(true);
        let items = files.iter().map(item).collect::<Result<_>>()?;
        let encrypted = archive.blocks[block]
            .coders
            .iter()
            .any(|c| c.encoder_method_id() == AES);
        units.push(Unit {
            start,
            key: block,
            items,
            encrypted,
        });
    }
    let empty: Vec<Item> = archive
        .files
        .iter()
        .zip(&covered)
        .filter(|(_, covered)| !**covered)
        .map(|(f, _)| item(f))
        .collect::<Result<_>>()?;
    if !empty.is_empty() {
        units.push(Unit {
            start: tail_start,
            key: NO_DATA,
            items: empty,
            encrypted: false,
        });
    }

    Ok(Opened {
        name: "7z",
        format: Box::new(SevenZ {
            storage: storage.clone(),
            archive,
            password,
        }),
        units,
        tail_start,
    })
}

fn item(f: &ArchiveEntry) -> Result<Item> {
    let path = safe_path(f.name()).ok_or_else(|| {
        anyhow!(
            "'{}' would be written outside the output folder; refusing",
            f.name()
        )
    })?;
    let mode = if f.has_windows_attributes {
        super::unix_mode_from_attributes(f.windows_attributes())
    } else {
        None
    };
    let kind = if f.is_directory() {
        Kind::Dir
    } else if is_symlink_mode(mode) {
        Kind::Symlink
    } else {
        Kind::File
    };
    let mtime = f
        .has_last_modified_date
        .then(|| SystemTime::from(f.last_modified_date()));
    Ok(Item {
        path,
        size: f.size(),
        kind,
        mode,
        mtime,
    })
}

impl Format for SevenZ {
    fn worker(&self, threads: u32) -> Result<Box<dyn Worker + '_>> {
        Ok(Box::new(SevenZWorker {
            fmt: self,
            reader: self.storage.reader(None),
            threads,
        }))
    }
}

struct SevenZWorker<'a> {
    fmt: &'a SevenZ,
    reader: Reader,
    threads: u32,
}

impl Worker for SevenZWorker<'_> {
    fn extract(&mut self, unit: &Unit, sink: &mut dyn Sink) -> Result<()> {
        if unit.key == NO_DATA {
            for item in &unit.items {
                sink.item(item, &mut io::empty())?;
            }
            return Ok(());
        }
        if unit.encrypted && self.fmt.password.is_empty() {
            return Err(PasswordNeeded.into());
        }
        let decoder = BlockDecoder::new(
            self.threads,
            unit.key,
            &self.fmt.archive,
            &self.fmt.password,
            &mut self.reader,
        );
        // The decoder's callback can only return its own error type, so the
        // sink's real error waits here and replaces the generic one.
        let mut failed = None;
        let mut items = unit.items.iter();
        decoder
            .for_each_entries(&mut |_entry, data: &mut dyn Read| {
                let item = items.next().expect("one item per block entry");
                match sink.item(item, data) {
                    Ok(()) => Ok(true),
                    Err(e) => {
                        failed = Some(e);
                        Ok(false)
                    }
                }
            })
            .map_err(|e| anyhow!("{e}"))
            .context("decoding a 7z block")?;
        match failed {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
