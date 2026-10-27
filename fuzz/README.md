# Fuzzing

These targets feed mutated archives to the parsers and decoders in `crates/core` with [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) and libFuzzer. Each input is written to a file in a temporary folder of its own per process, then taken through the read-only path of `hatchzip test`: `session::open` without write access, the space check's replay of every unit (`Job::space_needed`), then `Job::verify`, which decodes every unit and writes nothing. Nothing is extracted, since extraction deletes its input.

An error is the expected result for a damaged archive. Only a panic, a crash, a hang or running out of memory is a finding.

## Running a target

cargo-fuzz needs a nightly toolchain:

```sh
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz --locked
```

From the repository root:

```sh
cargo +nightly fuzz run zip
cargo +nightly fuzz run zip -- -max_total_time=120 -max_len=262144 -timeout=60 -rss_limit_mb=4096
cargo +nightly fuzz run rar --features rar
```

The second line uses the limits the CI workflow (`.github/workflows/fuzz.yml`) uses. A finding is saved under `fuzz/artifacts/<target>/`, and running the target on that file alone reproduces it:

```sh
cargo +nightly fuzz run zip fuzz/artifacts/zip/crash-<hash>
```

New inputs that reach new code are added to `fuzz/corpus/<target>/` as the fuzzer runs. Only the seeds there are committed; the rest can be discarded with `git clean -f fuzz/corpus`.

## Targets

Each target writes its input under a fixed name. The format is still recognized from the content first, as for any archive, so the name matters where the content does not decide: a tar without the `ustar` magic, or a gzip stream that only its name says holds a tar. Each target keeps its own corpus, so mutations stay close to one format.

| Target | File name | Covers |
|---|---|---|
| `zip` | `a.zip` | The ZIP reader and its layout checks (central directory, duplicate names, overlapping entries, self-extractor stubs, ZIP64), and the zip crate's decoders: stored, deflate, deflate64, bzip2, LZMA, XZ and PPMd. |
| `sevenz` | `a.7z` | The 7z header parser and layout checks (block offsets, empty blocks, headers inside packed data), and the sevenz-rust2 decoders: LZMA, LZMA2, BCJ, deflate, bzip2, PPMd and zstd. |
| `tar` | `a.tar` | The tar reader: GNU long names, pax records and global headers, links, hard links listed before their file, and entries that run past the end of the file. |
| `targz` | `a.tar.gz` | The gzip header and member reader, the resumable deflate decoder and the tar parser that runs inside it, all of which `targz::open` drives over the whole stream while it opens. |
| `sniff` | `archive` | Format detection from the content alone, then whichever reader it picks. |
| `rar` | `a.rar` | The RAR 4 and RAR 5 header reader in `archive/rar.rs`, then UnRAR's decoding. Built only with `--features rar`, which compiles UnRAR's C++ source, and not run in CI. |

Every target writes a single volume, so split sets are not covered. Encrypted archives are opened without a password: their headers are read up to where a password is needed, and nothing is decrypted.

## Limits

These keep the fuzzer from reporting a slow or large archive as a finding:

- Inputs over 256 KiB are skipped (`MAX_INPUT` in `src/lib.rs`). A compressed tar is decoded whole while it opens, before any size can be checked, and deflate expands at most about 1032 to 1, so this bounds that pass.
- An archive that declares more than 16 MiB of output is opened and planned but not decoded. Every reader stops at the size its entries declare, so a few bytes that declare gigabytes cost nothing.
- `-rss_limit_mb=4096` sits just above the largest dictionary a 7z or ZIP archive can declare, which decoders allocate in one piece.

## Seeds

The seeds in `corpus/` are small archives of 16 KiB at most. The codec samples come from `crates/cli/tests/fixtures/codecs`, and the RAR seeds are single volumes from `crates/cli/tests/fixtures/rar`. The rest were written by Python 3.14's `zipfile`, `tarfile` and `zlib` modules, Info-ZIP 3.0, 7-Zip 25.01 (with uncompressed headers, so mutations reach the header parser rather than an LZMA stream around it), GNU tar 1.35 and pigz 2.8.
