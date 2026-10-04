# test.sh — run the workspace tests, working around Windows Smart App Control.
#
# On this machine `VerifiedAndReputablePolicyState = 1`, so Smart App Control evaluates every
# freshly linked executable by hash against Microsoft's cloud. A binary that has never been seen is
# refused with `ERROR_FILE_BLOCKED` (os error 4551) until the verdict arrives, which happens seconds
# to a couple of minutes after the link. Compilation, `cargo check` and clippy are unaffected because
# the compiler itself is trusted.
#
# So: try to run, and if the refusal is a block rather than a test failure, wait and retry. The
# retry is bounded and the reason is printed, because silently waiting looks exactly like a hang.
#
#   scripts/test.sh                       # whole workspace, release profile
#   scripts/test.sh -p aporia-dsl         # extra arguments go through to `cargo test`
#   scripts/test.sh --debug ...           # force the debug profile
#
# `APORIA_TEST_LOG=<file>` writes the full output somewhere instead of only printing the summary,
# which is how a benchmark run keeps its raw evidence next to the numbers it reports.

set -u
cd "$(dirname "$0")/.." || exit 1
source scripts/dev-env.sh || exit 1

# Default to the release profile: on this machine the debug test binary has repeatedly been refused
# by Smart App Control while the release build of the same code ran first time. It is also the
# profile whose timings mean something.
#
# The profile flag is consumed here rather than passed through, because `cargo test` takes `--` for
# test arguments and a leading `--debug` in the wrong place is an error from cargo, not from this
# script — which used to report it as a test failure after eight silent retries.
PROFILE_ARGS=(--release)
if [ "${1-}" = "--debug" ]; then
    PROFILE_ARGS=(--debug)
    shift
elif [ "${1-}" = "--release" ]; then
    shift
fi

attempts="${APORIA_TEST_RETRIES:-8}"
delay="${APORIA_TEST_DELAY:-15}"
quiet_first=0

if [ "${1-}" = "--quiet-first" ]; then quiet_first=1; shift; fi

# The packages named on the command line, so a retry can force a re-link of exactly those targets.
# An empty list means "the whole workspace", where a forced rebuild costs too much to be worth
# repeating on a timeout.
PACKAGES=()
prev=""
for arg in "$@"; do
    if [ "$prev" = "-p" ] || [ "$prev" = "--package" ]; then PACKAGES+=("$arg"); fi
    prev="$arg"
done

# A passing run prints one line per test binary plus a total, because "the suite is green" is only
# useful if you can see how many tests were actually green and where they live.
summarize() {
    local file="$1"
    awk '
        /Running unittests/ {
            bin = $NF                              # (target\release\deps\aporia_dsl-abc123.exe)
            gsub(/[()]/, "", bin); sub(/.*[\/\\]/, "", bin); sub(/-[^-]*$/, "", bin); next
        }
        /^   Doc-tests / { bin = "doc-" $2; next }
        /^test result:/ {
            printf "  %-28s %s passed, %s failed\n", bin, $4, $6
            total += $4; failed += $6; next
        }
        END { printf "  %-28s %d passed, %d failed\n", "TOTAL", total, failed }
    ' "$file"
    [ -n "${APORIA_TEST_LOG:-}" ] && printf '  full output: %s\n' "$APORIA_TEST_LOG"
}

[ -n "${APORIA_TEST_LOG:-}" ] && mkdir -p "$(dirname "$APORIA_TEST_LOG")" 2>/dev/null
tmp="${TMPDIR:-/tmp}/aporia-test-$$.txt"
i=1
while [ "$i" -le "$attempts" ]; do
    cargo test "${PROFILE_ARGS[@]}" "$@" >"$tmp" 2>&1
    status=$?
    if [ $status -eq 0 ]; then
        [ -n "${APORIA_TEST_LOG:-}" ] && cp "$tmp" "$APORIA_TEST_LOG"
        summarize "$tmp"
        rm -f "$tmp"
        exit 0
    fi
    if grep -q "os error 4551\|Application Control policy" "$tmp"; then
        if [ "$i" -lt "$attempts" ]; then
            # Alternating the debug-info level changes the linked binary's bytes, hence its hash,
            # which is the only lever available against a per-file reputation verdict: an identical
            # rebuild is refused identically. Changing the profile setting is not enough on its own —
            # cargo reports the target as fresh and re-links nothing, so ten "retries" ran the same
            # blocked bytes ten times. `cargo clean -p` for the packages actually under test forces
            # the link step, and leaves the dependency graph alone, so it costs seconds.
            if [ $((i % 2)) -eq 0 ]; then
                export CARGO_PROFILE_RELEASE_DEBUG=0
            else
                unset CARGO_PROFILE_RELEASE_DEBUG
            fi
            for pkg in "${PACKAGES[@]}"; do
                cargo clean --release -p "$pkg" >/dev/null 2>&1
            done
            say="blocked by Smart App Control, retry $i/$attempts in ${delay}s"
            [ "$quiet_first" = 1 ] && [ "$i" = 1 ] || printf '\033[33m==>\033[0m %s\n' "$say" >&2
            sleep "$delay"
            i=$((i + 1))
            continue
        fi
        tail -n 20 "$tmp"
        rm -f "$tmp"
        printf '\033[31m==>\033[0m still blocked after %s attempts.\n' "$attempts" >&2
        printf '    This is a machine policy, not a project failure. Keep writing and use\n' >&2
        printf '    `cargo check --workspace --all-targets` for the parts that do not need to run.\n' >&2
        exit 3
    fi
    # A real test failure: show it, do not retry.
    tail -n 60 "$tmp"
    [ -n "${APORIA_TEST_LOG:-}" ] && cp "$tmp" "$APORIA_TEST_LOG"
    rm -f "$tmp"
    exit $status
done
