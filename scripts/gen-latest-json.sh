#!/usr/bin/env bash
# gen-latest-json.sh <version> <out-path> <bundle-dir> — emit the tauri-updater manifest (latest.json).
#
# GENERIC — shared verbatim across every consuming app via shell-core; app-specific values come
# from the tracked per-app scripts/tooling.env. Materialized git-ignored by the app's build.rs from
# the pinned shell-core rev — edit it in shell-core, never in the consuming app.
#
# Reads the bundler's signed updater artifact (${APP_NAME}.app.tar.gz.sig) and writes a manifest the
# updater fetches from https://github.com/${UPDATER_REPO}/releases/latest/download/latest.json. The
# `.sig` only exists when the release build ran with the updater signing env set
# (TAURI_SIGNING_PRIVATE_KEY[_PASSWORD]); without it this errors rather than emit an unsigned
# manifest.
#
# The release artifact is a universal binary, so BOTH macOS platform keys — darwin-aarch64 and
# darwin-x86_64 — point at that one tarball. Both entries are load-bearing: the updater matches on
# the running install's own arch and finds nothing if its key is absent, which surfaces as auto-update
# silently going quiet (no error, no bar) for every user on the missing architecture.
#
# <bundle-dir> is passed by release.sh rather than recomputed here — it moves with the build target,
# and a second copy of that path would silently read a stale host-arch bundle from an earlier build.
set -euo pipefail
source "$(dirname "$0")/tooling.env"
cd "$(dirname "$0")/.."

: "${APP_NAME:?tooling.env must set APP_NAME}"
: "${UPDATER_REPO:?tooling.env must set UPDATER_REPO}"

VERSION="${1:?usage: gen-latest-json.sh <version> <out-path> <bundle-dir>}"
OUT="${2:?usage: gen-latest-json.sh <version> <out-path> <bundle-dir>}"
BUNDLE_DIR="${3:?usage: gen-latest-json.sh <version> <out-path> <bundle-dir>}"

SIG_FILE="${BUNDLE_DIR}/${APP_NAME}.app.tar.gz.sig"
if [ ! -f "$SIG_FILE" ]; then
  echo "gen-latest-json: no updater signature at $SIG_FILE" >&2
  echo "  → build with TAURI_SIGNING_PRIVATE_KEY[_PASSWORD] set first." >&2
  exit 1
fi

SIG="$(cat "$SIG_FILE")"
URL="https://github.com/${UPDATER_REPO}/releases/download/v${VERSION}/${APP_NAME}.app.tar.gz"

cat > "$OUT" <<JSON
{
  "version": "${VERSION}",
  "notes": "See the release notes at https://github.com/${UPDATER_REPO}/releases/tag/v${VERSION}",
  "platforms": {
    "darwin-aarch64": {
      "signature": "${SIG}",
      "url": "${URL}"
    },
    "darwin-x86_64": {
      "signature": "${SIG}",
      "url": "${URL}"
    }
  }
}
JSON

echo "gen-latest-json: wrote $OUT (v${VERSION}, darwin-aarch64 + darwin-x86_64 → universal ${APP_NAME}.app.tar.gz)"
