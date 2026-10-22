#!/bin/sh
# Run as root inside a clean container right after the .deb or .rpm is
# installed (packaging/linux/test-packages.sh does that). Fails unless every
# command starts and every file `man hatchzip`, the menus and the shells
# look for is where they look.
set -eu

fail() {
    echo "check-installed: $*" >&2
    exit 1
}

hatchzip --version
hz --version
# Proves the desktop app's shared libraries are all there, which the
# package's dependencies have to guarantee. Needs no display.
hatchzip-gui --help >/dev/null || fail "hatchzip-gui does not start"

for f in \
    /usr/share/man/man1/hatchzip.1.gz \
    /usr/share/bash-completion/completions/hatchzip \
    /usr/share/fish/vendor_completions.d/hatchzip.fish \
    /usr/share/applications/io.github.xevrion.hatchzip.desktop \
    /usr/share/metainfo/io.github.xevrion.hatchzip.metainfo.xml \
    /usr/share/icons/hicolor/48x48/apps/io.github.xevrion.hatchzip.png \
    /usr/share/icons/hicolor/scalable/apps/io.github.xevrion.hatchzip.svg; do
    [ -s "$f" ] || fail "$f is missing or empty"
done
[ -s /usr/share/zsh/vendor-completions/_hatchzip ] || [ -s /usr/share/zsh/site-functions/_hatchzip ] ||
    fail "no zsh completion"

gzip -t /usr/share/man/man1/hatchzip.1.gz
gzip -dc /usr/share/man/man1/hatchzip.1.gz | grep -q '^\.TH' || fail "hatchzip.1.gz is not a man page"
[ "$(readlink /usr/share/man/man1/hz.1.gz)" = hatchzip.1.gz ] || fail "hz.1.gz does not point at hatchzip.1.gz"
[ "$(readlink /usr/bin/hz)" = hatchzip ] || fail "/usr/bin/hz does not point at hatchzip"

echo "check-installed: everything is in place"
