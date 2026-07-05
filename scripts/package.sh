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

# Map the release target to a Zig target triple + the platform's target dir
# (used to build the HTTP platform host from source, so it's never stale).
case "$TARGET" in
  macos-arm64) ZIG_TARGET="aarch64-macos"; PLAT_DIR="arm64mac" ;;
  linux-x64)   ZIG_TARGET="x86_64-linux-musl"; PLAT_DIR="x64musl" ;;
  *) echo "no platform-host mapping for $TARGET" >&2; exit 1 ;;
esac

echo ">> building Rust binaries (release)"
cargo build --release --bin claw --bin claw-mcp --bin claw-lsp

# --- the bundled model + inference server ---------------------------------
# model/claw-0.5b-q8.gguf (quantized fine-tune) and a llama.cpp server
# binary must exist before packaging — the bundle ships AI batteries.
MODEL_FILE="${CLAW_MODEL_FILE:-$ROOT/model/claw-0.5b-q8.gguf}"
INFER_BIN="${CLAW_INFER_BIN:-$ROOT/model/claw-infer}"
[ -f "$MODEL_FILE" ] || { echo "missing model: $MODEL_FILE (set CLAW_MODEL_FILE)" >&2; exit 1; }
[ -f "$INFER_BIN" ] || { echo "missing inference server: $INFER_BIN (set CLAW_INFER_BIN)" >&2; exit 1; }

echo ">> building the compiler (clawc + snapshot)"
( cd compiler && zig build roc -Doptimize=ReleaseFast && zig build build-snapshot-tool -Doptimize=ReleaseFast )

# --- assemble --------------------------------------------------------------
STAGE="$(mktemp -d)"; trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/bin"
mkdir -p "$STAGE/model"
cp "$MODEL_FILE" "$STAGE/model/claw-0.5b-q8.gguf"
cp "$INFER_BIN" "$STAGE/bin/claw-infer"
cp target/release/claw "$STAGE/bin/"
cp target/release/claw-mcp "$STAGE/bin/"
cp target/release/claw-lsp "$STAGE/bin/"
cp compiler/zig-out/bin/clawc "$STAGE/bin/"
cp compiler/zig-out/bin/snapshot "$STAGE/bin/"
chmod +x "$STAGE/bin/"*

# Bundled platforms (for `claw new --platform http|cli`). The HTTP host is
# built from source for this target so the tarball always has a working host
# (the prebuilt .a files are gitignored / may be stale or absent in CI).
echo ">> bundling http platform (building host for $ZIG_TARGET)"
mkdir -p "$STAGE/platforms"
cp -R compiler/test/http-headers/platform "$STAGE/platforms/http"
mkdir -p "$STAGE/platforms/http/targets/$PLAT_DIR"
( cd compiler/test/http-headers/platform \
  && zig build-lib host.zig -target "$ZIG_TARGET" -O ReleaseSmall \
       -femit-bin="$STAGE/platforms/http/targets/$PLAT_DIR/libhost.a" )

# The cli (stdin/stdout) platform ships only if its prebuilt host exists for
# this target (its host isn't a single self-contained file).
if [ -f "compiler/test/fx-open/platform/targets/$PLAT_DIR/libhost.a" ]; then
  echo ">> bundling cli platform"
  cp -R compiler/test/fx-open/platform "$STAGE/platforms/cli"
else
  echo ">> skipping cli platform (no prebuilt host for $PLAT_DIR)"
fi

mkdir -p "$ROOT/dist"
OUT="$ROOT/dist/claw-$VERSION-$TARGET.tar.gz"
tar -czf "$OUT" -C "$STAGE" bin platforms
echo ">> wrote $OUT"
tar -tzf "$OUT" | head
