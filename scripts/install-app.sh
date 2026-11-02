#!/usr/bin/env bash
# install-app.sh <built-app> — place a freshly built ${APP_NAME}.app into /Applications (override
# with APP_DEST), quitting any running copy first, stripping the quarantine xattr so an unsigned
# local build isn't blocked by Gatekeeper, re-signing it with this Mac's Developer ID if there is one,
# then stamping the build commit on it. Does NOT relaunch —
# the caller decides.
#
# GENERIC — shared verbatim across every consuming app via shell-core; app-specific values come
# from the tracked per-app scripts/tooling.env. Materialized git-ignored by the app's build.rs from
# the pinned shell-core rev — edit it in shell-core, never in the consuming app.
set -euo pipefail
source "$(dirname "$0")/tooling.env"

: "${APP_NAME:?tooling.env must set APP_NAME}"

src="${1:?usage: install-app.sh <built ${APP_NAME}.app>}"
dest="${APP_DEST:-/Applications/${APP_NAME}.app}"

[ -d "$src" ] || { echo "install-app.sh: no app bundle at $src" >&2; exit 1; }
[[ "$dest" == *.app ]] || { echo "install-app.sh: refusing — dest is not an .app bundle: $dest" >&2; exit 1; }

# Gone means gone to LaunchServices too, which drops an app some 0.2s after its process exits. An
# `open` inside that window is routed to the dead instance and fails with -600 (procNotFound).
gone() {
    ! pgrep -f "${dest}/" >/dev/null 2>&1 && [ -z "$(lsappinfo find bundlepath="$dest" 2>/dev/null)" ]
}

osascript -e "quit app \"${APP_NAME}\"" 2>/dev/null || true
# The app flushes its window geometry on exit; give the quit up to ~5s to finish that before a
# kill, which would lose or truncate it.
for _ in $(seq 1 50); do
    gone && break
    sleep 0.1
done
pkill -f "${dest}/" 2>/dev/null || true
for _ in $(seq 1 30); do
    gone && break
    sleep 0.1
done

rm -rf "$dest"
mkdir -p "$(dirname "$dest")"
cp -R "$src" "$dest"
xattr -dr com.apple.quarantine "$dest" 2>/dev/null || true

# Re-sign with this Mac's Developer ID when it has one. The build's linker ad-hoc signature has a
# cdhash for its designated requirement, so every rebuild is a new app to TCC (folder grants) and to
# keychain item ACLs — each install re-prompts for both. A certificate-backed signature's DR is
# bundle id + team, stable across builds and identical to the release build's. No identity, or a
# failed sign (locked keychain) → the ad-hoc build installs as before.
identity=$(security find-identity -v -p codesigning 2>/dev/null |
    awk '/"Developer ID Application: /{print $2; exit}')
if [ -n "$identity" ]; then
    if codesign --force --deep --sign "$identity" "$dest" 2>/dev/null; then
        echo "install-app.sh: signed with Developer ID ($identity)"
    else
        echo "install-app.sh: Developer ID signing failed — installed ad-hoc, grants won't persist" >&2
    fi
fi

# Stamp the commit this build came from ("-dirty" with uncommitted changes), so a release cut later
# can tell whether it is newer than this build before replacing it. An xattr, not an Info.plist
# key: it sits outside the code seal, so the signature stays valid. Not a checkout → no stamp.
repo="$(dirname "$0")/.."
if commit=$(git -C "$repo" rev-parse HEAD 2>/dev/null); then
    [ -z "$(git -C "$repo" status --porcelain)" ] || commit+=-dirty
    xattr -w com.lockyc.build-commit "$commit" "$dest"
fi

echo "install-app.sh: installed → $dest"
