#!/bin/sh
# Builds dist/HatchZip-x86_64.AppImage around the desktop app. Run from the
# repository root on the oldest system the AppImage should run on (CI uses
# Ubuntu 22.04), since it uses that system's C library:
#
#   sh packaging/linux/build-appimage.sh target/release/hatchzip-gui
#
# linuxdeploy copies in the shared libraries the app links that aren't part
# of every desktop system; appimagetool packs the folder into one file. Both
# are downloaded at fixed versions and checked against their SHA-256, and so
# is the runtime appimagetool puts at the front of the file, which it would
# otherwise fetch at whatever version is newest.
set -eu

gui="$1"
id=io.github.xevrion.hatchzip
linux=packaging/linux
tools="${TOOLS_DIR:-dist/appimage-tools}"

LINUXDEPLOY_URL=https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage
LINUXDEPLOY_SHA256=c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d
APPIMAGETOOL_URL=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
RUNTIME_URL=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64
RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d

fetch() {
    curl -fsSL --proto '=https' -o "$tools/$1" "$2"
    echo "$3  $tools/$1" | sha256sum -c -
}

mkdir -p "$tools"
fetch linuxdeploy "$LINUXDEPLOY_URL" "$LINUXDEPLOY_SHA256"
fetch appimagetool "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA256"
fetch runtime "$RUNTIME_URL" "$RUNTIME_SHA256"
chmod +x "$tools/linuxdeploy" "$tools/appimagetool"
# The tools are AppImages themselves; this lets them run without FUSE.
export APPIMAGE_EXTRACT_AND_RUN=1
# The release profile strips the app already, and linuxdeploy's own strip
# is old enough to reject binaries linked on newer systems.
export NO_STRIP=1

appdir=dist/HatchZip.AppDir
rm -rf "$appdir"
set --
for size in 16 24 32 48 64 128 256 512; do
    set -- "$@" --icon-file "$linux/icons/hicolor/${size}x${size}/apps/$id.png"
done
"$tools/linuxdeploy" --appdir "$appdir" \
    --executable "$gui" \
    --desktop-file "$linux/$id.desktop" \
    --icon-file "$linux/icons/hicolor/scalable/apps/$id.svg" \
    "$@"
# Software centres and AppImageHub read this for the description.
install -Dm644 "$linux/$id.metainfo.xml" "$appdir/usr/share/metainfo/$id.appdata.xml"
# The licenses travel with the app, UnRAR's included: its source is compiled
# into the app and its license requires it.
for license in LICENSE-MIT LICENSE-APACHE LICENSE-UNRAR; do
    install -Dm644 "$license" "$appdir/usr/share/licenses/hatchzip/$license"
done

ARCH=x86_64 "$tools/appimagetool" --runtime-file "$tools/runtime" "$appdir" dist/HatchZip-x86_64.AppImage
rm -rf "$appdir"
