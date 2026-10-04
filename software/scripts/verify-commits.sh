# verify-commits.sh — check that each commit's own tree builds.
#
# The test suite runs against the working tree, so an uncommitted file can hide a broken commit:
# a variant used by code that was committed while its definition stayed unstaged compiles here and
# fails in the history. This script exports each commit into a temporary directory and runs
# `cargo check --workspace --all-targets` on exactly what that commit contains, which is what a
# reader who clones the repository at that commit gets.
#
#   scripts/verify-commits.sh                     # every commit on the current branch
#   scripts/verify-commits.sh HEAD~4..HEAD        # a range
#   scripts/verify-commits.sh --quick             # only the tip of each crate-touching commit
#
# Output is one line per commit plus a tally, because the point is a record that can be read later,
# not a wall of compiler output.

set -u
cd "$(git rev-parse --show-toplevel 2>/dev/null || echo .)" || exit 1
source software/scripts/dev-env.sh || exit 1

range="${1-}"
[ "$range" = "--quick" ] && range=""
if [ -z "$range" ]; then
    range="$(git rev-list HEAD)"
else
    range="$(git rev-list "$range")"
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

ok=0
broken=0
printf '%-10s %-9s %s\n' COMMIT BUILD DETAIL
for sha in $range; do
    tree="$work/$(git rev-parse --short "$sha")"
    mkdir -p "$tree"
    git archive "$sha" | tar -x -C "$tree" || {
        printf '%-10s %-9s %s\n' "$(git rev-parse --short "$sha")" EXPORT_FAIL "archive failed"
        broken=$((broken + 1))
        continue
    }
    # Older commits may not contain software/ at all; nothing to build there.
    if [ ! -f "$tree/software/Cargo.toml" ]; then
        printf '%-10s %-9s %s\n' "$(git rev-parse --short "$sha")" NONE "no software/ directory"
        continue
    fi
    out="$(cd "$tree/software" && cargo check --workspace --all-targets --quiet 2>&1)"
    if [ $? -eq 0 ]; then
        printf '%-10s %-9s %s\n' "$(git rev-parse --short "$sha")" OK "$(git log -1 --format=%s "$sha" | cut -c1-58)"
        ok=$((ok + 1))
    else
        first="$(printf '%s\n' "$out" | grep -m1 '^error' | cut -c1-90)"
        printf '%-10s %-9s %s\n' "$(git rev-parse --short "$sha")" BROKEN "$first"
        broken=$((broken + 1))
    fi
done

printf '\n%d builds, %d broken\n' "$ok" "$broken"
[ "$broken" -eq 0 ]
