#!/bin/sh
# Install, update, or uninstall i3-editor on macOS or Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/afi3/i3-editor/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/afi3/i3-editor/main/install.sh | sh -s -- --update
#   curl -fsSL https://raw.githubusercontent.com/afi3/i3-editor/main/install.sh | sh -s -- --uninstall
#
# Options:
#   --prefix <dir>   install directory (default: ~/.local/bin)
#   --version <ver>  release tag without the leading "v" (default: latest)
#   --update         replace an existing install with the latest (or pinned) release
#   --uninstall      remove the binary; configuration is left in place
#   --help
#
# Environment overrides: I3_EDITOR_INSTALL_DIR, I3_EDITOR_VERSION, I3_EDITOR_REPO.
# If no release asset exists, install/update fall back to building from source
# with Cargo.

set -eu

REPO="${I3_EDITOR_REPO:-afi3/i3-editor}"
BIN="i3-editor"
PREFIX="${I3_EDITOR_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${I3_EDITOR_VERSION:-}"
MODE="install"

say() { printf '%s\n' "$*" >&2; }
die() { say "error: $*"; exit 1; }

usage() {
    cat <<'EOF'
Install, update, or uninstall i3-editor.

Usage: install.sh [--prefix DIR] [--version VER] [--update | --uninstall] [--help]

  --prefix DIR    install into DIR (default: ~/.local/bin)
  --version VER   release version without the leading "v" (default: latest)
  --update        replace the installed binary with the latest release
  --uninstall     remove the installed binary (configuration is kept)
  --help          show this message

Supported platforms: macOS (Intel and Apple Silicon) and Linux (x86_64 and
aarch64). Without a matching release asset the script builds from source:
  cargo install --git https://github.com/<repo> --locked
which requires a Rust toolchain (https://rustup.rs).
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) [ $# -ge 2 ] || die "--prefix needs a directory"; PREFIX="$2"; shift 2 ;;
        --prefix=*) PREFIX="${1#*=}"; shift ;;
        --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift 2 ;;
        --version=*) VERSION="${1#*=}"; shift ;;
        --update|--upgrade) MODE="update"; shift ;;
        --uninstall) MODE="uninstall"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done

config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/i3-editor"

if [ "$MODE" = "uninstall" ]; then
    target="$PREFIX/$BIN"
    if [ -e "$target" ]; then
        rm -f "$target" || die "cannot remove $target"
        say "removed $target"
    else
        say "$BIN is not installed in $PREFIX"
    fi
    if [ -d "$config_dir" ]; then
        say "configuration kept at $config_dir (delete it manually to remove themes and settings)"
    fi
    exit 0
fi

current_version() {
    if [ -x "$PREFIX/$BIN" ]; then
        "$PREFIX/$BIN" --version 2>/dev/null | awk '{print $2}' | head -n 1
    fi
}

latest_version() {
    if [ -n "${I3_EDITOR_LATEST_VERSION:-}" ]; then
        printf '%s' "$I3_EDITOR_LATEST_VERSION"
        return 0
    fi
    payload=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null) || return 1
    printf '%s' "$payload" \
        | tr ',' '\n' \
        | grep -m 1 '"tag_name"' \
        | sed 's/.*"v\{0,1\}\([^"]*\)".*/\1/'
}

CURRENT=""
if [ "$MODE" = "update" ]; then
    CURRENT=$(current_version || true)
    if [ -n "$VERSION" ]; then
        DESIRED="$VERSION"
    else
        DESIRED=$(latest_version || true)
    fi
    if [ -n "$CURRENT" ] && [ -n "$DESIRED" ] && [ "$CURRENT" = "$DESIRED" ]; then
        say "i3-editor is already up to date (v$CURRENT)"
        exit 0
    fi
    if [ -n "$DESIRED" ]; then
        VERSION="$DESIRED"
    else
        say "could not determine the latest version; installing the latest release"
    fi
    if [ -n "$CURRENT" ]; then
        say "updating i3-editor from v$CURRENT"
    else
        say "i3-editor is not installed in $PREFIX; installing"
    fi
