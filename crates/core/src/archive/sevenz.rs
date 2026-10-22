//! 7z keeps its header at the end, like ZIP, and stores its packed data as
//! blocks in order. A solid block can only be decoded from its start, so the
//! unit is the whole block, and it is freed once all its files are out.

use std::io::{self, Read};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use sevenz_rust2::{
    Archive, ArchiveEntry, ArchiveWriter, BlockDecoder, Password, SIGNATURE_HEADER_SIZE,
};

use super::storage::{Reader, Storage, Tail};
use super::{Format, Item, Kind, Opened, Sink, Unit, Worker, is_symlink_mode, safe_path};

/// Files with no data (directories, empty files) live only in the header.
/// They get a unit of their own that starts where the header does, so it's
/// done first and truncating it frees nothing that's still needed.
const NO_DATA: usize = usize::MAX;

const AES: &[u8] = &[0x06, 0xF1, 0x07, 0x01];

struct SevenZ {
    storage: Arc<Storage>,
    archive: Archive,
}

pub fn open(storage: &Arc<Storage>, tail: Option<Arc<Tail>>) -> Result<Opened> {
    let resuming = tail.as_ref().map(|t| t.start);
    let mut reader = storage.reader(tail);
    let archive = Archive::read(&mut reader, &Password::empty()).map_err(|e| {
        if matches!(e, sevenz_rust2::Error::PasswordRequired) {
            anyhow!("this 7z archive is password protected, which HatchZip doesn't support yet")
        } else {
            anyhow!("this is not a 7z archive HatchZip can read: {e}")
        }
    })?;

    if archive
        .blocks
        .iter()
        .flat_map(|b| &b.coders)
        .any(|c| c.encoder_method_id() == AES)
    {
        bail!("this 7z archive is password protected, which HatchZip doesn't support yet");
    }
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
    let password = Password::empty();
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
        units.push(Unit {
            start,
            key: block,
            items,
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
        });
    }

    Ok(Opened {
        name: "7z",
        format: Box::new(SevenZ {
            storage: storage.clone(),
            archive,
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
        let password = Password::empty();
        let decoder = BlockDecoder::new(
            self.threads,
            unit.key,
            &self.fmt.archive,
            &password,
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

/// A new header describing the blocks that start below `keep` and their
/// files, to be written at `keep`, and the start header that points at it.
///
/// sevenz-rust2's writer does the encoding. It is handed each kept block raw,
/// the way it copies blocks from one archive into another, but the copy goes
/// nowhere: the blocks already sit exactly where it would put them, right
/// after the start header. Only what it writes at `keep` and the start header
/// are kept, so nothing is read from the blocks and nothing is moved.
pub fn rebuild_header(storage: &Storage, tail: Arc<Tail>, keep: u64) -> Result<(Vec<u8>, Vec<u8>)> {
    let unusual = || {
        anyhow!(
            "this 7z archive is laid out in a way restore can't rebuild yet; \
             run extract again to finish"
        )
    };
    let mut reader = storage.reader(Some(tail));
    let archive = Archive::read(&mut reader, &Password::empty())
        .map_err(|e| anyhow!("reading the saved 7z header: {e}"))?;
    // 7-Zip always starts the packed data right after the start header.
    // Anything else would leave a gap the writer doesn't know about.
    if archive.pack_pos() != 0 {
        return Err(unusual());
    }
    let first_pack = archive.stream_map.block_first_pack_stream_index();
    let offsets = archive.stream_map.pack_stream_offsets();
    let starts: Vec<u64> = (0..archive.blocks.len())
        .map(|b| SIGNATURE_HEADER_SIZE + offsets[first_pack[b]])
        .collect();
    let kept = starts.iter().take_while(|&&s| s < keep).count();
    if starts[kept..].iter().any(|&s| s < keep) {
        return Err(unusual());
    }

    let encode = |e: sevenz_rust2::Error| anyhow!("writing the new 7z header: {e}");
    let mut w = ArchiveWriter::new(Sieve::new(keep)).map_err(encode)?;
    for block in 0..kept {
        w.push_raw_block(&archive, block, &mut Nothing, |e| e.name().to_string())
            .map_err(encode)?;
    }
    // Files with no data that the engine counts as part of a kept block, as
    // in `open`. Ones outside every block were done first and are gone.
    for block in 0..kept {
        let first = archive.stream_map.block_first_file_index[block];
        let count =
            BlockDecoder::new(1, block, &archive, &Password::empty(), &mut reader).entry_count();
        for f in archive.files[first..first + count]
            .iter()
            .filter(|f| !f.has_stream)
        {
            w.push_archive_entry::<&[u8]>(f.clone(), None)
                .map_err(encode)?;
        }
    }
    let sieve = w.finish().context("writing the new 7z header")?;
    if sieve.skipped != keep - SIGNATURE_HEADER_SIZE {
        return Err(unusual());
    }
    let start = sieve.start.ok_or_else(unusual)?;
    Ok((sieve.header, start))
}

/// Stands in for the archive as the source of raw blocks. The writer copies
/// from it into [`Sieve`], which drops those bytes, so what it reads doesn't
/// matter and it never touches the disk.
struct Nothing;

impl Read for Nothing {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
}

impl io::Seek for Nothing {
    fn seek(&mut self, to: io::SeekFrom) -> io::Result<u64> {
        match to {
            io::SeekFrom::Start(p) => Ok(p),
            _ => Err(io::Error::other("only absolute seeks are expected")),
        }
    }
}

/// Where the writer's output goes: block copies below `keep` are counted and
/// dropped, the start header and everything from `keep` on are kept. Any
/// other write means the writer's layout differs from the archive's.
struct Sieve {
    keep: u64,
    pos: u64,
    skipped: u64,
    start: Option<Vec<u8>>,
    header: Vec<u8>,
}

impl Sieve {
    fn new(keep: u64) -> Self {
        Self {
            keep,
            pos: 0,
            skipped: 0,
            start: None,
            header: Vec::new(),
        }
    }
}

impl io::Write for Sieve {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let end = self.pos + buf.len() as u64;
        if self.pos >= self.keep {
            let at = (self.pos - self.keep) as usize;
            if self.header.len() < at + buf.len() {
                self.header.resize(at + buf.len(), 0);
            }
            self.header[at..at + buf.len()].copy_from_slice(buf);
        } else if self.pos == 0 && end == SIGNATURE_HEADER_SIZE {
            self.start = Some(buf.to_vec());
        } else if self.pos >= SIGNATURE_HEADER_SIZE && end <= self.keep {
            self.skipped += buf.len() as u64;
        } else {
            return Err(io::Error::other(format!(
                "unexpected write of {} bytes at {}",
                buf.len(),
                self.pos
            )));
        }
        self.pos = end;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Seek for Sieve {
    fn seek(&mut self, to: io::SeekFrom) -> io::Result<u64> {
        self.pos = match to {
            io::SeekFrom::Start(p) => p,
            io::SeekFrom::Current(d) => self
                .pos
                .checked_add_signed(d)
                .ok_or_else(|| io::Error::other("bad seek"))?,
            io::SeekFrom::End(_) => return Err(io::Error::other("no end to seek from")),
        };
        Ok(self.pos)
    }
}
