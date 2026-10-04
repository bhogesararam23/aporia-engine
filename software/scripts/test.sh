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

set -u
cd "$(dirname "$0")/.." || exit 1
source scripts/dev-env.sh || exit 1

# Default to the release profile: on this machine the debug test binary has repeatedly been refused
# by Smart App Control while the release build of the same code ran first time. It is also the
# profile whose timings mean something.
PROFILE_ARGS=()
if [ "${1-}" != "--release" ] && [ "${1-}" != "--debug" ]; then
    PROFILE_ARGS=(--release)
fi

attempts="${APORIA_TEST_RETRIES:-8}"
delay="${APORIA_TEST_DELAY:-15}"
quiet_first=0

if [ "${1-}" = "--quiet-first" ]; then quiet_first=1; shift; fi

i=1
while [ "$i" -le "$attempts" ]; do
    out="$(cargo test "${PROFILE_ARGS[@]}" "$@" 2>&1)"
    status=$?
    if [ $status -eq 0 ]; then
        printf '%s\n' "$out" | tail -n 40
        exit 0
    fi
    if printf '%s' "$out" | grep -q "os error 4551\|Application Control policy"; then
        if [ "$i" -lt "$attempts" ]; then
            say="blocked by Smart App Control, retry $i/$attempts in ${delay}s"
            [ "$quiet_first" = 1 ] && [ "$i" = 1 ] || printf '\033[33m==>\033[0m %s\n' "$say" >&2
            sleep "$delay"
            i=$((i + 1))
            continue
        fi
        printf '%s\n' "$out" | tail -n 20
        printf '\033[31m==>\033[0m still blocked after %s attempts.\n' "$attempts" >&2
        printf '    This is a machine policy, not a project failure. Keep writing and use\n' >&2
        printf '    `cargo check --workspace --all-targets` for the parts that do not need to run.\n' >&2
        exit 3
    fi
    # A real test failure: show it, do not retry.
    printf '%s\n' "$out" | tail -n 60
    exit $status
done
