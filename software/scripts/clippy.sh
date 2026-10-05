# clippy.sh — run the lint gate on this machine.
#
# `cargo clippy` is refused here. Smart App Control blocks `cargo-clippy.exe` by hash
# (`os error 4551`) and the verdict has not arrived, so the command that normally checks
# lints cannot be spawned at all. What `cargo clippy` actually does is set
# `RUSTC_WORKSPACE_WRAPPER` to `clippy-driver` and re-enter `cargo check`; the driver is a
# different file, it is trusted by the same policy that lets `rustc` run, and it works. So
# this script performs the substitution that the blocked wrapper would have done. The lints
# reported are clippy's own, from the same component, on the command line `cargo clippy`
# would have built.
#
#   scripts/clippy.sh                       # whole workspace, all targets, release profile
#   scripts/clippy.sh -p aporia-store       # extra arguments go through to `cargo check`
#
# Cargo caches lint results per crate, so a second run over unchanged code prints nothing.
# Silence is a pass, not a skipped gate; `touch` a source file to force a re-lint.

set -u
cd "$(dirname "$0")/.." || exit 1
source scripts/dev-env.sh || exit 1

sysroot="$(rustc --print sysroot)" || exit 1
driver="$sysroot/bin/clippy-driver.exe"
if [ ! -f "$driver" ]; then
    driver="$sysroot/bin/clippy-driver"
fi
if [ ! -f "$driver" ]; then
    echo "clippy: no clippy-driver next to $sysroot/bin/rustc" >&2
    echo "clippy: install it with  rustup component add clippy" >&2
    exit 1
fi

# Only add --workspace when the caller has not named packages: cargo refuses both.
selected=""
for arg in "$@"; do
    case "$arg" in
        -p | --package | --all | --exclude) selected="named" ;;
    esac
done
if [ -z "$selected" ]; then
    set -- --workspace "$@"
fi

export RUSTC_WORKSPACE_WRAPPER="$driver"
exec cargo check --all-targets --release "$@"
