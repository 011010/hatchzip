# Packaging

How each download on the releases page is made. The release workflow
(`.github/workflows/release.yml`) does all of it on a tag; this is what it
runs, so any step can be repeated by hand.

| Download | Contents | Built with | On |
|---|---|---|---|
| `hatchzip-<arch>-<os>.tar.gz`, `hatchzip-x86_64-windows.zip` | The CLI, man page, completions | `cargo build`, `tar`, `7z` | Each OS |
| `HatchZip-x64-setup.exe` | App, CLI (`hatchzip.exe` and `hz.exe`) | [Inno Setup 6](https://jrsoftware.org/isinfo.php) | Windows |
| `HatchZip-aarch64.dmg`, `HatchZip-x86_64.dmg` | `HatchZip.app`, with the CLI and man page inside | `hdiutil`, `codesign` | macOS |
| `hatchzip_amd64.deb`, `hatchzip-x86_64.rpm` | App, CLI, `hz` link, man pages, completions, menu entry, icons | [nfpm](https://nfpm.goreleaser.com) | Ubuntu 22.04 |
| `HatchZip-x86_64.AppImage` | App | [linuxdeploy](https://github.com/linuxdeploy/linuxdeploy), [appimagetool](https://github.com/AppImage/appimagetool) | Ubuntu 22.04 |

Every file gets a `.sha256` next to it. Every package is installed and run
before anything is published: in clean Ubuntu 22.04 and Fedora containers
for the .deb and .rpm, by a silent install and uninstall on the Windows
runner, and by opening the dmg on the Mac runner.

The Windows and macOS jobs build the app next to the CLI and package both.
The Linux packages have a job of their own on Ubuntu 22.04: the app links
the C library and fontconfig, so it's built on the oldest system it should
run on, while the CLI inside is the static musl build from the tarball.

## Why these tools

One tool for everything, [cargo-packager](https://github.com/crabnebula-dev/cargo-packager),
was the other candidate. It's maintained, but its releases ship no binaries
(the latest, 0.11.8, is from November 2025), so every CI job would compile
it first, and the Windows requirements below would need NSIS hooks anyway.
These tools are all prebuilt, pinned by version and SHA-256, and each one
does one thing that's easy to check:

- **nfpm** writes .deb and .rpm from one YAML file, without dpkg or rpmbuild.
- **Inno Setup** is already on GitHub's Windows runners, installs per user
  without admin rights, and its Pascal code makes the PATH edit exact.
- **hdiutil** and **codesign** come with macOS; an `.app` is a folder and a
  plist.
- **linuxdeploy** copies in any shared library the app links that isn't on
  every desktop, and **appimagetool** packs the folder. The AppImage runtime
  is pinned too, rather than downloaded at whatever version is newest.

## Icon

`assets/icon/hatchzip.svg` is the drawing, on a 16 px grid. 16 and 24 px
have their own pixel-fitted drawings next to it. Everything else is generated
and committed, so packaging never needs librsvg:

```sh
python3 -I assets/icon/build.py   # needs rsvg-convert
```

It writes the PNGs in `assets/icon/png/`, `hatchzip.ico` (16 to 256 px),
`hatchzip.icns` (32 to 1024 px, covering 16 to 512 pt on normal and Retina screens) and the `hicolor` set in
`packaging/linux/icons/`. `site/favicon.svg` is a copy of the drawing.

## Linux: .deb and .rpm

`linux/nfpm.yaml` lists every file. It reads them from `dist/pkg/`:

```sh
cargo build --release --locked --bin hatchzip-gui
mkdir -p dist/pkg/completions
cp path/to/static/hatchzip target/release/hatchzip-gui dist/pkg/
dist/pkg/hatchzip man | gzip -9n > dist/pkg/hatchzip.1.gz
dist/pkg/hatchzip completions bash > dist/pkg/completions/hatchzip.bash
dist/pkg/hatchzip completions zsh > dist/pkg/completions/_hatchzip
dist/pkg/hatchzip completions fish > dist/pkg/completions/hatchzip.fish
HATCHZIP_VERSION=0.1.0 nfpm package -f packaging/linux/nfpm.yaml -p deb -t dist/hatchzip_amd64.deb
HATCHZIP_VERSION=0.1.0 nfpm package -f packaging/linux/nfpm.yaml -p rpm -t dist/hatchzip-x86_64.rpm
sh packaging/linux/test-packages.sh   # docker; DOCKER=podman works too
```

The CLI is the static musl build from the
tarball, so the packages' only real dependency is the app's C library.
`linux/check-installed.sh` is what the container test checks.

Both packages install to `/usr`: `hatchzip`, `hz` (a link to it) and
`hatchzip-gui` in `/usr/bin`, `hatchzip.1.gz` and `hz.1.gz` so
`man hatchzip` and `man hz` work, bash, zsh and fish completions, the desktop
entry, AppStream metadata and icons. They don't add the right-click menu:
that lives in each user's home, and a package runs as root, so the app turns
it on.

Desktop entry and metadata checks:

```sh
desktop-file-validate packaging/linux/io.github.xevrion.hatchzip.desktop
appstreamcli validate --no-net packaging/linux/io.github.xevrion.hatchzip.metainfo.xml
```

## Linux: AppImage

```sh
sh packaging/linux/build-appimage.sh target/release/hatchzip-gui
```

Build it on the oldest system it should run on: it uses that system's C
library. CI uses Ubuntu 22.04.

## Windows: setup

```powershell
cargo build --release --locked --bin hatchzip-gui
# dist\payload: the files of hatchzip-x86_64-windows.zip, plus hatchzip-gui.exe
packaging\windows\build-setup.ps1 dist\payload 0.1.0
packaging\windows\test-setup.ps1 dist\HatchZip-x64-setup.exe
```

The setup installs for the current user to
`%LOCALAPPDATA%\Programs\HatchZip`, the folder `install.ps1` uses, with no
admin prompt. It adds a Start menu entry, an entry in Settings > Apps that
uninstalls it, `hz.exe` as a copy of `hatchzip.exe`, the folder on the user
PATH, and HatchZip under "Open with" for .zip, .7z, .tar and .001 files. It
runs `hatchzip-gui --register-shell` after installing and
`hatchzip-gui --unregister-shell` before uninstalling. Uninstalling removes
all of it, PATH entry included.

## macOS: dmg

```sh
cargo build --release --locked --bin hatchzip-gui --target aarch64-apple-darwin
sh packaging/macos/build-dmg.sh aarch64 \
    target/aarch64-apple-darwin/release/hatchzip-gui dist/hatchzip-aarch64-macos
sh packaging/macos/test-dmg.sh dist/HatchZip-aarch64.dmg
```

The last argument of `build-dmg.sh` is the unpacked CLI tarball for the same
architecture. `macos/Info.plist` declares zip, 7z and tar as documents
HatchZip can open, so it shows up in Finder's "Open With" without replacing
Archive Utility as the default.

The app is signed ad hoc, not with a Developer ID, and not notarized, so
macOS asks before the first launch:

- macOS 15 and later: open HatchZip, close the warning, then go to System
  Settings > Privacy & Security and click "Open Anyway".
- macOS 14 and earlier: Control-click HatchZip in Applications, choose Open,
  then Open again.
- Or, in Terminal: `xattr -dr com.apple.quarantine /Applications/HatchZip.app`

The CLI and its man page are inside the app. To use them from a terminal:

```sh
sudo mkdir -p /usr/local/bin /usr/local/share/man/man1
sudo ln -sf /Applications/HatchZip.app/Contents/MacOS/hatchzip /usr/local/bin/hatchzip
sudo ln -sf /Applications/HatchZip.app/Contents/MacOS/hatchzip /usr/local/bin/hz
sudo ln -sf /Applications/HatchZip.app/Contents/Resources/man/man1/hatchzip.1 /usr/local/share/man/man1/hatchzip.1
```

## What the packages expect from `hatchzip-gui`

- `--help` prints and exits without opening a window or needing a display:
  the Linux and macOS tests run it to prove the app starts.
- `--register-shell` and `--unregister-shell` exit when done, with no window.
- On Windows, the exe carries `assets/icon/hatchzip.ico` as its first icon
  resource (the right-click menu, Start menu and "Open with" all show icon 0
  of the exe) and a version resource.
- On Linux, the window's Wayland app ID and X11 class are
  `io.github.xevrion.hatchzip`, so the desktop shows the menu entry's icon.
- Inside an AppImage the program runs from a temporary mount, so
  `--register-shell` should point the menu at `$APPIMAGE`, not at its own path.
- On macOS, "Open With" and drops on the Dock icon arrive as an Apple Event,
  not as a command line argument.
