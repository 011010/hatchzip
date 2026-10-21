<h1 align="center">HatchZip</h1>

<p align="center">
  Extract huge archives in place. The archive shrinks as your files hatch,<br>
  so you only need free space for the output, not for the output <em>and</em> the archive.
</p>

<h4 align="center">
  <a href="#installation">Installation</a> |
  <a href="#usage">Usage</a> |
  <a href="#is-it-safe">Safety</a> |
  <a href="docs/how-it-works.md">How it works</a> |
  <a href="CONTRIBUTING.md">Contributing</a>
</h4>

<p align="center">
  <a href="https://github.com/xevrion/hatchzip/actions/workflows/ci.yml"><img src="https://github.com/xevrion/hatchzip/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-FSL--1.1--ALv2-blue.svg" alt="FSL-1.1-ALv2 licensed" /></a>
  <img src="https://img.shields.io/badge/platform-linux%20%7C%20windows%20%7C%20macos-blue.svg" alt="Linux, Windows and macOS" />
</p>

A 180 GB game download that unpacks to 200 GB needs 380 GB of disk with WinRAR, 7-Zip or `unzip`, because the archive and everything extracted from it have to exist at the same time. HatchZip needs a little over 200 GB. It extracts the archive from the end and cuts each finished part off the archive file as it goes, so the space the archive gives back is reused for the next files.

It checks that the extraction will fit before it changes anything, keeps a safety margin of free space the whole way through, and can be stopped or crash at any moment and pick up where it left off. It reads ZIP, 7z, tar and split archives.

## Highlights

The same 200 MB of game data (21 files), packed by the real tools, extracted on a 220 MiB disk with about 50 MB free:

| Archive | `unzip` / 7-Zip | HatchZip |
|---|---|---|
| ZIP, 180 MB | `unzip` ran out of space at file 6 of 21 | Finished, every file byte-identical |
| 7z from 7-Zip 25.01, `-ms=20m` (10 solid blocks) | Needs 380 MB | Finished in 0.64 s, every file byte-identical |
| 7z split into 5 volumes of 40 MB | Needs 380 MB | Finished; volumes deleted one at a time, peak 210 MB used |
| tar from GNU tar, 200 MB | Needs 400 MB | Finished in 0.12 s, every file byte-identical |
| 7z from 7-Zip with default settings (1 solid block) | Needs 380 MB | Refused before touching anything: one solid block can't be freed until all of it is out, so it needs 200 MB free |

| | Result |
|---|---|
| ZIP extraction speed, same data | 0.25 to 0.35 s, against 1.48 to 1.50 s for `unzip` 6.00, including an fsync `unzip` skips |
| Crash safety | Killed at both unsafe moments of a batch at many points of each format's run; every resume finished with identical output |
| Works on | Any filesystem that can shrink a file: ext4, btrfs, XFS, NTFS, APFS, and also exFAT and FAT32 USB drives |

Measured on Fedora 43, Intel Core i7-14700HX, SK Hynix NVMe SSD, warm page cache; the disk-space runs used a 220 MiB tmpfs to stand in for a full laptop disk, the speed runs ext4. Speed comes from decompressing files in parallel, so an archive holding one giant file extracts at the speed of a single core.

The extra space HatchZip needs beyond the output is about the size of the largest single unit it can free at once: one file for ZIP and tar, one solid block for 7z. `hatchzip check` computes the exact figure for any archive.

## Installation

