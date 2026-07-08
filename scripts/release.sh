#!/usr/bin/env bash
# Build, notarize, zip, and attach the app bundle to its GitHub release.
#
# GENERIC — shared verbatim across every consuming app via shell-core. Every app-specific value is
# read from the tracked per-app scripts/tooling.env (APP_NAME, TAURI_CRATE_DIR, UPDATER_REPO); do
# NOT hardcode an app name here. This file is materialized git-ignored by the app's build.rs from
# the pinned shell-core rev — edit it in shell-core, never in the consuming app.
#
# The version is single-sourced in ${TAURI_CRATE_DIR}/Cargo.toml — tauri.conf.json has no `version`
# key, so the bundle inherits it. Run this AFTER the release commit is tagged and pushed and
# `gh release create v<version>` has published the notes (see CLAUDE.md › Releases); this script
# only builds + attaches the macOS artifact.
set -euo pipefail
source "$(dirname "$0")/tooling.env"
cd "$(dirname "$0")/.."

: "${APP_NAME:?tooling.env must set APP_NAME}"
: "${TAURI_CRATE_DIR:?tooling.env must set TAURI_CRATE_DIR}"
: "${UPDATER_REPO:?tooling.env must set UPDATER_REPO}"
VERSION_FILE="${TAURI_CRATE_DIR}/Cargo.toml"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$VERSION_FILE" | head -1)"
[ -n "$VERSION" ] || { echo "release: could not read version from $VERSION_FILE" >&2; exit 1; }
TAG="v$VERSION"
ZIP="${APP_NAME}-${VERSION}-macos.zip"
APP="target/release/bundle/macos/${APP_NAME}.app"

# The artifact must match the tag: build only from a clean tree whose HEAD *is* the tag. Otherwise
# the notarized zip attached to $TAG could silently contain uncommitted or post-tag code — the one
# thing a release artifact must never do (it's what everyone downloads as "v$VERSION").
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "release: working tree is dirty — commit or stash before building $TAG." >&2
  exit 1
fi
if ! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  echo "release: tag $TAG does not exist — tag the release commit first (see CLAUDE.md › Releases)." >&2
  exit 1
fi
if [ "$(git rev-parse "$TAG^{commit}")" != "$(git rev-parse HEAD)" ]; then
  echo "release: HEAD is not $TAG — check out the tagged commit before building the release artifact." >&2
  exit 1
fi

# A release artifact MUST be signed + notarized — an unsigned zip is Gatekeeper-blocked on
# other Macs, so refuse rather than ship one that looks official but won't open. (Contributors
# building for local use go through `just build`/`just deploy`, which tolerate unsigned.)
if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
  echo "release: APPLE_SIGNING_IDENTITY is unset — the build would be unsigned/un-notarized." >&2
  echo "         Set APPLE_SIGNING_IDENTITY + APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID" >&2
  echo "         (or APPLE_API_KEY/APPLE_API_ISSUER/APPLE_API_KEY_PATH) before releasing." >&2
  exit 1
fi

# The updater key signs the .app.tar.gz that existing installs download + verify. Without it the
# createUpdaterArtifacts build produces no .sig and latest.json can't be formed — so refuse rather
# than publish a release existing users can't auto-update to.
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  echo "release: TAURI_SIGNING_PRIVATE_KEY is unset — no updater signature would be produced." >&2
  echo "         Set TAURI_SIGNING_PRIVATE_KEY (+ _PASSWORD) from the ${APP_NAME} updater key before releasing." >&2
  exit 1
fi

# The release must exist (notes published) before we attach to it.
if ! gh release view "$TAG" >/dev/null 2>&1; then
  echo "release: GitHub release $TAG not found — run 'gh release create $TAG' first." >&2
  exit 1
fi

echo "→ building + notarizing ${APP_NAME} $VERSION (cargo tauri build) …"
# createUpdaterArtifacts is enabled here via --config, NOT in the committed tauri.conf.json: baking
# it in makes every `cargo tauri build` demand TAURI_SIGNING_PRIVATE_KEY, which breaks keyless
# from-source builds (install.sh / just build / just deploy). Release-only, so those stay keyless.
( cd "$TAURI_CRATE_DIR" && cargo tauri build --config '{"bundle":{"createUpdaterArtifacts":true}}' )
[ -d "$APP" ] || { echo "release: bundle not found at $APP" >&2; exit 1; }

echo "→ zipping $APP → $ZIP (ditto, preserves the stapled notarization ticket)"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"

echo "→ uploading $ZIP to release $TAG"
gh release upload "$TAG" "$ZIP" --clobber

# Updater artifacts: the signed .app.tar.gz (+ .sig) existing installs download, and the manifest
# the updater fetches from the releases/latest/download/ alias. createUpdaterArtifacts + the signing
# env above produce the tarball + .sig during the build.
TARBALL="target/release/bundle/macos/${APP_NAME}.app.tar.gz"
[ -f "$TARBALL" ] && [ -f "$TARBALL.sig" ] || {
  echo "release: updater artifacts missing at ${TARBALL} (+ .sig) — is createUpdaterArtifacts on + the signing env set?" >&2
  exit 1
}
echo "→ generating latest.json + uploading updater artifacts to $TAG"
bash scripts/gen-latest-json.sh "$VERSION" latest.json
gh release upload "$TAG" "$TARBALL" "$TARBALL.sig" latest.json --clobber

echo "✓ attached $ZIP + updater artifacts (${APP_NAME}.app.tar.gz, .sig, latest.json) to $TAG"
