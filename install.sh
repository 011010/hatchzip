#!/bin/sh
# Installs HatchZip from the latest release, or updates it. Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/xevrion/hatchzip/main/install.sh | sh
#
# It downloads the build for this machine, checks it against the published
# checksum, and puts `hatchzip` in ~/.local/bin, with its man page and shell
# completions. No root, and nothing outside your home folder is touched.
#
#   sh install.sh --uninstall    removes it again
#
# Environment:
#   HATCHZIP_INSTALL_DIR   where the program goes (default ~/.local/bin)
#   HATCHZIP_VERSION       a release tag like v0.1.0 (default: the latest)

set -eu

REPO="xevrion/hatchzip"
BIN_DIR="${HATCHZIP_INSTALL_DIR:-$HOME/.local/bin}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
MAN_DIR="$DATA_HOME/man/man1"
BASH_DIR="$DATA_HOME/bash-completion/completions"
ZSH_DIR="$DATA_HOME/zsh/site-functions"
FISH_DIR="$CONFIG_HOME/fish/completions"

if [ -t 1 ]; then
    bold=$(printf '\033[1m')
    red=$(printf '\033[31m')
    reset=$(printf '\033[0m')
else
    bold=""
    red=""
    reset=""
fi

say() { printf '%s\n' "$*"; }
step() { printf '%s\n' "${bold}$*${reset}"; }
fail() {
    printf '%s\n' "${red}hatchzip: $*${reset}" >&2
    exit 1
}
has() { command -v "$1" >/dev/null 2>&1; }

download() {
    case "$1" in
        # Only for testing this script against a local build.
        file://*) cp "${1#file://}" "$2" ;;
        *)
            if has curl; then
                curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"
            elif has wget; then
                wget -q --https-only -O "$2" "$1"
            else
                fail "needs curl or wget to download"
            fi
            ;;
    esac
}

sha256() {
    if has sha256sum; then
        sha256sum "$1" | cut -d' ' -f1
    elif has shasum; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        fail "needs sha256sum or shasum to check the download"
    fi
}

uninstall() {
    step "Removing HatchZip"
    rm -f "$BIN_DIR/hatchzip" "$MAN_DIR/hatchzip.1" \
        "$BASH_DIR/hatchzip" "$ZSH_DIR/_hatchzip" "$FISH_DIR/hatchzip.fish"
    # Only our own hz: a symlink pointing at hatchzip.
    if [ "$(readlink "$BIN_DIR/hz" 2>/dev/null)" = hatchzip ]; then
        rm -f "$BIN_DIR/hz" "$MAN_DIR/hz.1" "$BASH_DIR/hz" "$ZSH_DIR/_hz" "$FISH_DIR/hz.fish"
    fi
    say "Removed. Half-finished extractions keep their .hatchzip files next to the archive."
}

install_hatchzip() {
    case "$(uname -s)" in
        Linux) os=linux ;;
        Darwin) os=macos ;;
        *) fail "this installer is for Linux and macOS; on Windows use install.ps1" ;;
    esac
    case "$(uname -m)" in
        x86_64 | amd64) arch=x86_64 ;;
        aarch64 | arm64) arch=aarch64 ;;
        *) fail "no build for $(uname -m) yet; build from source with cargo instead" ;;
    esac
    # An Intel shell on an Apple Silicon Mac (Rosetta) still wants the native build.
    if [ "$os" = macos ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
        arch=aarch64
    fi

    name="hatchzip-$arch-$os"
    if [ -n "${HATCHZIP_BASE_URL:-}" ]; then
        base="$HATCHZIP_BASE_URL"
    elif [ -n "${HATCHZIP_VERSION:-}" ]; then
        base="https://github.com/$REPO/releases/download/$HATCHZIP_VERSION"
    else
        base="https://github.com/$REPO/releases/latest/download"
    fi

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT

    step "Downloading HatchZip for $os $arch"
    download "$base/$name.tar.gz" "$tmp/$name.tar.gz"
    download "$base/$name.tar.gz.sha256" "$tmp/$name.tar.gz.sha256"
    expected=$(cut -d' ' -f1 "$tmp/$name.tar.gz.sha256")
    actual=$(sha256 "$tmp/$name.tar.gz")
    [ "$expected" = "$actual" ] || fail "the download does not match its checksum; nothing was installed"

    tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
    src="$tmp/$name"

    step "Installing to $BIN_DIR"
    mkdir -p "$BIN_DIR" "$MAN_DIR" "$BASH_DIR" "$ZSH_DIR" "$FISH_DIR"
    # Replaced through a rename, so a copy that is running keeps working.
    cp "$src/hatchzip" "$BIN_DIR/.hatchzip.new"
    chmod 755 "$BIN_DIR/.hatchzip.new"
    mv -f "$BIN_DIR/.hatchzip.new" "$BIN_DIR/hatchzip"
    cp "$src/man/hatchzip.1" "$MAN_DIR/hatchzip.1"
    cp "$src/completions/hatchzip.bash" "$BASH_DIR/hatchzip"
    cp "$src/completions/_hatchzip" "$ZSH_DIR/_hatchzip"
    cp "$src/completions/hatchzip.fish" "$FISH_DIR/hatchzip.fish"

    # `hz` is the short name, unless some other program already answers to it.
    other_hz=$(command -v hz 2>/dev/null || true)
    if [ -n "$other_hz" ] && [ "$other_hz" != "$BIN_DIR/hz" ]; then
        say "Skipping the short name hz: $other_hz already uses it."
    else
        ln -sf hatchzip "$BIN_DIR/hz"
        ln -sf hatchzip.1 "$MAN_DIR/hz.1"
        "$BIN_DIR/hz" completions bash > "$BASH_DIR/hz"
        "$BIN_DIR/hz" completions zsh > "$ZSH_DIR/_hz"
        "$BIN_DIR/hz" completions fish > "$FISH_DIR/hz.fish"
    fi

    say ""
    say "$("$BIN_DIR/hatchzip" --version) installed. Run it as hatchzip or hz; man hatchzip has the manual."
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *)
            say ""
            say "$BIN_DIR is not on your PATH yet. Add this line to your shell's startup file:"
            say "  export PATH=\"$BIN_DIR:\$PATH\""
            ;;
    esac
    say ""
    say "Try: hatchzip check some-archive.zip"
}

# Nothing above has done anything yet. Piped from curl, the shell runs the
# script as it arrives, so a download cut off halfway stops before this line.
if [ "${1:-}" = "--uninstall" ]; then
    uninstall
else
    install_hatchzip
fi
