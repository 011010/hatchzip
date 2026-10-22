#!/bin/sh
# Installs the .deb and the .rpm in clean containers, checks them with
# check-installed.sh, removes them again, and starts the AppImage. Run from
# the repository root after the packages are in dist/:
#
#   sh packaging/linux/test-packages.sh
#
# Needs docker (or podman with DOCKER=podman). Ubuntu 22.04 is the oldest
# system the packages promise to run on, so it's the one tested; Fedora is
# the newest rpm system.
set -eu

DOCKER="${DOCKER:-docker}"
here=$(cd "$(dirname "$0")" && pwd)
dist=$(cd dist && pwd)

# Dependencies only, no recommended packages: the desktop app has to start
# with just what the package requires.
# shellcheck disable=SC2016 # expanded by the shell inside the container
"$DOCKER" run --rm -v "$dist:/dist:ro" -v "$here/check-installed.sh:/check.sh:ro" ubuntu:22.04 sh -euc '
    # Ubuntu container images drop man pages on install; the check wants them.
    rm -f /etc/dpkg/dpkg.cfg.d/excludes
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends /dist/hatchzip_amd64.deb
    sh /check.sh
    apt-get remove -y -qq hatchzip
    for f in /usr/bin/hatchzip /usr/bin/hz /usr/bin/hatchzip-gui /usr/share/man/man1/hatchzip.1.gz; do
        if [ -e "$f" ] || [ -L "$f" ]; then echo "$f is left behind" >&2; exit 1; fi
    done
    echo "deb: removed cleanly"
'

# shellcheck disable=SC2016
"$DOCKER" run --rm -v "$dist:/dist:ro" -v "$here/check-installed.sh:/check.sh:ro" fedora:latest sh -euc '
    dnf install -y -q --setopt=install_weak_deps=False /dist/hatchzip-x86_64.rpm
    sh /check.sh
    dnf remove -y -q hatchzip
    for f in /usr/bin/hatchzip /usr/bin/hz /usr/bin/hatchzip-gui /usr/share/man/man1/hatchzip.1.gz; do
        if [ -e "$f" ] || [ -L "$f" ]; then echo "$f is left behind" >&2; exit 1; fi
    done
    echo "rpm: removed cleanly"
'

# Extracts itself instead of mounting, so it runs without FUSE.
chmod +x "$dist/HatchZip-x86_64.AppImage"
"$dist/HatchZip-x86_64.AppImage" --appimage-extract-and-run --help >/dev/null
echo "AppImage: starts"
