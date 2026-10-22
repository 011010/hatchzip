# How it works

This document describes how HatchZip extracts an archive while giving its space back, and why each step happens in the order it does. Read it before changing anything in `engine.rs`, `journal.rs` or `archive/storage.rs` under `crates/core/src`.

## The space problem

A normal extractor reads the archive and writes the output, then the user deletes the archive. At the moment just before that deletion, the disk holds both: the full archive and the full output. For a 180 GB archive that unpacks to 200 GB, that moment needs 380 GB.

HatchZip deletes the archive gradually, in step with writing the output, so the disk never has to hold much more than the output alone.

## Why from the end

Files can be shortened from the end on every operating system and filesystem (`ftruncate`, `SetEndOfFile`). Removing bytes from the start or the middle of a file needs hole punching (`fallocate` on Linux, `FSCTL_SET_ZERO_DATA` on Windows, `F_PUNCHHOLE` on macOS), which FAT32 and exFAT do not support and which behaves differently on each system.

So HatchZip works from the end. Each format describes its archive as a list of **units**: byte ranges that decode on their own, and that can be truncated away once every file in them is safely on disk.

| Format | Unit | Layout |
|---|---|---|
| ZIP | One entry (local header plus data) | Entries in order, then the central directory, the index, at the very end. A self-extracting stub may come first. |
| 7z | One solid block, or one file in a non-solid archive | A 32-byte signature header, the packed blocks in order, then the header (the index) at the end. |
| tar | One entry, including any long-name or pax records before it | Header and data records in order, then zero blocks. There is no index. |

Units are extracted in reverse order of their position in the file. After each batch the archive is truncated to where the lowest unit of the batch begins.

A solid 7z block can only be decoded from its beginning, so it is freed only once all of it is extracted. That makes the largest unit the main cost of in-place extraction: the run needs free space for the output plus roughly the largest unit. 7-Zip's default settings can put a whole archive into one block, which leaves nothing to gain; the space check reports this before anything is touched.

Directories and empty files in 7z have no data at all, only header entries. They form one extra unit that starts where the header does, so it is done first and truncating it frees only the header, which is already saved.

## Split archives

A split archive (`game.7z.001`, `game.7z.002`, ...) as 7-Zip writes it is one archive cut into byte ranges. `Storage` (`crates/core/src/archive/storage.rs`) joins the volumes into one logical file. Truncating it to a length deletes every volume that lies wholly past that length and shortens the one it falls in. Volume boundaries need no special handling: a unit may span two volumes, and the volume past it is deleted once the unit is committed.

Any volume's name finds the whole set, and the journal records every volume with its original length so a resume can check that what is left is an untouched prefix.

## The saved tail

ZIP and 7z keep their index at the end, which is the first thing truncation removes. Before anything is truncated, HatchZip copies everything after the last unit's data (the "tail") into `<archive>.hatchzip-tail` and records its CRC-32 in the journal. For tar the tail is just the closing zero blocks.

From then on the archive is parsed through `storage::Reader`, which serves the live volumes below the tail offset and the saved tail above it, so the format library sees the archive exactly as it was. Regions already truncated away read as zeros. The zip crate scans backwards over them while looking for its end record, and zeros never match a signature; for tar, the zeros read like the end of the archive, so parsing what is left yields exactly the units still to do.

## The journal

`<archive>.hatchzip` is a small JSON file holding:

| Field | Meaning |
|---|---|
| `format` | `zip`, `7z` or `tar`, so a resume never has to guess from a half-eaten file |
| `parts` | Every volume with its original length |
| `tail_start`, `tail_crc` | Where the saved tail belongs, and its checksum |
| `output` | The output folder, so a resume can't be pointed elsewhere |
| `total_size` | Total output size, for progress |
| `committed_len` | The archive length after the latest commit. Every unit starting at or past it is done. |
| `links` | Symlinks and hard links to create at the very end |

There is no separate "done" counter: units start at distinct offsets, so `committed_len` alone says which are finished. The journal is always replaced atomically: written to a temporary file, flushed, renamed over the old one, and the directory flushed.

## One batch

Units are processed in batches. Each batch goes through these steps, in this order:

1. **Choose the batch.** Starting from the next uncommitted unit, take units while their extracted size fits in the free space that exists right now, minus `--keep-free`. A batch holds at most 512 units and 2 GB.
2. **Extract in parallel.** Each unit is decoded by a worker thread, and every file in it is written to `<name>.hatchzip-part`. The decoders verify CRCs as each stream ends, and HatchZip checks the written size against the size the archive declares. Each finished part file is renamed to its real name. Links are not created yet, only recorded.
3. **Flush.** The batch is forced to the physical disk: one `syncfs` on Linux, an fsync of every file and directory elsewhere (`F_FULLFSYNC` on macOS).
4. **Commit.** The journal is updated with the new `committed_len` and the batch's links.
5. **Truncate.** The archive is shortened to `committed_len`, the start of the batch's lowest unit. The freed space becomes available for the next batch.

At the end, links are created, then the remaining archive (at most a self-extractor stub or the 7z signature header) is deleted, then the side files.

## Crash safety

Each step can be interrupted. The order above makes every interruption recoverable:

