# TODO

Ticked when it is merged and tested, not when it is started.

## Launch: before announcing anywhere

- [x] ZIP, 7z, tar and split archive extraction, freeing space as it goes
- [x] Crash-safe resume, tested by killing the process at every commit point
- [x] Space check before touching anything, with a kept-free margin
- [x] CLI: help with examples, shell completions, man page, exit codes, `--quiet`, `list`, `test`
- [x] MIT OR Apache-2.0, repository public
- [x] CI green on Linux, macOS and Windows (the full suite, crash tests included, on real Windows and macOS runners)
- [x] Ctrl+C message tells the truth: it stops mid-file and redoes that batch on resume
- [x] Extract into a folder that already has other files ("Extract here"), never overwriting one
- [x] `hatchzip restore`: turn a half-extracted archive back into a normal ZIP, 7z or tar any tool can open (tested with `unzip`, `7z` and GNU `tar`, including interrupted restores)
- [ ] Decide whether `--verify-first` becomes the default
- [x] Release workflow: static binaries for Windows x64, macOS arm64 and x64, Linux x64 and arm64, smoke tested, with SHA-256 checksums (dry run passed on all five)
- [ ] First tagged release (`git tag v0.1.0 && git push --tags`)
- [x] One-line install: `install.sh` for Linux and macOS (tested locally, including a tampered checksum and uninstall), `install.ps1` for Windows
- [ ] Test `install.ps1` on a real Windows machine against a published release
- [x] Archive over 4 GB with 64-bit offsets, including a crash and resume past 4 GiB (sparse test archive)
- [ ] Real run on a big real-world archive and on a real Windows machine
- [ ] Demo video: `unzip` fails on a nearly full disk, HatchZip finishes (1920x1080, 60 fps, for X)
- [ ] Demo GIF at the top of the README
- [ ] Publish to crates.io so `cargo install hatchzip` works
- [ ] Open a handful of `good first issue` issues from the lists below
- [ ] Launch posts: X thread, Show HN, r/rust, r/DataHoarder, r/linux

## GUI, like WinRAR and 7-Zip

- [x] Split the code into a workspace: `core` library, `cli`, `shell`, `gui`
- [x] Choose the GUI framework: Slint (native, no web view, software renderer when there is no GPU)
- [x] Open an archive and browse its contents (a flat list with sizes; a folder tree is still to do)
- [x] Show the space plan before extracting: fits or not, how much is needed, free after
- [x] Task windows for right-click jobs with progress, speed, time left, pause and resume
- [x] Detect half-extracted archives and offer to continue
- [x] Drag and drop archives onto the window (Windows, macOS, X11; Wayland has no file drops in winit yet)
- [x] Settings in the window: kept-free space, verify first, output folder
- [ ] Remember settings between runs
- [ ] Contents as a folder tree
- [x] Light and dark theme following the system
- [x] Falls back to the software renderer when OpenGL won't start (tested with a broken EGL on Linux)
- [ ] Run and click through the GUI on real Windows and macOS
- [ ] Optional file association: open `.zip`, `.7z` and `.tar` in HatchZip

## Right-click menu and OS integration

- [x] Windows Explorer: HatchZip submenu with "Extract here", "Extract to a new folder" and "Test", per user without admin rights (`crates/shell`; registry keys tested in CI, not yet clicked in a real Explorer)
- [x] Linux: Dolphin service menu, GNOME Files scripts and Nemo actions (`crates/shell`; files tested, not yet clicked)
- [ ] Wire `crates/shell` into the GUI: `--register-shell` / `--unregister-shell` and a settings switch
- [ ] Thunar custom actions
- [ ] Windows 11 top-level context menu (the classic one sits under "Show more options"); needs a packaged shell extension
- [ ] macOS: Finder Quick Action (needs a Mac to test)
- [ ] Windows installer (MSI or NSIS) and winget
- [ ] macOS: dmg and Homebrew
- [ ] Linux: deb, rpm, AUR, Flatpak
- [ ] Code signing for Windows SmartScreen and macOS notarization (paid certificates)

## Formats

- [x] ZIP, ZIP64, self-extracting `.exe` ZIPs
- [x] 7z, freed one solid block at a time
- [x] Uncompressed tar, including long names, symlinks and hard links
- [x] Split archives (`.001`, `.002`, ...), volumes deleted as they empty
- [ ] Free space inside one solid block or stream (hole punching plus resume points); fixes 7z made with 7-Zip's default settings
- [ ] `.tar.gz` (resume points at deflate block boundaries)
- [ ] `.tar.xz` (per xz block)
- [ ] `.tar.zst`
- [ ] RAR, starting with deleting finished volumes of `.partN.rar` sets
- [ ] Native split ZIP (`.z01`, `.z02`, ..., `.zip`)
- [x] Password-protected ZIP (ZipCrypto, AES) and 7z (AES, encrypted headers too), proven before anything is deleted, never saved (CLI; tested including a real `zip -P` archive through a terminal prompt)
- [ ] Password dialog in the GUI
- [x] Codec tests with archives from 7-Zip 25.01: Deflate64, bzip2, LZMA, PPMd in ZIP; LZMA, BCJ+LZMA2, bzip2, Deflate, PPMd in 7z
- [x] Archives over 4 GiB: entries past the 4 GiB mark, crashed and resumed there (sparse test archive, Linux and macOS)
- [ ] Tests for XZ in ZIP and Zstandard in 7z (7-Zip itself can't write those)

## Website

- [x] Plain static page in the spirit of 7-zip.org: downloads, what it does, measured results, limits (`site/`, about 11 KB, no JavaScript, strict CSP)
- [ ] Pick and buy the domain, then fill in the canonical URL, sitemap and Open Graph URL
- [ ] Deploy (GitHub Pages workflow is ready and manual-only; Cloudflare Pages also works)
- [ ] Screenshots of the GUI once it exists

## Safety and polish

- [ ] `--skip-damaged`: save what can be saved as `name.damaged` and keep going
- [ ] Read each file back before cutting its bytes from the archive, to catch antivirus quarantine and bad writes
- [x] Windows: reserved names (`CON`, `NUL`, ...), trailing dots and spaces, and forbidden characters are refused before anything is touched
- [ ] Windows: a test for paths over 260 characters (Rust's standard library handles them, unverified here)
- [ ] Benchmarks on more hardware, including a hard drive and a USB stick
