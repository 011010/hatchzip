# Project coding standards

HatchZip extracts archives in place: it extracts ZIP entries from the last to the first and truncates the archive behind itself, so the user only needs free space for the output. `src/engine.rs` plans and runs the extraction, `src/journal.rs` holds the crash-safety state, `src/platform/` everything that differs per OS. Read [docs/how-it-works.md](docs/how-it-works.md) before touching either of the first two.

## Communication

- Be succinct. Prefer code over prose.
- Do not summarise your changes afterwards unless asked.
- State what you did not verify. An honest gap costs less than a confident wrong claim.

## Correctness

- The tool destroys its input. The invariant is absolute: the journal never claims a file before it is durable, and the archive is never truncated past an entry the journal doesn't claim.
- Anything that can be refused is refused before the archive is touched.
- Every number in the README and docs is measured on real hardware, never estimated. Paste the measurement when you change one.
- A behavior is not finished until it ran for real: a real archive extracted, the output compared byte for byte, the crash tests passing.
- Never run `extract` on an archive you can't recreate. It deletes it.

## Operating systems

- Everything that differs between systems lives in `src/platform/`, one file per system (`linux.rs`, `macos.rs`, `windows.rs`, shared Unix code in `unix.rs`) providing the same items.
- No `cfg(target_os)`, `cfg(windows)` or `cfg!(windows)` outside that module. `cfg(debug_assertions)` for test knobs is fine.
- CI runs the tests on Linux, macOS and Windows, since nobody develops on the latter two.

## Rust

- Rust 1.95, pinned in `rust-toolchain.toml`. MSRV 1.89 for `File::try_lock`.
- `cargo clippy --all-targets -- -D warnings` clean, `cargo fmt --all` formatted.
- `anyhow` for errors, with context on anything touching the filesystem. Errors tell the user what to do next.
- Comments explain why, not what, and read like a person wrote them.

## Traps already fallen into

- The zip crate scans backwards over the whole archive looking for the end record. `PatchedArchive` must return zeros, not an error, for regions already truncated away, or every resume fails with "No CDFH found".
- On resume, files a crashed batch wrote but never committed still occupy space. They are deleted before the space check, or the check refuses a run that fits.

## Copy

- README and docs are clear, neutral, professional English, sentence case headings, no first person.
- No em dashes and no unicode arrows anywhere: code, comments, commits, docs, CLI output.

## Commands

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
cargo run -- check some.zip
```
