#!/usr/bin/env bash
# Run a command (normally `cargo ...`) while holding ONE machine-wide lock, so
# parallel agents queue their builds and tests instead of competing for the CPU.
#
#   tools/cargo-lock.sh cargo test some_name
#   tools/cargo-lock.sh cargo clippy --all-targets -- -D warnings
#
# The lock is released when the command exits (or is killed). Waiting counts
# against a caller's own timeout: if a tool call times out while queued, nothing
# was held, so simply run it again. The command runs at low priority (nice 10).
# Override the lock file with RAWEDIT_CARGO_LOCK.
set -u
lock=${RAWEDIT_CARGO_LOCK:-$HOME/.cache/rawedit-cargo.lock}
mkdir -p "$(dirname "$lock")"
exec 9>"$lock"
if ! flock -n 9; then
  echo "[cargo-lock] another build/test is running; waiting..." >&2
  flock 9
fi
exec nice -n 10 "$@"
