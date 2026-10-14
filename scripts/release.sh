#!/usr/bin/env bash
# release.sh [notes-file] — cut the release for the version in ${TAURI_CRATE_DIR}/Cargo.toml, in one go:
#   refuse unless dev can ship → build + notarize + updater artifacts → tag v<version> →
#   fast-forward main to it → push main + the tag → `gh release create` with the notes and every asset.
#
# GENERIC — shared verbatim across every consuming app via shell-core. Every app-specific value is
# read from the tracked per-app scripts/tooling.env (APP_NAME, TAURI_CRATE_DIR, UPDATER_REPO); do
# NOT hardcode an app name here. This file is materialized git-ignored by the app's build.rs from
# the pinned shell-core rev — edit it in shell-core, never in the consuming app.
#
# Run it as the app's `just release [notes=<file>]`, which runs the gate first. The version is
# single-sourced in ${TAURI_CRATE_DIR}/Cargo.toml — tauri.conf.json has no `version` key, so the
# bundle inherits it — and the tag is exactly v<that version>.
#
# Nothing public happens until the artifacts exist. Every refusal precedes the build and the build
# precedes the tag, so a failed build or notarization leaves no tag, no moved main and no release.
# The assets go up in the same `gh release create` that publishes the notes, so the release is never
# visible without the zip or the updater manifest its `releases/latest/download/latest.json` alias
# serves.
#
# Notes: [notes-file] when given (a path relative to the caller's directory), else the commit
# subjects since the previous tag; either way a release with nothing to say is refused. Not
# `gh --generate-notes`: it summarises merged PRs, and these repos integrate by direct merge to dev,
# so it yields a bare compare link.
#
# DRY_RUN=1 runs every check, then prints the build, tag, push and publish commands (and the notes
# body) instead of running them. A refusal is reported and the dry run carries on, exiting 1.
#
# The release artifact is a UNIVERSAL binary (arm64 + x86_64) — one bundle both architectures run,
# so a release cut on either kind of Mac serves every user and latest.json can point both platform
# keys at the same tarball. This is a release-only target: `just build`/`just install`/install.sh
# stay host-arch, which is all a local install needs and is half the compile.
#
# Every check is an explicit `if …; then refuse; fi`: under `set -e` a bare `[ … ] || refuse` is easy
# to misread, and a refusal must never fall through to the build, tag or publish steps.
set -euo pipefail

