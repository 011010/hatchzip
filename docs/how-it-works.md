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

For one large file the same cost is avoided where the filesystem can punch holes: see [Freeing space inside a large file](#freeing-space-inside-a-large-file).

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
| `resume` | While a large file is streamed: its unit, how many bytes of it are flushed, the bit of the archive decoding restarts at, and the CRC-32 so far. See below. |

The journal is at version 3 since `resume` was added. Version 2 journals load as they are; older builds refuse version 3, which matters, because they would delete the half-written file whose archive bytes are already gone.

There is no separate "done" counter: units start at distinct offsets, so `committed_len` alone says which are finished. The journal is always replaced atomically: written to a temporary file, flushed, renamed over the old one, and the directory flushed.

## One batch

Units are processed in batches. Each batch goes through these steps, in this order:

1. **Choose the batch.** Starting from the next uncommitted unit, take units while their extracted size fits in the free space that exists right now, minus `--keep-free`. A batch holds at most 512 units and 2 GB.
2. **Extract in parallel.** Each unit is decoded by a worker thread, and every file in it is written to `<name>.hatchzip-part`. The decoders verify CRCs as each stream ends, and HatchZip checks the written size against the size the archive declares. Each finished part file is renamed to its real name. Links are not created yet, only recorded.
3. **Flush.** The batch is forced to the physical disk: one `syncfs` on Linux, an fsync of every file and directory elsewhere (`F_FULLFSYNC` on macOS).
4. **Commit.** The journal is updated with the new `committed_len` and the batch's links.
5. **Truncate.** The archive is shortened to `committed_len`, the start of the batch's lowest unit. The freed space becomes available for the next batch.

At the end, links are created, then the remaining archive (at most a self-extractor stub or the 7z signature header) is deleted, then the side files.

## Freeing space inside a large file

A unit only frees its archive bytes when it is committed, so one 30 GB file needs 30 GB free before any of its bytes can go. Where the archive's filesystem can punch holes (free bytes in the middle of a file without moving the rest), a unit holding one file of at least 512 MiB, stored or deflated in a ZIP or any regular file in a tar, is **streamed** instead, alone, when its output doesn't fit whole in the space free at that moment:

1. Its file is written to `<name>.hatchzip-part`. Every 256 MiB of output, at the next point where decoding can restart:
2. **Flush** the part file and the folders above it.
3. **Journal the resume point**: output length, the bit of the archive decoding restarts at, and the CRC-32 so far.
4. **Punch** the archive from the start of the file's data up to the byte holding that bit. The archive's length doesn't change, and the freed bytes read as zeros.

When the file is done it is renamed and committed through the ordinary steps 3 to 5, in the same journal write that clears `resume`. The local header or tar header in front of the data is never punched, so the archive still parses on resume.

Stored data can restart at any byte. A deflate stream can restart at any block boundary, the technique of zlib's `examples/zran.c`, using the C API of zlib-rs (`inflatePrime`, `inflateSetDictionary`, `Z_BLOCK`). A boundary may fall inside a byte, so the point is recorded in bits, and the decoder also needs the 32 KiB of output before the point, which back-references reach into. That window is not stored in the journal: it is the last 32 KiB of the part file, flushed before the point was journaled.

Hole punching is detected by punching a 1 MiB scratch file next to the archive and measuring that its allocation dropped, since a call can succeed without freeing anything. Without it (FAT32, exFAT, HFS+) every unit is extracted whole, as before. 7z solid blocks are not streamed yet.

| Interrupted | State on disk | What a resume does |
|---|---|---|
| Before the first point is journaled | A part file, nothing punched | Deletes it and starts the file over |
| After the flush, before the journal write | The part file holds output past the previous point, which is still the one in force | Cuts the part file back to the previous point and decodes from there; everything it needs is still in the archive |
| After the journal write, before or after the punch | The point is durable, the bytes before it may be freed | Cuts the part file to the point and decodes from it, never reading before it |
| After the rename, before the commit | The file has its real name, the journal still holds the last point | Renames it back and continues from that point |

The rule: **the archive is never punched past the last durable resume point**, and a point is only durable once the output it vouches for is flushed. A half-streamed file is never deleted as uncommitted output. `restore` refuses an archive whose journal holds a resume point, since an entry with holes in it can't be put back. The test suite kills the process at each of the three steps of a point at several points, for a stored ZIP entry, a deflated ZIP entry and a tar entry, and around the final commit (`crates/cli/tests/streaming.rs`); a unit test resumes a deflate stream from every block boundary at levels 6 and 9 (`crates/core/src/archive/resumable.rs`).

A deflate block can be as long as its encoder likes. zlib-ng and zlib-rs write level 1 as one block, which has no point to resume from but its start; such a file frees nothing until it ends, and if the space runs out first the run stops and says how much to free. zlib-rs at levels 6 and 9 wrote a block boundary about every 300 KB of output on the unit test's data, and the ZIP entries written by the zip crate and by Python's `zipfile` in the tests above had boundaries throughout; other encoders are not measured.

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

A unit that can be streamed counts at most one step of output (256 MiB) plus a 1 MiB read buffer, plus what the filesystem may keep allocated at each end of a punched range (64 KiB, 1 MiB on NTFS, 16 MiB on ZFS, 128 MiB on btrfs, whose extents are freed whole), plus how far its output outgrows its compressed data, never more than the whole unit. For deflate that last part is bounded by its value at the end, since encoders fall back to stored blocks rather than expand data, with a 1/1024 margin for block headers. Measured on a 950 MiB tmpfs holding a ZIP with one 600 MiB entry and three small ones: stored, the check asked for 269.6 MB where the previous release asked for 629.1 MB and refused, and the lowest free space seen while it ran (sampled every 10 ms) was 98.5 MB of 367.0 MB, a peak of 268.5 MB; deflated (mostly incompressible data, 498.1 MB archive), it asked for 401.3 MB against 629.1 MB, and the peak was 324.4 MB. Both finished with the file byte-identical.

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
