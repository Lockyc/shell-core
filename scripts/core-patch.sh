#!/usr/bin/env bash
# core-patch.sh — the shared-core dev loop: build against a sibling checkout, re-pin, and guard.
#   core-patch.sh dev <core>   activate <core>'s [patch] → build against ../<crate>, edits included
#   core-patch.sh pin <core>   pin <core> to ../<crate>'s pushed HEAD and deactivate its [patch]
#   core-patch.sh pin all      pin every core, in tooling.env order, stopping at the first failure
#   core-patch.sh guard        fail while any [patch] is active in the workspace Cargo.toml
#
# GENERIC — shared verbatim across every consuming app via shell-core; materialized git-ignored by the
# app's build.rs from the pinned shell-core rev — edit it in shell-core, never in the consuming app.
# The app's justfile `<core>-dev` / `<core>-pin` recipes are thin wrappers over it; fleet tooling
# calls those recipes by name, so the recipe names are the stable interface, not this script.
#
# Per-app data is the tracked scripts/tooling.env's CORES: space-separated `<core>:<crate>:<manifest>`
# entries. <core> is the recipe prefix and the `#PATCH:<core>#` marker in the workspace Cargo.toml;
# <crate> is the crate, its github.com/Lockyc repo and the ../<crate> sibling checkout; <manifest>
# is the Cargo.toml (from the repo root) whose git dep line carries the pinned rev, or a comma-separated
# list of them when several of the workspace's crates pin the core (re-pinned together).
#
# The workspace Cargo.toml carries each core's [patch] commented out as `#PATCH:<core>#` lines. An
# active patch builds green locally and is unresolvable for every other clone, so `guard` (the
# gate's first check) blocks it and `pin` both writes the rev and re-comments the patch.
set -euo pipefail
source "$(dirname "$0")/tooling.env"
cd "$(dirname "$0")/.."

: "${CORES:?tooling.env must set CORES (space-separated <core>:<crate>:<manifest> entries)}"
GIT_BASE="https://github.com/Lockyc"

fail() {
  echo "✗ $*" >&2
  exit 1
}

core_keys() {
  local entry
  for entry in $CORES; do printf '%s\n' "${entry%%:*}"; done
}

# resolve <core> — sets CRATE and MANIFEST from its CORES entry.
resolve() {
  local entry key rest
  for entry in $CORES; do
    key="${entry%%:*}"
    rest="${entry#*:}"
    if [ "$key" = "$1" ]; then
      CRATE="${rest%%:*}"
      MANIFEST="${rest#*:}"
      return 0
    fi
  done
  fail "unknown core '$1' — tooling.env CORES has: $(core_keys | tr '\n' ' ')"
}

cmd_dev() {
  resolve "$1"
  local core=$1 tmp
  if [ ! -d "../$CRATE" ]; then fail "../$CRATE not found — ghq get github.com/Lockyc/$CRATE"; fi
  tmp=$(mktemp)
  sed "s/^#PATCH:${core}#//" Cargo.toml >"$tmp" && mv "$tmp" Cargo.toml
  echo "✓ $CRATE → local ../$CRATE (patch active). Iterate, then: just run"
  echo "  ⚠ NEVER commit an active patch — run 'just $core-pin' first ('just gate' will block it)."
}

cmd_pin() {
  resolve "$1"
  local core=$1 cc="../$CRATE" rev tmp
  if [ ! -d "$cc" ]; then fail "$cc not found"; fi
  if [ -n "$(git -C "$cc" status --porcelain)" ]; then fail "$CRATE has uncommitted changes — commit + push it first"; fi
  git -C "$cc" fetch -q origin
  rev=$(git -C "$cc" rev-parse HEAD)
  if ! git -C "$cc" branch -r --contains "$rev" | grep -q origin/; then fail "$CRATE HEAD ($rev) isn't pushed — push it first"; fi
  # Every dep line for the crate in each of its manifests (shell-core has two in an app manifest: the
  # build-dep and the runtime dep).
  local manifest manifests
  IFS=, read -ra manifests <<<"$MANIFEST"
  for manifest in "${manifests[@]}"; do
    tmp=$(mktemp)
    sed -E 's|('"$CRATE"' = \{ git = "'"$GIT_BASE/$CRATE"'", rev = ")[0-9a-f]+|\1'"$rev"'|g' "$manifest" >"$tmp" && mv "$tmp" "$manifest"
    if ! grep -qF "rev = \"$rev\"" "$manifest"; then fail "$core-pin: failed to write rev into $manifest — dep line shape changed, re-pin by hand"; fi
  done
  tmp=$(mktemp)
  sed -E 's|^\[patch\."'"$GIT_BASE/$CRATE"'"\]$|#PATCH:'"$core"'#&|; s|^'"$CRATE"' = \{ path = "\.\./'"$CRATE"'" \}$|#PATCH:'"$core"'#&|' Cargo.toml >"$tmp" && mv "$tmp" Cargo.toml
  if grep -qE '^\[patch\."'"$GIT_BASE/$CRATE"'"\]$' Cargo.toml; then fail "$core-pin: $CRATE [patch] still active after re-comment — check Cargo.toml"; fi
  # Reconcile Cargo.lock on a patch→git flip: a bare -p cannot match the patched (path) source, a
  # minimal resolve does.
  cargo update -p "$CRATE" || cargo metadata --format-version 1 >/dev/null
  echo "✓ pinned $CRATE → $rev (patch deactivated). Commit ${MANIFEST//,/ + } + Cargo.lock."
}

cmd_guard() {
  local hint="" key
  if grep -qE '^\[patch\.' Cargo.toml; then
    for key in $(core_keys); do hint="${hint:+$hint / }'just $key-pin'"; done
    fail "active [patch] in Cargo.toml — run $hint before committing"
  fi
}

case "${1:-}:${2:-}" in
  dev:?*) cmd_dev "$2" ;;
  pin:all)
    for key in $(core_keys); do cmd_pin "$key"; done
    ;;
  pin:?*) cmd_pin "$2" ;;
  guard:) cmd_guard ;;
  *) fail "usage: core-patch.sh dev <core> | pin <core|all> | guard" ;;
esac
