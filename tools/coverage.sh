#!/usr/bin/env bash
# Line coverage for the whole workspace, as an HTML report and a per-file
# summary. Needs cargo-llvm-cov (`cargo install cargo-llvm-cov`, or the
# distro's package).
#
#   tools/coverage.sh          the normal test suite
#   tools/coverage.sh --gpu    plus the engine's ignored GPU tests, one at a
#                              time (wgpu's GL backend fails in parallel); a few
#                              known failures don't stop the report
#
# The report is a map of what no test reaches, not a score: the iced views
# and widgets are left out on purpose, and shader code (WGSL) never shows.
# Output: target/llvm-cov/html/index.html and target/llvm-cov/summary.txt.
set -euo pipefail
cd "$(dirname "$0")/.."

# cargo-llvm-cov looks for llvm-cov/llvm-profdata in rustup's llvm-tools.
# A distro Rust (no rustup) uses the system LLVM, which rustc was built
# against, so point it there.
if ! command -v rustup >/dev/null 2>&1; then
    export LLVM_COV="${LLVM_COV:-$(command -v llvm-cov)}"
    export LLVM_PROFDATA="${LLVM_PROFDATA:-$(command -v llvm-profdata)}"
fi

# The front end's drawing code: tested by eye, not by unit tests.
IGNORE='(^|/)src/(ui|app/views)/|(^|/)src/main\.rs$|/tests?/|testkit\.rs$'

cargo llvm-cov clean --workspace
cargo llvm-cov --workspace --no-report
if [[ "${1:-}" == "--gpu" ]]; then
    cargo llvm-cov -p raw-editor-core --no-report --no-fail-fast \
        -- --ignored --test-threads=1 --skip nr_sweep --skip presence_dump || true
fi
cargo llvm-cov report --workspace --html --ignore-filename-regex "$IGNORE"
cargo llvm-cov report --workspace --summary-only --ignore-filename-regex "$IGNORE" \
    | tee target/llvm-cov/summary.txt | tail -1
echo "Report: target/llvm-cov/html/index.html"
