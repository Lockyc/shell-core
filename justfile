# shell-core — task runner

# Recipes run in `sh`, which doesn't inherit cargo from an interactive fish/zsh setup.
export PATH := env_var('HOME') + "/.cargo/bin:" + env_var('PATH')

# `default` pipes `just --list` through a small stock-perl filter that clips long recipe
# docs to your terminal width (…) instead of wrapping. Self-contained — no external files;
# falls back to plain `just --list` where perl is absent. Edit the recipes below, not this.
# List available recipes
default:
    @if command -v perl >/dev/null 2>&1; then just --color always --list | perl -CS -Mutf8 -lpe 'BEGIN{($w)=`stty size 2>/dev/null </dev/tty`=~/ (\d+)/; $w||=100; $col=(-t STDOUT && !exists $ENV{NO_COLOR})} s/\e\[[0-9;]*m//g unless $col; (my $v=$_)=~s/\e\[[0-9;]*m//g; if(length($v)>$w){my($o,$n)=("",0); while(length && $n<$w-1){ if($col && s/^(\e\[[0-9;]*m)//){$o.=$1}else{s/^(.)//;$o.=$1;$n++} } $_=$o."…".($col?"\e[0m":"")}'; else just --list; fi

# NOTE: `menu`, `home`, and `detach` are all #[cfg(feature = "runtime")], so a BARE `cargo test` /
# `cargo clippy` compiles none of them and reports green having checked almost nothing. Every
# check recipe below therefore passes --features runtime. Don't drop it.

# Run the test suite (--features runtime, or the unit tests don't compile in at all)
[group("check")]
test:
    cargo test --features runtime

# Clippy lints, warnings are errors (--features runtime, or menu/home aren't linted)
[group("check")]
lint:
    cargo clippy --features runtime --all-targets -- -D warnings

# Format all Rust files in place
[group("check")]
fmt:
    cargo fmt

# Build the runtime surface
[group("build")]
build:
    cargo build --features runtime

# Prove the default feature set is still zero-dependency — the load-bearing invariant that lets
# consumers take shell-core as a light [build-dependencies] entry without dragging in tauri.
[group("check")]
zero-dep:
    #!/usr/bin/env bash
    set -euo pipefail
    deps="$(cargo tree --no-default-features -e normal | tail -n +2)"
    if [ -n "$deps" ]; then
      echo "✗ default features are no longer zero-dep — a consumer's build-deps would pull:" >&2
      echo "$deps" >&2
      exit 1
    fi
    echo "✓ default features are zero-dependency"

# Non-mutating pre-merge gate: rustfmt check, clippy, and the test suite. What CI would run.
[group("check")]
gate:
    cargo fmt --check
    cargo clippy --features runtime --all-targets -- -D warnings
    cargo test --features runtime
