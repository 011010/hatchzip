#!/bin/sh
# Opens a dmg the way a user does, copies the app out of it, and checks the
# copy: signature, Info.plist, icon, and that both programs start. Only the
# dmg for this Mac's own architecture can be run.
#
#   sh packaging/macos/test-dmg.sh dist/HatchZip-aarch64.dmg
set -eu

dmg="$1"
mnt=$(mktemp -d)
dest=$(mktemp -d)
cleanup() {
    hdiutil detach "$mnt" -quiet || true
    rm -rf "$dest"
}
trap cleanup EXIT

hdiutil verify "$dmg"
hdiutil attach "$dmg" -nobrowse -readonly -mountpoint "$mnt" -quiet
[ -L "$mnt/Applications" ] || { echo "no Applications link in the dmg" >&2; exit 1; }
# ditto keeps the bundle exactly as it is, signature included.
ditto "$mnt/HatchZip.app" "$dest/HatchZip.app"
app="$dest/HatchZip.app"

codesign --verify --strict --verbose=2 "$app"
plutil -lint "$app/Contents/Info.plist"
[ "$(defaults read "$app/Contents/Info" CFBundleExecutable)" = hatchzip-gui ]
[ -s "$app/Contents/Resources/HatchZip.icns" ]
[ -s "$app/Contents/Resources/man/man1/hatchzip.1" ]

"$app/Contents/MacOS/hatchzip" --version
python3 .github/scripts/smoke.py "$app/Contents/MacOS/hatchzip"
"$app/Contents/MacOS/hatchzip-gui" --help >/dev/null
echo "test-dmg: $(basename "$dmg") is good"