fi

os=$(uname -s 2>/dev/null || echo unknown)
arch=$(uname -m 2>/dev/null || echo unknown)

case "$os" in
    Darwin)
        case "$arch" in
            arm64) target="aarch64-apple-darwin" ;;
            x86_64) target="x86_64-apple-darwin" ;;
            *) die "unsupported macOS architecture: $arch" ;;
        esac
        ;;
    Linux)
        case "$arch" in
            x86_64|amd64) target="x86_64-unknown-linux-gnu" ;;
            aarch64|arm64) target="aarch64-unknown-linux-gnu" ;;
            *) die "unsupported Linux architecture: $arch" ;;
        esac
        ;;
    *) die "unsupported operating system: $os (macOS and Linux only)" ;;
esac

mkdir -p "$PREFIX" || die "cannot create $PREFIX"

installed=""

if command -v curl >/dev/null 2>&1; then
    if [ -n "$VERSION" ]; then
        base="https://github.com/$REPO/releases/download/v$VERSION"
    else
        base="https://github.com/$REPO/releases/latest/download"
    fi
    if [ -n "${I3_EDITOR_BASE_URL:-}" ]; then
        base="$I3_EDITOR_BASE_URL"
    fi
    asset="$BIN-$target.tar.gz"
    tmp=$(mktemp -d 2>/dev/null || mktemp -d -t i3-editor)
    trap 'rm -rf "$tmp"' EXIT INT TERM
    say "fetching $base/$asset"
    if curl -fsSL "$base/$asset" -o "$tmp/$asset"; then
        if curl -fsSL "$base/$asset.sha256" -o "$tmp/$asset.sha256" 2>/dev/null; then
            if command -v sha256sum >/dev/null 2>&1; then
                actual=$(sha256sum "$tmp/$asset" | awk '{print $1}')
            elif command -v shasum >/dev/null 2>&1; then
                actual=$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')
            else
                actual=""
            fi
            expected=$(awk '{print $1}' "$tmp/$asset.sha256")
            if [ -n "$actual" ] && [ "$actual" != "$expected" ]; then
                die "checksum mismatch for $asset"
            fi
        fi
        tar -xzf "$tmp/$asset" -C "$tmp" || die "cannot extract $asset"
        binary=$(find "$tmp" -type f -name "$BIN" | head -n 1)
        [ -n "$binary" ] || die "archive did not contain $BIN"
        cp "$binary" "$PREFIX/$BIN" || die "cannot write $PREFIX/$BIN"
        chmod 755 "$PREFIX/$BIN"
        installed="$PREFIX/$BIN"
    fi
fi

if [ -z "$installed" ]; then
    say "no release asset available for $target"
    command -v cargo >/dev/null 2>&1 || die "install Rust from https://rustup.rs, or download a release manually"
    build_root=$(mktemp -d 2>/dev/null || mktemp -d -t i3-editor-build)
    say "building from source with cargo (this can take a few minutes)"
    cargo install --git "https://github.com/$REPO" --locked --root "$build_root" \
        || die "cargo install failed"
    cp "$build_root/bin/$BIN" "$PREFIX/$BIN" || die "cannot write $PREFIX/$BIN"
    chmod 755 "$PREFIX/$BIN"
    rm -rf "$build_root"
    installed="$PREFIX/$BIN"
fi

"$installed" --version >/dev/null 2>&1 || die "$installed does not run on this machine"
NEW=$("$installed" --version | awk '{print $2}' | head -n 1)

if [ "$MODE" = "update" ] && [ -n "$CURRENT" ]; then
    say "updated i3-editor v$CURRENT -> v$NEW ($installed)"
else
    say "installed $installed (v$NEW)"
fi

case ":$PATH:" in
    *":$PREFIX:"*) ;;
    *)
        say ""
        say "$PREFIX is not on your PATH. Add it with:"
        say "  export PATH=\"$PREFIX:\$PATH\""
        ;;
esac
