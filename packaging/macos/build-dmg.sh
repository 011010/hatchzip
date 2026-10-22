#!/bin/sh
# Builds dist/HatchZip-<arch>.dmg: HatchZip.app next to a link to
# /Applications, to drag it onto. Runs on a Mac (hdiutil, codesign), from
# the repository root:
#
#   sh packaging/macos/build-dmg.sh aarch64 \
#       target/aarch64-apple-darwin/release/hatchzip-gui \
#       dist/hatchzip-aarch64-macos
#
# The last argument is the unpacked CLI release tarball. Its hatchzip and
# man page go inside the app too, so the one download has everything; the
# app's README section in packaging/README.md shows how to put them on PATH.
#
# Not signed with a Developer ID or notarized yet. The app is signed ad hoc,
# which Apple Silicon requires to run anything at all; Gatekeeper still asks
# the user to confirm the first launch.
set -eu

arch="$1"
gui="$2"
cli="$3"

# Read from the workspace rather than by running the CLI, since the Intel
# build can't run on an Apple Silicon Mac without Rosetta.
version="${HATCHZIP_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)}"
[ -n "$version" ] || { echo "build-dmg.sh: no version in Cargo.toml" >&2; exit 1; }

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
app="$stage/HatchZip.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/man/man1"

cp "$gui" "$app/Contents/MacOS/hatchzip-gui"
cp "$cli/hatchzip" "$app/Contents/MacOS/hatchzip"
chmod 755 "$app/Contents/MacOS/hatchzip-gui" "$app/Contents/MacOS/hatchzip"
cp "$cli/man/hatchzip.1" "$app/Contents/Resources/man/man1/hatchzip.1"
cp assets/icon/hatchzip.icns "$app/Contents/Resources/HatchZip.icns"
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
plutil -lint "$app/Contents/Info.plist"

# Inside out: the helper first, then the bundle, whose signature covers it.
codesign --force --sign - "$app/Contents/MacOS/hatchzip"
codesign --force --sign - "$app"
codesign --verify --strict --verbose=2 "$app"

ln -s /Applications "$stage/Applications"
mkdir -p dist
hdiutil create -volname HatchZip -srcfolder "$stage" -fs HFS+ -format UDZO -ov "dist/HatchZip-$arch.dmg"
