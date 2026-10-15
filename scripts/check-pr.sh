#!/usr/bin/env bash
# Minimum PR gate for this fork.
#
# Runs the three checks a reviewer would otherwise have to reproduce by hand for
# the crates this branch touches: formatting, clippy with warnings denied, and
# the unit tests of every crate that owns a changed file. The crate list comes
# from scripts/affected-crates.sh, which already fails open to the whole
# workspace when it cannot attribute a change.
#
# Usage:
#   scripts/check-pr.sh [base]      # default base: origin/main
set -euo pipefail

base="${1:-origin/main}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if ! git rev-parse --verify --quiet "$base" >/dev/null; then
    echo "check-pr: base '$base' is not a known revision" >&2
    exit 2
fi

# Compare the whole worktree against the base, not just committed history, so
# an author can run this before committing and see the crate list they are about
# to touch. Untracked files count too: new tests belong to their crate.
changed="$(
    {
        git diff --name-only "$base"
        git ls-files --others --exclude-standard
    } | sort -u
)"
if [[ -z "$changed" ]]; then
    echo "check-pr: no changed files against $base; nothing to check"
    exit 0
fi

plan="$(printf '%s\n' "$changed" | bash scripts/affected-crates.sh --stdin)"
crates="$(printf '%s' "$plan" | jq -r '.affected[]')"
test_crates="$(printf '%s' "$plan" | jq -r '.test_crates[]')"

echo "check-pr: formatting"
cargo fmt --all --check

if [[ -n "$crates" ]]; then
    echo "check-pr: clippy (warnings denied)"
    while IFS= read -r crate; do
        [[ -n "$crate" ]] || continue
        cargo clippy -p "$crate" --all-targets -- -D warnings
    done <<<"$crates"
else
    echo "check-pr: no Rust crate affected; skipping clippy"
fi

if [[ -n "$test_crates" ]]; then
    echo "check-pr: unit tests"
    while IFS= read -r crate; do
        [[ -n "$crate" ]] || continue
        cargo test -p "$crate" --lib
    done <<<"$test_crates"
else
    echo "check-pr: no crate with tests affected; skipping tests"
fi

echo "check-pr: all gates passed"
