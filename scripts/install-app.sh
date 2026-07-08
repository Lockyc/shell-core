#!/usr/bin/env bash
# install-app.sh <built-app> — place a freshly built ${APP_NAME}.app into /Applications (override
# with APP_DEST), quitting any running copy first and stripping the quarantine xattr so an unsigned
# local build isn't blocked by Gatekeeper. Does NOT relaunch — the caller decides.
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

osascript -e "quit app \"${APP_NAME}\"" 2>/dev/null || true
pkill -f "${dest}/" 2>/dev/null || true
sleep 1

rm -rf "$dest"
mkdir -p "$(dirname "$dest")"
cp -R "$src" "$dest"
xattr -dr com.apple.quarantine "$dest" 2>/dev/null || true

echo "install-app.sh: installed → $dest"
