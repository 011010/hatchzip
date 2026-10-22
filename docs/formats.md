# Format notes

Research for the formats after ZIP, checked against crate sources and against real archives made with zstd 1.5.7, xz 5.8.2, 7-Zip 25.01, gzip and pigz. Anything marked unverified has not been tested.

## The general problem

ZIP is easy because every entry decompresses on its own. Solid formats compress everything as one stream, so the archive must be read from the front, and the consumed front must be freed with hole punching. If HatchZip crashes after punching, the decoder state that was needed to continue is gone with it.

The answer is a **resume point**: a saved state from which decoding can restart at a known compressed offset. HatchZip may only punch up to the last durable resume point. A resume point records:

- the compressed offset
- the decoder state
- the uncompressed offset
- the tar parser state (current entry, bytes left in it)
- the length of each open output file
- the running checksum state

The protocol: flush the outputs, write and flush the journal, then punch up to the point just made durable. **Done** for single files stored or deflated in ZIP and for tar entries ([How it works](how-it-works.md#freeing-space-inside-a-large-file)); the deflate decoder state is the bit position plus the last 32 KiB of output, read back from the flushed output file instead of stored in the journal.

## Per format

| Format | Restart granularity available | Crate | Work needed |
|---|---|---|---|
| 7z | **Done** (`crates/core/src/archive/sevenz.rs`). Per folder (solid block), truncating from the end like ZIP. Inside a folder, only at LZMA2 full-reset chunks. | `sevenz-rust2` (Apache-2.0): `BlockDecoder` decodes one folder from our own `Read + Seek`; pack positions and sizes are public. | Freeing space inside one solid block is the remaining work; 7-Zip's defaults can put a whole archive in one block. |
| Split archives | **Done** (`crates/core/src/archive/storage.rs`) for byte splits. | n/a | `.7z.001` and `.zip.001` are byte splits of one archive. Native `.z01` ZIP, which keeps the directory in the last part and numbers disks in its offsets, is not done. |
| tar | **Done** (`crates/core/src/archive/tar.rs`) for uncompressed tar, per entry. | `tar` (MIT or Apache) | |
| `.tar.gz` | Any deflate block boundary, about 32 KiB of state (the zran.c technique). Gzip member ends are free restart points. | `libz-rs-sys` (Zlib license) exports `inflatePrime`, `inflateSetDictionary` and `Z_BLOCK`. The safe `zlib-rs` API does not expose them. | Medium. A zran-style raw inflate loop over FFI, our own gzip header parsing, running CRC-32. |
| `.tar.xz` | Per xz block. `xz` 5.8 defaults to multithreaded mode, which writes many blocks (7 blocks for a 162 MiB file at `-6`); `-T1` writes one. | `lzma-rust2` (Apache-2.0, pure Rust): start a fresh `Lzma2Reader` per block. | Medium. Parse the xz index ourselves. Chunk-level restarts need a fork that saves the LZMA state (dictionary of 8 to 64 MiB plus tens of KB). |
| `.tar.zst` | Per frame only with libzstd. The `zstd` CLI writes one frame even with `-T0`, so typical files have no restart point. | `ruzstd` (MIT) keeps exactly the needed state in `DecoderScratch` and can stop at block boundaries, but it is crate-private. | Large. A ruzstd fork with save and load, plus the window (2 MiB at default levels, up to 128 MiB with `--long`). |
| RAR | Per volume. UnRAR opens files by name, with no reader callback and no offsets. | `unrar` wraps the UnRAR C library. `rars` (Apache-2.0, pure Rust, takes `Read + Seek`) is new and its own README asks for more testing. | Medium for volumes. Solid RAR can't be punched safely, because UnRAR's state can't be saved. |

## Dependency licenses

All candidate crates are permissive: zip (MIT), flate2 (MIT or Apache), zlib-rs and libz-rs-sys (Zlib), zstd and zstd-safe (BSD-3), ruzstd (MIT), lzma-rust2 (Apache-2.0), liblzma (MIT or Apache, building only the 0BSD liblzma sources), sevenz-rust2 (Apache-2.0), zesven (MIT or Apache), tar (MIT or Apache). No GPL-only, LGPL-only or non-commercial terms appear in any dependency tree.

The exception to watch is UnRAR. The `unrar` crate is MIT or Apache, but the bundled C code is under the UnRAR freeware license: it may be used to extract RAR archives in any software free of charge, but not to build a RAR-compatible archiver. Shipping modified UnRAR source requires including that license. This reading is not legal advice.

`xz2` and `liblzma` can't be in the same build, since both declare `links = "lzma"`.

## Hole punching per platform

| System | Call | Notes |
|---|---|---|
| Linux | `fallocate(FALLOC_FL_PUNCH_HOLE \| FALLOC_FL_KEEP_SIZE)` | ext4, XFS, btrfs, tmpfs. Verified on ext4: punching 32 MiB of a 64 MiB file dropped its usage from 65M to 33M; punching an unaligned 4,000,008 bytes of an 8 MiB file through `platform::punch_hole` dropped its allocation from 8,392,704 to 4,399,104 bytes (tmpfs: 8,388,608 to 4,395,008). |
| macOS | `fcntl(F_PUNCHHOLE)` with `fpunchhole_t` | Offset and length must be multiples of the filesystem block size, so HatchZip shrinks the range inwards. Can return ENOSPC on cloned files, which HatchZip treats as unsupported. Compiles; unverified whether APFS frees the space. |
| Windows | `FSCTL_SET_SPARSE`, then `FSCTL_SET_ZERO_DATA` | Without marking the file sparse first, NTFS writes real zeros and allocates the range. HatchZip aligns ranges inwards to 64 KiB and counts 1 MiB per end in the space check. Compiles; unverified on real NTFS. The probe measures with `GetCompressedFileSizeW`. |
| FAT32, exFAT | None | Only truncation from the end is possible. |
