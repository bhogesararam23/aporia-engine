#!/usr/bin/env bash
# eol-audit.sh — check that the bytes in the working tree are the bytes the repository says they are.
#
# Two failures hide themselves here and both of them cost a measurement, so both are checked:
#
#   1. A file whose worktree bytes differ from its stored blob while `git status` calls it clean.
#      With `core.autocrlf=true` and `text eol=lf`, git cleans CRLF to LF before comparing, so a file
#      that a tool wrote back with CRLF matches its blob in git's eyes and silently stops matching it
#      on disk. Nothing shows in a diff. A source file survives that; an archive artefact does not,
#      because a manifest digests the exact bytes (`aporia-store`), so a committed archive that drifts
#      fails its own integrity check on a fresh clone -- which is the reason `.gitattributes` carries
#      rules for `.jsonl` and `.apx` at all. `git diff` cannot see this class, so this script compares
#      `git hash-object --no-filters` against the index entry instead.
#
#   2. A file whose recorded stat data is stale. That shows up the other way round: `git status`
#      prints ` M` for a file whose contents are byte-identical to its blob, because the index cached
#      the size of an earlier CRLF-smudged checkout. A real two files sat like that across a whole
#      session, and the honest reading of a `git status` that disagrees with `git diff` is not "the
#      tree is dirty" but "the index is stale" -- so the check names the pair and says what fixes it
#      (refresh the index; never `--force`, never a rewrite of the file).
#
# The gate that fails is the one about evidence: every tracked path under a directory whose bytes are
# read as measurement input -- corpus, results, analysis, protocols, examples, test fixtures. Source
# files with CRLF are reported, because they are drift, and are not failed, because rewriting a file
# another session may be holding is a worse risk than a line ending in a comment.
#
#   scripts/eol-audit.sh          # exit 1 if any evidence byte differs or the index is stale
set -u
cd "$(dirname "$0")/.." || exit 1

# Tracked paths whose exact bytes are load-bearing.
byte_contracts() {
    git ls-files -- \
        software/benchmarks \
        software/examples \
        'documentation' \
        ':(glob)**/fixtures/**' \
        '*.jsonl' '*.apx' '*.air' '*.ap'
}

fail=0
stale=0
drift_src=0

# Check 1: worktree bytes against the index blob, for every byte-contract path.
while read -r path; do
    [ -z "$path" ] && continue
    [ -f "$path" ] || continue
    index_blob=$(git ls-files -s -- "$path" | awk '{print $2}')
    # --stage would print the same hash; the raw bytes are what a manifest digests, so compare those.
    work_blob=$(git hash-object --no-filters -- "$path")
    if [ "$index_blob" != "$work_blob" ]; then
        echo "bytes differ from the stored blob: $path"
        echo "    index    $index_blob"
        echo "    worktree $work_blob"
        fail=1
    fi
done <<<"$(byte_contracts)"

# Check 2: status claims a path is modified while the content comparison says otherwise.
dirty=$(git status --porcelain | awk '$1 ~ /M/ {print $2}')
if [ -n "$dirty" ]; then
    while read -r path; do
        [ -z "$path" ] && continue
        if ! git diff --name-only -- "$path" | grep -qxF "$path"; then
            echo "index stat data is stale (content matches its blob): $path"
            echo "    fix with: git update-index --really-refresh -- \"$path\", then re-check by hash"
            stale=1
        fi
    done <<<"$dirty"
fi

# Report: line-ending drift anywhere in the tree, and where it lands. Paths print relative to
# `software/`, because that is where this script changes directory, and the evidence test is on the
# path field rather than on the whole line -- testing the line matches the `w/crlf` token instead.
drift_paths=$(git ls-files --eol | awk '$2 ~ /^w\/(crlf|mixed)$/ {print $NF}')
if [ -n "$drift_paths" ]; then
    echo "line endings in the working tree that do not match the stored blob's:"
    git ls-files --eol | awk '$2 ~ /^w\/(crlf|mixed)$/ {print "    " $2 "  " $NF "  (blob " $1 ")"}'
    drift_src=$(echo "$drift_paths" | grep -c 'crates/\|Cargo.toml' || true)
    if echo "$drift_paths" | grep -qE '^(benchmarks|examples)/|/fixtures/'; then
        echo "    drift on an evidence or fixture path: refused by check 1 above"
        fail=1
    fi
fi

total=$(git ls-files | wc -l | tr -d ' ')
contracts=$(byte_contracts | grep -c . || true)
if [ "$fail" -eq 0 ] && [ "$stale" -eq 0 ]; then
    echo "eol-audit: $contracts byte-contract path(s) of $total tracked file(s) all match their blobs"
    echo "eol-audit: clean"
    exit 0
fi
echo "eol-audit: FAILED (bytes=$fail stale-index=$stale source-drift=$drift_src)"
exit 1
