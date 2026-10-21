# How it works

This document describes how HatchZip extracts a ZIP archive while giving its space back, and why each step happens in the order it does. Read it before changing anything in `src/engine.rs` or `src/journal.rs`.

## The space problem

A normal extractor reads the archive and writes the output, then the user deletes the archive. At the moment just before that deletion, the disk holds both: the full archive and the full output. For a 180 GB archive that unpacks to 200 GB, that moment needs 380 GB.

HatchZip deletes the archive gradually, in step with writing the output, so the disk never has to hold much more than the output alone.

## Why from the end

Files can be shortened from the end on every operating system and filesystem (`ftruncate`, `SetEndOfFile`). Removing bytes from the start or the middle of a file needs hole punching (`fallocate` on Linux, `FSCTL_SET_ZERO_DATA` on Windows, `F_PUNCHHOLE` on macOS), which FAT32 and exFAT do not support and which behaves differently on each system.

A ZIP archive compresses every entry independently. Each entry is a local header followed by its compressed data, and the central directory, the archive's index, sits at the very end:

| Offset | Contents |
|---|---|
| 0 | Optional stub, for example a self-extracting `.exe` |
| | Entry 1: local header, compressed data |
| | Entry 2: local header, compressed data |
| | ... |
| | Entry N: local header, compressed data |
| tail | Central directory, end of central directory record, comment |

So the entries can be extracted in reverse order of their position in the file, and after each one the archive can be truncated to where that entry's local header begins. Truncation alone is enough, and it works everywhere.

## The saved tail

The first truncation removes the central directory. Without it the remaining archive can't be parsed, so before anything is truncated HatchZip copies everything after the last entry's data (the "tail") into `<archive>.hatchzip-tail`, and records its CRC-32 in the journal.

From then on the archive is parsed through `PatchedArchive` (`src/journal.rs`), a reader that serves the live file below the tail offset and the saved tail above it. The zip crate sees the archive exactly as it was originally. Regions that were already truncated read as zeros; the zip crate scans backwards over them while looking for the end record, and zeros never match a signature.

## The journal

`<archive>.hatchzip` is a small JSON file holding:

| Field | Meaning |
|---|---|
| `archive_len`, `tail_start`, `tail_crc` | The original archive's identity and where the saved tail belongs |
| `output` | The output folder, so a resume can't be pointed elsewhere |
| `done` | How many entries, in extraction order, are committed |
| `committed_len` | The archive length once those entries are committed |
| `symlinks` | Symlinks to create at the very end |

It is always replaced atomically: written to a temporary file, flushed, renamed over the old one, and the directory flushed.

## One batch

Entries are processed in batches. Each batch goes through these steps, in this order:

1. **Choose the batch.** Starting from the next uncommitted entry, take entries while their extracted size fits in the free space that exists right now, minus `--keep-free`. A batch holds at most 512 entries and 2 GB.
2. **Extract in parallel.** Each entry is decompressed by a worker thread into `<name>.hatchzip-part`. The zip crate verifies the CRC-32 as the stream ends, and HatchZip checks the written size against the size the archive declares. The finished part file is renamed to its real name.
3. **Flush.** The batch is forced to the physical disk: one `syncfs` on Linux, an fsync of every file and directory elsewhere (`F_FULLFSYNC` on macOS).
4. **Commit.** The journal is updated with the new `done` count and `committed_len`.
5. **Truncate.** The archive is truncated to `committed_len`, the start of the batch's lowest entry. The freed space becomes available for the next batch.

## Crash safety

Each step can be interrupted. The order above makes every interruption recoverable:

| Interrupted during | State on disk | What a resume does |
|---|---|---|
| Choosing or extracting | Some part files or finished files exist, the journal doesn't list them, the archive is intact | Deletes those files (the output folder started empty, so they are HatchZip's) and extracts the batch again |
| Flushing | Same as above | Same as above |
| Committing | The atomic rename either happened or not | Either case above or below |
| Truncating | The journal lists the batch, the archive is still longer than `committed_len` | Truncates the archive, continues with the next batch |
| Creating symlinks at the end | All entries committed | Recreates every symlink, replacing any that exist |

The rule behind the table: **the journal never claims a file before it is durable, and the archive is never truncated past an entry before the journal claims it.**

The test suite kills the process at steps 3 and 5 of every batch position in a sample archive and checks that the resumed run produces identical output (`tests/extract.rs`).

## The space check

Before anything is written, HatchZip replays the whole extraction one entry at a time, counting what each step adds (the entry's size, rounded up to a 4 KiB block) and what it frees (the entry's span in the archive), plus the saved tail written at the start. The highest point of that running total is the free space the run needs. HatchZip refuses to start unless the disk has that much free plus `--keep-free`.

This is a lower bound for the real run. The real run uses larger batches only when there is room for them, since each batch is sized against the free space measured at that moment, so it never needs more than the check promised. If something else on the machine fills the disk during the run, HatchZip stops before the next file, saves its progress, and says how much space to free.

When the output folder is on a different disk from the archive, freeing the archive does not help the output disk, and the check says so.

## Rejected before starting

These are refused while the archive is still untouched:

- Encrypted entries (not supported yet).
- Paths that resolve outside the output folder (`../`, absolute paths).
- Two entries with the same path, ignoring case. On a case-insensitive filesystem they would race inside a parallel batch.
- Entries whose bytes overlap other entries. Overlapping archives are a known zip bomb technique, and truncating one entry's span would destroy another's data.
- A non-empty output folder, so that a resume can safely delete anything it finds for uncommitted entries.

## Operating systems

Everything that differs between systems lives in `src/platform/`, one file per system:

| | Linux | macOS | Windows |
|---|---|---|---|
| Flushing a batch | One `syncfs` | `F_FULLFSYNC` per file | `FlushFileBuffers` per file |
| Directory flush | `fsync` on the directory | `fsync` on the directory | Not possible; NTFS journals renames |
| Same disk check | Device ID | Device ID | Volume prefix of the canonical path |
| Unix permissions | Restored, without setuid, setgid and sticky | Same | Ignored |
| Symlinks | Created | Created | Written as a small text file holding the target, as Git does without developer mode |
