#!/usr/bin/env bash
# Build/lint/test a branch or worktree and print a SHORT verdict.
#
#   tools/check_branch.sh [branch-or-path] [--gpu]
#
# With no argument, checks the current directory. A branch name is resolved to
# its worktree (git worktree list); a path is used as is. Full logs go to
# $CHECK_LOG_DIR (default /tmp/rawedit-check/<name>/), and only the failing
# lines are printed, so a caller never has to read cargo's output.
#
#   clippy : cargo clippy --all-targets -- -D warnings   (CI's exact gate)
#   test   : cargo test                                  (ignored tests are skipped)
#   --gpu  : also run the ignored GPU tests (needs a GPU or lavapipe; slow)
#
# Every cargo command goes through tools/cargo-lock.sh (one at a time machine-wide).
#
# Exit status: 0 all passed, 1 something failed, 2 usage error.
set -u
LOCK="$(cd "$(dirname "$0")" && pwd)/cargo-lock.sh"

target=""
gpu=0
for a in "$@"; do
  case "$a" in
    --gpu) gpu=1 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) target="$a" ;;
  esac
done

dir=""
if [ -z "$target" ]; then
  dir=$PWD
elif [ -d "$target" ]; then
  dir=$(cd "$target" && pwd)
else
  # branch name -> worktree path
  dir=$(git worktree list --porcelain | awk -v b="refs/heads/$target" '
    /^worktree /{p=$2} $1=="branch" && $2==b {print p; exit}')
  if [ -z "$dir" ]; then
    echo "no worktree has branch '$target' checked out (give a path, or: git worktree add <path> $target)" >&2
    exit 2
  fi
fi
cd "$dir" || exit 2
[ -f Cargo.toml ] || { echo "$dir has no Cargo.toml" >&2; exit 2; }

name=$(git rev-parse --abbrev-ref HEAD 2>/dev/null | tr '/' '_')
log=${CHECK_LOG_DIR:-/tmp/rawedit-check}/$name
mkdir -p "$log"
echo "checking $dir @ $(git rev-parse --short HEAD) ($(git rev-parse --abbrev-ref HEAD))"
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  echo "  note: uncommitted changes in the tree"
fi

fail=0
run() { # run <label> <logfile> <command...>
  label=$1; out=$2; shift 2
  start=$(date +%s)
  if "$LOCK" "$@" >"$out" 2>&1; then
    printf '  %-7s PASS  (%ss)\n' "$label" $(( $(date +%s) - start ))
    return 0
  fi
  printf '  %-7s FAIL  (%ss)  full log: %s\n' "$label" $(( $(date +%s) - start )) "$out"
  fail=1
  return 1
}

run clippy "$log/clippy.log" cargo clippy --all-targets -- -D warnings \
  || grep -E '^(error|warning)(\[|:)|^ +--> ' "$log/clippy.log" | head -20

run test "$log/test.log" cargo test \
  || { grep -E '^test .* FAILED|^failures:|panicked at|^error(\[|:)' "$log/test.log" | head -20; }
grep -E '^test result:' "$log/test.log" | tail -3 | sed 's/^/          /'

if [ "$gpu" = 1 ]; then
  run gpu "$log/gpu.log" cargo test --lib -- --ignored \
    || { grep -E '^test .* FAILED|panicked at|^error(\[|:)' "$log/gpu.log" | head -20; }
  grep -E '^test result:' "$log/gpu.log" | tail -1 | sed 's/^/          /'
fi

if [ "$fail" = 0 ]; then echo "RESULT: PASS"; else echo "RESULT: FAIL"; fi
exit "$fail"
