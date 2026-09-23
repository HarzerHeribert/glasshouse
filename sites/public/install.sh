#!/bin/sh
# Install Pane, the inference gateway and Glasshouse from a GitHub release.
#
#   curl -fsSL https://harzerheribert.github.io/glasshouse/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --pane-only      # no `glasshouse` link
#   GLASSHOUSE_VERSION=v0.1.0-pre.2 sh install.sh         # a specific release
#
# What it does, in order, and nothing else:
#   1. picks the release (the newest, pre-releases included, or
#      $GLASSHOUSE_VERSION) and this machine's archive;
#   2. refuses the archive unless its SHA-256 matches the release's SHA256SUMS;
#   3. unpacks it into ~/.local/lib/glasshouse/versions/<tag>/bin -- a fresh
#      directory, never over a binary that may be running -- and points
#      ~/.local/lib/glasshouse/current at it;
#   4. links pane, inference-gateway (and glasshouse) into ~/.local/bin;
#   5. downloads the CLIProxyAPI build the release pins (cliproxyapi.toml),
#      refuses it unless its SHA-256 matches the pin, and hands it to
#      `inference-gateway subscriptions adopt-binary`.
# It installs no harness, touches no credential and edits no shell profile.
set -eu

REPO="${GLASSHOUSE_REPO:-HarzerHeribert/glasshouse}"
# Test seams: where releases are listed and downloaded from.
API="${GLASSHOUSE_RELEASES_API:-https://api.github.com/repos/$REPO/releases?per_page=1}"
DOWNLOADS="${GLASSHOUSE_RELEASE_DOWNLOADS:-https://github.com/$REPO/releases/download}"
BROKER_DOWNLOADS="${GLASSHOUSE_BROKER_DOWNLOADS:-}"
ROOT="${GLASSHOUSE_HOME:-$HOME/.local/lib/glasshouse}"
BIN_DIR="${GLASSHOUSE_BIN_DIR:-$HOME/.local/bin}"
PANE_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --pane-only) PANE_ONLY=1 ;;
    *) echo "install.sh: unknown option $arg" >&2; exit 2 ;;
  esac
done

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

fetch() { # url dest
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    die "neither curl nor wget is installed"
  fi
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) TARGET=aarch64-apple-darwin ;;
  Linux-x86_64) TARGET=x86_64-unknown-linux-gnu ;;
  Linux-aarch64 | Linux-arm64) TARGET=aarch64-unknown-linux-gnu ;;
  *) die "no release is built for $(uname -s) $(uname -m); on Windows use install.ps1" ;;
esac

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

TAG="${GLASSHOUSE_VERSION:-}"
if [ -z "$TAG" ]; then
  fetch "$API" "$TMP/releases.json"
  TAG="$(sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' "$TMP/releases.json" | head -n1)"
  [ -n "$TAG" ] || die "could not read the newest release of $REPO"
fi
VERSION="${TAG#v}"
ARCHIVE="glasshouse-$VERSION-$TARGET.tar.gz"
BASE="$DOWNLOADS/$TAG"

say "Installing $TAG for $TARGET"
fetch "$BASE/$ARCHIVE" "$TMP/$ARCHIVE"
fetch "$BASE/SHA256SUMS" "$TMP/SHA256SUMS"
WANT="$(grep " $ARCHIVE\$" "$TMP/SHA256SUMS" | cut -d' ' -f1)"
[ -n "$WANT" ] || die "$ARCHIVE is not listed in the release's SHA256SUMS"
[ "$(sha256_of "$TMP/$ARCHIVE")" = "$WANT" ] || die "$ARCHIVE does not match its SHA-256; refusing it"

DEST="$ROOT/versions/$TAG"
if [ -x "$DEST/bin/pane" ]; then
  say "$TAG is already installed at $DEST"
else
  tar xzf "$TMP/$ARCHIVE" -C "$TMP"
  STAGE="$TMP/glasshouse-$VERSION-$TARGET"
  mkdir -p "$ROOT/versions" "$DEST.partial/bin"
  for b in pane inference-gateway glasshouse; do
    [ -f "$STAGE/$b" ] && cp "$STAGE/$b" "$DEST.partial/bin/$b"
  done
  [ -f "$STAGE/cliproxyapi.toml" ] && cp "$STAGE/cliproxyapi.toml" "$DEST.partial/"
  [ -x "$DEST.partial/bin/pane" ] || die "the archive carried no pane binary"
  mv "$DEST.partial" "$DEST"
fi
ln -sfn "$DEST" "$ROOT/current"

mkdir -p "$BIN_DIR"
for b in pane inference-gateway glasshouse; do
  [ "$b" = glasshouse ] && [ "$PANE_ONLY" = 1 ] && continue
  [ -x "$ROOT/current/bin/$b" ] && ln -sfn "$ROOT/current/bin/$b" "$BIN_DIR/$b"
done

# The subscription broker the release was built with.
PIN="$DEST/cliproxyapi.toml"
if [ -f "$PIN" ]; then
  BROKER_REPO="$(sed -n 's/^repository = "\(.*\)"/\1/p' "$PIN")"
  BROKER_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$PIN")"
  LINE="$(grep "^$TARGET = " "$PIN" || true)"
  NAME="$(printf '%s' "$LINE" | sed -n 's/.*name = "\([^"]*\)".*/\1/p')"
  SUM="$(printf '%s' "$LINE" | sed -n 's/.*sha256 = "\([^"]*\)".*/\1/p')"
  if [ -n "$NAME" ] && [ -n "$SUM" ] && [ "$(cat "$ROOT/broker-version" 2>/dev/null)" != "$BROKER_VERSION" ]; then
    fetch "${BROKER_DOWNLOADS:-https://github.com/$BROKER_REPO/releases/download}/v$BROKER_VERSION/$NAME" "$TMP/$NAME"
    [ "$(sha256_of "$TMP/$NAME")" = "$SUM" ] || die "$NAME does not match the SHA-256 the release pins; refusing it"
    mkdir -p "$TMP/broker"
    tar xzf "$TMP/$NAME" -C "$TMP/broker"
    "$DEST/bin/inference-gateway" subscriptions adopt-binary "$TMP/broker/cli-proxy-api" >/dev/null
    printf '%s' "$BROKER_VERSION" > "$ROOT/broker-version"
    say "Subscription broker: CLIProxyAPI $BROKER_VERSION"
  fi
fi

say "Installed $TAG. Pane updates itself from here on; run \`pane\` to start, \`pane doctor\` to check."
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) say "Add $BIN_DIR to your PATH to run pane from any shell." ;;
esac
