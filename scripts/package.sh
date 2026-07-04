#!/bin/sh
# Build and package a Claw release tarball for the current platform.
#
#   scripts/package.sh <version>        # e.g. scripts/package.sh v0.1.0
#
# Produces dist/claw-<version>-<target>.tar.gz with layout:
#   bin/claw  bin/claw-mcp  bin/claw-lsp  bin/clawc  bin/snapshot
#
# Requires: zig 0.16.0, cargo. Run from the repo root.
set -eu

VERSION="${1:?usage: package.sh <version>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# --- target triple ---------------------------------------------------------
os="$(uname -s)"; arch="$(uname -m)"
case "$os" in Darwin) os="macos" ;; Linux) os="linux" ;; *) echo "unsupported OS: $os" >&2; exit 1 ;; esac
case "$arch" in arm64|aarch64) arch="arm64" ;; x86_64|amd64) arch="x64" ;; *) echo "unsupported arch: $arch" >&2; exit 1 ;; esac
TARGET="$os-$arch"

echo ">> building Rust binaries (release)"
cargo build --release --bin claw --bin claw-mcp --bin claw-lsp

echo ">> building the compiler (clawc + snapshot)"
( cd compiler && zig build roc && zig build build-snapshot-tool )

# --- assemble --------------------------------------------------------------
STAGE="$(mktemp -d)"; trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/bin"
cp target/release/claw "$STAGE/bin/"
cp target/release/claw-mcp "$STAGE/bin/"
cp target/release/claw-lsp "$STAGE/bin/"
cp compiler/zig-out/bin/clawc "$STAGE/bin/"
cp compiler/zig-out/bin/snapshot "$STAGE/bin/"
chmod +x "$STAGE/bin/"*

# Bundled platforms (for `claw new --platform http|cli`). Prebuilt hosts are
# macOS-only today; the Linux host is a roadmap item.
echo ">> bundling platforms (http, cli)"
mkdir -p "$STAGE/platforms"
cp -R compiler/test/http-headers/platform "$STAGE/platforms/http"
cp -R compiler/test/fx-open/platform "$STAGE/platforms/cli"

mkdir -p "$ROOT/dist"
OUT="$ROOT/dist/claw-$VERSION-$TARGET.tar.gz"
tar -czf "$OUT" -C "$STAGE" bin platforms
echo ">> wrote $OUT"
tar -tzf "$OUT" | head