# A relative notes path is relative to the caller's directory, so resolve it before the cd below.
NOTES_ARG="${1:-}"
case "$NOTES_ARG" in
  "" | /*) ;;
  *) NOTES_ARG="$PWD/$NOTES_ARG" ;;
esac

source "$(dirname "$0")/tooling.env"
cd "$(dirname "$0")/.."

: "${APP_NAME:?tooling.env must set APP_NAME}"
: "${TAURI_CRATE_DIR:?tooling.env must set TAURI_CRATE_DIR}"
: "${UPDATER_REPO:?tooling.env must set UPDATER_REPO}"
VERSION_FILE="${TAURI_CRATE_DIR}/Cargo.toml"
DRY_RUN="${DRY_RUN:-}"

refused=0
refuse() {
  echo "release: $*" >&2
  if [ -n "$DRY_RUN" ]; then
    refused=1
  else
    exit 1
  fi
}

# run <cmd…> — a side effect: executed, or under DRY_RUN printed as `+ cmd`.
run() {
  if [ -n "$DRY_RUN" ]; then
    printf '+'
    printf ' %q' "$@"
    printf '\n'
  else
    "$@"
  fi
}

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$VERSION_FILE" | head -1)"
if [ -z "$VERSION" ]; then
  echo "release: could not read version from $VERSION_FILE" >&2
  exit 1
fi
TAG="v$VERSION"
ZIP="${APP_NAME}-${VERSION}-macos.zip"
# Single-sourced here and passed to gen-latest-json.sh — the bundle dir moves with TARGET, so a
# second copy of this path would silently read the stale host-arch bundle from an earlier build.
TARGET="universal-apple-darwin"
BUNDLE_DIR="target/${TARGET}/release/bundle/macos"
APP="${BUNDLE_DIR}/${APP_NAME}.app"
TARBALL="${BUNDLE_DIR}/${APP_NAME}.app.tar.gz"

# ── refusals: all of them, before the first side effect ──

# dev is what ships, and the gate just tested this checkout — so they must be one tree, clean (no
# untracked files either: the artifact everyone downloads as $TAG must contain only committed code),
# and identical to origin/dev so the tag names a commit the world can see.
if [ "$(git symbolic-ref --short -q HEAD || true)" != dev ]; then
  refuse "not on dev — release from the dev checkout (dev is what ships)."
fi
if [ -n "$(git status --porcelain)" ]; then
  refuse "working tree is dirty — commit the version bump first."
fi
git fetch -q --tags origin
if [ "$(git rev-parse HEAD)" != "$(git rev-parse origin/dev)" ]; then
  refuse "dev is not in sync with origin/dev — push (or pull) first."
fi
# main advances only by fast-forward to the release commit. Check both copies now, so a diverged
# main refuses here rather than after the tag exists.
if ! git merge-base --is-ancestor origin/main HEAD; then
  refuse "origin/main is not an ancestor of dev — git merge origin/main on dev, then re-run."
fi
if git rev-parse -q --verify refs/heads/main >/dev/null && ! git merge-base --is-ancestor main HEAD; then
  refuse "local main is not an ancestor of dev — reconcile it with origin/main, then re-run."
fi
if git worktree list --porcelain | grep -x 'branch refs/heads/main' >/dev/null; then
  refuse "main is checked out in a worktree — git cannot fast-forward a checked-out branch from here."
fi
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  refuse "tag $TAG already exists — bump the version in $VERSION_FILE first."
fi
if [ -n "$NOTES_ARG" ] && [ ! -s "$NOTES_ARG" ]; then
  refuse "notes file is missing or empty: $NOTES_ARG"
fi

# A release artifact MUST be signed + notarized — an unsigned zip is Gatekeeper-blocked on other
# Macs, so refuse rather than ship one that looks official but won't open. (Contributors building
# for local use go through `just build`/`just install`, which tolerate unsigned.)
if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
  refuse "APPLE_SIGNING_IDENTITY is unset — the build would be unsigned/un-notarized.
         Set APPLE_SIGNING_IDENTITY + APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID
         (or APPLE_API_KEY/APPLE_API_ISSUER/APPLE_API_KEY_PATH) before releasing."
fi
# The updater key signs the .app.tar.gz that existing installs download + verify. Without it the
# createUpdaterArtifacts build produces no .sig and latest.json can't be formed — so refuse rather
# than publish a release existing users can't auto-update to.
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  refuse "TAURI_SIGNING_PRIVATE_KEY is unset — no updater signature would be produced.
         Set TAURI_SIGNING_PRIVATE_KEY (+ _PASSWORD) from the ${APP_NAME} updater key before releasing."
fi
# The publish step runs after main and the tag are pushed; an unauthenticated gh there would strand
# a public tag with no release.
if ! gh auth status >/dev/null 2>&1; then
  refuse "gh is not authenticated — gh auth login first."
fi

# The notes body, resolved now so an empty release refuses before the build. The previous tag is
# looked up before this release's own tag exists. On a failure after tagging the body is kept, so
# the printed finishing command still has its --notes-file.
BODY="$(mktemp)"
next=""
on_exit() {
  if [ -n "$next" ]; then
    echo "release: failed after tagging $TAG — finish by hand: $next" >&2
  else
    rm -f "$BODY"
  fi
}
trap on_exit EXIT

PREV="$(git describe --tags --abbrev=0 --match 'v*' HEAD 2>/dev/null || true)"
if [ -n "$NOTES_ARG" ]; then
  if [ -s "$NOTES_ARG" ]; then cat "$NOTES_ARG" >"$BODY"; fi
elif [ -n "$PREV" ]; then
  git log --no-merges --pretty='- %s' "${PREV}..HEAD" >"$BODY"
else
  echo "- initial release" >"$BODY"
fi
# Check emptiness BEFORE appending the compare link, which would otherwise count as content.
if [ ! -s "$BODY" ]; then
  refuse "nothing to release: no commits since ${PREV} and no notes file."
fi
if [ -n "$PREV" ]; then
  printf '\n**Full Changelog**: https://github.com/%s/compare/%s...%s\n' "$UPDATER_REPO" "$PREV" "$TAG" >>"$BODY"
fi

if [ -n "$DRY_RUN" ]; then
  echo "── dry run: ${APP_NAME} $TAG from $(git rev-parse --short HEAD), notes: ──"
  sed 's/^/  │ /' "$BODY"
fi

# ── build: notarized bundle + signed updater artifacts. Nothing public yet. ──

echo "→ building + notarizing ${APP_NAME} $VERSION (cargo tauri build --target $TARGET) …"
# createUpdaterArtifacts is enabled here via --config, NOT in the committed tauri.conf.json: baking
# it in makes every `cargo tauri build` demand TAURI_SIGNING_PRIVATE_KEY, which breaks keyless
# from-source builds (install.sh / just build / just install). Release-only, so those stay keyless.
if [ -n "$DRY_RUN" ]; then
  echo "+ (cd $TAURI_CRATE_DIR && cargo tauri build --target $TARGET --config '{\"bundle\":{\"createUpdaterArtifacts\":true}}')"
else
  (cd "$TAURI_CRATE_DIR" && cargo tauri build --target "$TARGET" --config '{"bundle":{"createUpdaterArtifacts":true}}')
  if [ ! -d "$APP" ]; then
    echo "release: bundle not found at $APP" >&2
    exit 1
  fi
  if [ ! -f "$TARBALL" ] || [ ! -f "$TARBALL.sig" ]; then
    echo "release: updater artifacts missing at ${TARBALL} (+ .sig) — is createUpdaterArtifacts on + the signing env set?" >&2
    exit 1
  fi
fi

echo "→ zipping $APP → $ZIP (ditto, preserves the stapled notarization ticket)"
run rm -f "$ZIP"
run ditto -c -k --keepParent "$APP" "$ZIP"
# latest.json points the updater at releases/download/$TAG/<tarball> — the asset this same publish
# step uploads, under the tag pushed just before it.
echo "→ generating latest.json"
run bash scripts/gen-latest-json.sh "$VERSION" latest.json "$BUNDLE_DIR"

# ── publish: tag → fast-forward main → push both atomically → release with every asset ──

PUBLISH=(gh release create "$TAG" "$ZIP" "$TARBALL" "$TARBALL.sig" latest.json
  --verify-tag --latest --title "$TAG" --notes-file "$BODY")

echo "→ tagging $TAG and fast-forwarding main to it"
run git tag -a "$TAG" -m "$TAG" HEAD
if [ -z "$DRY_RUN" ]; then next="git tag -d $TAG, then re-run the release (nothing was pushed)"; fi
# A refspec without `+` updates main only if it fast-forwards — `git merge --ff-only` without
# checking main out, which would move this shared checkout's branch.
run git fetch -q . dev:main
echo "→ pushing main + $TAG"
# --atomic: both refs land or neither does, so main never moves public without its tag.
run git push --atomic origin main "refs/tags/$TAG"
if [ -z "$DRY_RUN" ]; then next="${PUBLISH[*]}"; fi
echo "→ publishing GitHub release $TAG with $ZIP + updater artifacts"
run "${PUBLISH[@]}"
next=""

if [ -n "$DRY_RUN" ]; then
  if [ "$refused" = 1 ]; then
    echo "✗ dry run: the real release would have refused (see above)." >&2
    exit 1
  fi
  echo "✓ dry run: every check passed; nothing was built, tagged, pushed or published."
else
  echo "✓ released ${APP_NAME} $TAG: $ZIP + updater artifacts (${APP_NAME}.app.tar.gz, .sig, latest.json)"
fi
