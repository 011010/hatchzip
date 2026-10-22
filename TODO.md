# TODO

Ticked when it is merged and tested, not when it is started.

## Launch: before announcing anywhere

- [x] ZIP, 7z, tar and split archive extraction, freeing space as it goes
- [x] Crash-safe resume, tested by killing the process at every commit point
- [x] Space check before touching anything, with a kept-free margin
- [x] CLI: help with examples, shell completions, man page, exit codes, `--quiet`, `list`, `test`
- [x] MIT OR Apache-2.0, repository public
- [ ] CI green on Linux, macOS and Windows
- [ ] Ctrl+C message tells the truth: it stops mid-file and redoes that batch on resume, it doesn't finish the current files
- [ ] `hatchzip restore`: turn a half-extracted archive back into a normal ZIP or 7z any tool can open
- [ ] Decide whether `--verify-first` becomes the default
- [ ] Prebuilt binaries on GitHub Releases: Windows x64, macOS arm64 and x64, Linux x64 and arm64 (static), with SHA-256 checksums
- [ ] One-line install: `install.sh` for Linux and macOS, `install.ps1` for Windows
- [ ] Real run on an archive over 4 GB (64-bit ZIP offsets) and on a real Windows machine
- [ ] Demo video: `unzip` fails on a nearly full disk, HatchZip finishes (1920x1080, 60 fps, for X)
- [ ] Demo GIF at the top of the README
- [ ] Publish to crates.io so `cargo install hatchzip` works
- [ ] Open a handful of `good first issue` issues from the lists below
- [ ] Launch posts: X thread, Show HN, r/rust, r/DataHoarder, r/linux

## GUI, like WinRAR and 7-Zip

- [ ] Split the code into a workspace: `core` library, `cli`, `gui`
- [ ] Choose the GUI framework
- [ ] Open an archive and browse its contents as a tree, with sizes
- [ ] Show the space plan before extracting: fits or not, how much is needed, free after
- [ ] Extraction window with progress, pause, resume and cancel
- [ ] Detect half-extracted archives and offer to continue
- [ ] Drag and drop archives onto the window
- [ ] Settings: kept-free space, verify first, default output folder
- [ ] Light and dark theme following the system
- [ ] Optional file association: open `.zip`, `.7z` and `.tar` in HatchZip

## Right-click menu and OS integration

- [ ] Windows Explorer: HatchZip submenu with "Extract here", "Extract to \<name\>\\", "Check space" and "Test", registered per user without admin rights
- [ ] Windows 11 top-level context menu (the classic one sits under "Show more options"); needs a packaged shell extension
- [ ] Linux: KDE Dolphin service menu, GNOME Files, Nemo and Thunar actions
- [ ] macOS: Finder Quick Action
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
- [ ] Password-protected ZIP and 7z
- [ ] Tests for the codecs decoded but not yet tested: Deflate64, LZMA, XZ, PPMd in ZIP; LZMA, BCJ, bzip2, Deflate, PPMd, Zstandard in 7z

## Safety and polish

- [ ] `--skip-damaged`: save what can be saved as `name.damaged` and keep going
- [ ] Read each file back before cutting its bytes from the archive, to catch antivirus quarantine and bad writes
- [ ] Windows: reserved names (`CON`, `NUL`, ...), trailing dots and spaces, paths over 260 characters
- [ ] Benchmarks on more hardware, including a hard drive and a USB stick