| Interrupted during | State on disk | What a resume does |
|---|---|---|
| Choosing or extracting | Some part files or finished files exist, the journal doesn't list them, the archive is intact | Deletes those files (the output folder started empty, so they are HatchZip's) and extracts the batch again |
| Flushing | Same as above | Same as above |
| Committing | The atomic rename either happened or not | Either case above or below |
| Truncating | The journal lists the batch, the archive is still longer than `committed_len` | Truncates the archive, continues with the next batch |
| Creating links at the end | All units committed | Recreates every link, replacing any that exist |
| Deleting the archive and side files | The archive is gone, the journal is still there | Recreates the links and removes the side files |

The rule behind the table: **the journal never claims a file before it is durable, and the archive is never truncated past a unit before the journal claims it.**

The test suites kill the process at steps 3 and 5 at many positions for every format, including split archives with volume boundaries inside entries, and check that the resumed run produces identical output (`crates/cli/tests/extract.rs`, `crates/cli/tests/formats.rs`).

## Restore

`hatchzip restore` turns a half-extracted archive back into a normal one, for when an extraction should stop for good. What is left of the archive is the first `committed_len` bytes of the original, and those hold exactly the units not yet extracted, untouched. Only the index is missing, so restore appends a new one after them. Nothing below `committed_len` moves, so it needs only the free space the new index takes. Files already extracted stay in the output folder.

| Format | What restore writes |
|---|---|
| ZIP | A central directory holding the saved records of the entries left, copied as they are (their local header offsets still point at the same bytes), then the end records, with ZIP64 records when the original had them or the counts or offsets need them. A self-extractor stub stays in front, and the offsets keep whatever convention the original used. |
| 7z | A new header for the blocks left, which are the first ones, right after the signature header, and their files. It is encoded by sevenz-rust2's writer, which is handed each block raw and writes nothing for it, since the block already sits where the writer would put it. The signature header is rewritten to point at the new header. |
| tar | Entries for hard links already cut off whose targets are still in the archive, then the two zero blocks that end a tar archive. |

The steps, in order:

1. Undo any earlier restore that was interrupted (below), and finish a truncation a crashed run left pending, so the archive is exactly `committed_len` bytes.
2. Build the new index in memory. Anything restore refuses, it refuses here, before writing.
3. Delete output a crashed batch wrote but never committed, as a resume does, since those entries are still in the archive. Create the links the journal holds for entries already cut off.
4. Save the journal with a `restoring` field holding the bytes step 6 will overwrite.
5. Append the new index at `committed_len` and flush it.
6. Overwrite the bytes below `committed_len` that change (only the 7z signature header) and flush them.
7. Open the result as a fresh archive with the same parsers extraction uses and check that it lists exactly the entries left. If it doesn't, undo and stop.
8. Delete the journal, then the saved tail.

Any run that opens an archive whose journal has `restoring` set, `restore` and `extract` alike, first cuts the archive back to `committed_len`, including a volume the new index made longer than it was, and puts back the bytes the field holds. So an interrupted restore can be run again, or the extraction continued instead. The journal goes before the tail because a journal without its tail can't be loaded; a tail left behind by a crash between the two is never read again. The test suite kills restore after steps 5 and 6 for every format and checks both ways on (`crates/cli/tests/restore.rs`).

## The space check

Before anything is written, HatchZip replays the whole extraction one unit at a time, counting what each step adds (the unit's files, each rounded up to a 4 KiB block) and what it frees (the unit's span in the archive), plus the saved tail written at the start. The highest point of that running total is the free space the run needs. HatchZip refuses to start unless the disk has that much free plus `--keep-free`.

The real run never needs more than the check promised: it uses larger batches only when there is room for them, since each batch is sized against the free space measured at that moment. If something else on the machine fills the disk during the run, HatchZip stops before the next unit, saves its progress, and says how much space to free.

When the output folder is on a different disk from the archive, freeing the archive does not help the output disk, and the check says so.

## Rejected before starting

These are refused while the archive is still untouched:

- Encrypted entries or blocks (not supported yet).
- Paths that resolve outside the output folder (`..`, absolute ZIP paths, drive letters). Leading slashes in tar names are dropped, as tar itself does.
- Two entries with the same path, ignoring case. On a case-insensitive filesystem they would race inside a parallel batch.
- ZIP entries whose bytes overlap other entries. Overlapping archives are a known zip bomb technique, and truncating one entry's span would destroy another's data.
- tar entry types HatchZip can't create safely: device files, FIFOs, sparse files.
- 7z anti-items (deletions recorded by archive updates).
- Compressed tar streams and RAR, which need a different technique.
- A non-empty output folder, so that a resume can safely delete anything it finds for uncommitted units.

## Operating systems

Everything that differs between systems lives in `crates/core/src/platform/`, one file per system:

| | Linux | macOS | Windows |
|---|---|---|---|
| Flushing a batch | One `syncfs` | `F_FULLFSYNC` per file | `FlushFileBuffers` per file |
| Directory flush | `fsync` on the directory | `fsync` on the directory | Not possible; NTFS journals renames |
| Same disk check | Device ID | Device ID | Volume prefix of the canonical path |
| Unix permissions | Restored, without setuid, setgid and sticky | Same | Ignored |
| Symlinks | Created | Created | Written as a small text file holding the target, as Git does without developer mode |
| Hard links | Created, or copied where the filesystem has none (FAT32, exFAT) | Same | Same |