HatchZip has no published releases yet. Build it from source with [Rust](https://rustup.rs) (the version is pinned in `rust-toolchain.toml` and installed automatically):

```sh
git clone https://github.com/xevrion/hatchzip.git
cd hatchzip
cargo build --release
```

The binary is `target/release/hatchzip` (`hatchzip.exe` on Windows). It is a single self-contained file; copy it anywhere on your `PATH`.

### Shell completions and man page

```sh
hatchzip completions bash > ~/.local/share/bash-completion/completions/hatchzip
hatchzip completions zsh > "${fpath[1]}/_hatchzip"
hatchzip completions fish > ~/.config/fish/completions/hatchzip.fish
hatchzip completions powershell >> $PROFILE

hatchzip man > ~/.local/share/man/man1/hatchzip.1    # then: man hatchzip
```

## Usage

### See whether it fits

```sh
hatchzip check game.zip
```

```
game.zip  180.2 MB, unpacks to 200.0 MB in 22 entries
  free space now        50.5 MB
  normal extractors     200.0 MB free
  HatchZip needs        29.9 MB free (+5.0 MB kept spare)
  free after            30.6 MB
```

`check` reads the archive's index and the disk's free space and changes nothing.

### Extract

```sh
hatchzip extract game.zip                 # into ./game/
hatchzip extract game.zip -o D:\Games\Foo # somewhere else
hatchzip x game.7z.001                    # split archive; any part's name works
```

HatchZip prints the same space plan, asks for confirmation, then extracts. When it finishes, the archive is gone and the files are in the output folder. For a split archive, each volume is deleted as soon as everything in it is extracted.

| Option | Effect |
|---|---|
| `-o, --output <dir>` | Output folder. Defaults to a folder named after the archive, next to it. It must be empty or not exist yet. |
| `--keep-free <size>` | Free space HatchZip never uses, such as `500MB` or `10GB`. Default `2GB`, so the disk never fills to zero. |
| `--verify-first` | Read the whole archive and check every file's checksum before deleting anything. Takes extra time, recommended for downloads you can't easily get again. |
| `-y, --yes` | Skip the confirmation prompt. Required when not run from a terminal. |
| `-q, --quiet` | Print only errors and prompts. |

### Stop and resume

Press `Ctrl+C` and HatchZip finishes the files it is writing, saves its progress and exits. A second `Ctrl+C` quits immediately, which is also safe. To continue, run the same command again:

```sh
hatchzip extract game.zip
```

The same applies after a crash, a power cut, or running out of space because something else filled the disk. Progress lives in two small files next to the archive, `game.zip.hatchzip` and `game.zip.hatchzip-tail`. **Do not delete them while an extraction is unfinished**: they hold the archive's index, which is no longer inside the archive itself.

### List and test

```sh
hatchzip list game.7z    # what's inside, with sizes
hatchzip test game.7z    # read every file and verify its checksum, writing nothing
```

Plain tar has no checksums for file contents, so `test` on a tar only proves every file can be read in full.

### Exit status

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Failure. If extraction had started, progress is saved. |
| 2 | Invalid command line |
| 3 | Not enough free space. Nothing was changed. |
| 4 | The archive is damaged. Nothing was deleted. |
| 130 | Paused with Ctrl+C. Run the same command to continue. |

## Is it safe

HatchZip destroys the archive as it extracts, so it is built around never losing a file that has not safely landed on disk.

- **Nothing changes until the plan fits.** The space check runs before the first byte is touched, and it replays the whole extraction to find the moment that needs the most free space, not just the total.
- **A file is written, flushed to the physical disk and renamed into place before the part of the archive holding it is cut off.** A crash at any point leaves either the file or its bytes in the archive, never neither.
- **Damage stops the run where it is.** Every file's CRC-32 is checked as it is extracted. A damaged file is not written, and it stays in the archive along with everything before it.
- **Hostile archives are refused before anything is deleted:** paths that escape the output folder, entries that overlap each other, entries that appear twice. Setuid and setgid bits are never restored, and symlinks are created only after every regular file is written, so nothing can be written through one.

What it cannot protect against: if the download itself is damaged, the files before the damaged spot are extracted and the archive is partly gone. Use `--verify-first` when the archive can't be downloaded again. On btrfs or APFS, space held by a snapshot that includes the archive is not freed until the snapshot is deleted.

## Supported formats

| Format | Status | Freed in steps of |
|---|---|---|
| ZIP, ZIP64, self-extracting `.exe` ZIPs | Supported. Stored, Deflate, Zstandard and bzip2 are covered by tests; Deflate64, LZMA, XZ and PPMd are decoded by the same library but not yet tested here. | One file |
| 7z, solid and non-solid | Supported. LZMA2, 7-Zip's default, is tested with archives from 7-Zip itself; LZMA, BCJ filters, bzip2, Deflate, PPMd and Zstandard are decoded by the same library but not yet tested here. | One solid block |
| tar (uncompressed) | Supported, including long names, symlinks and hard links | One file |
| Split archives (`.001`, `.002`, ...) of ZIP or 7z, as 7-Zip makes them | Supported | One file or block; each volume is deleted once emptied |
| Password-protected ZIP or 7z | Refused before extraction, planned | |
| `.tar.gz`, `.tar.zst`, `.tar.xz` | Refused before extraction, planned. One continuous stream, which needs resumable checkpoints inside it; see [the format notes](docs/formats.md). | |
| One huge solid 7z block | Works, but needs free space for the whole block, as the table above shows. Freeing space inside a block is the same planned technique. | |
| RAR, native split ZIP (`.z01`) | Planned | |

## How it works

ZIP, 7z and tar all store their contents as a sequence of independent pieces: files for ZIP and tar, solid blocks for 7z. ZIP and 7z keep their index at the very end. HatchZip saves that index to a side file, then extracts pieces starting from the last one in the archive. After each batch it truncates the archive to where that batch began, which every operating system and filesystem supports; for a split archive, that means deleting whole volumes and shortening one. The full design, including the exact order of writes that makes a crash safe, is in [How it works](docs/how-it-works.md).

## License

HatchZip is source available under the [Functional Source License 1.1, Apache 2.0 future license](LICENSE). You may use, study, modify and share it for free, including at work, but you may not sell it or offer it as part of a competing product or service. Every release becomes Apache 2.0 two years after it is published.

## Contributing

Bug reports, especially from filesystems and operating systems not covered by CI, are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) first; contributions require agreeing to the terms described there.
