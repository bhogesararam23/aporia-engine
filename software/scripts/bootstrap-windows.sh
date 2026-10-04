# bootstrap-windows.sh — bring a clean Windows box to the state where `cargo test` works for
# APORIA, and record what this repository actually needs.
#
# Run from Git Bash. Everything here is a user-space install; no admin rights are assumed beyond
# what the Visual Studio bootstrapper itself asks for.
#
# This script is idempotent: each step checks for the thing it installs and skips it if present.

set -u

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1m!!\033[0m %s\n' "$*" >&2; }

# ---------------------------------------------------------------- rust toolchain
if ! command -v rustc >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/rustc.exe" ]; then
    say "installing rustup + stable (minimal profile, no PATH edit)"
    tmp="$(mktemp -d)"
    curl -sSL --max-time 300 -o "$tmp/rustup-init.exe" \
        https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe || exit 1
    "$tmp/rustup-init.exe" -y --default-toolchain stable --profile minimal --no-modify-path || exit 1
    rm -rf "$tmp"
fi
export PATH="$HOME/.cargo/bin:$PATH"
say "rust: $(rustc --version), cargo: $(cargo --version)"

# ---------------------------------------------------- lint and format components
for c in clippy rustfmt; do
    if ! "$HOME/.cargo/bin/cargo" "${c#cargo}" --version >/dev/null 2>&1; then
        say "adding component $c"
        rustup component add "$c" || warn "component $c failed to install"
    fi
done

# --------------------------------------------------------- MSVC linker and SDK
# Rust needs link.exe plus the ucrt/um import libraries. Without them the very first build fails
# with LNK1181, and the failure looks like a Rust bug: Git Bash has a /usr/bin/link.exe (the
# hard-link utility) that shadows nothing useful.
vs_probe() {
    for c in \
        "/c/Program Files/Microsoft Visual Studio/2022/Enterprise" \
        "/c/Program Files/Microsoft Visual Studio/2022/Professional" \
        "/c/Program Files/Microsoft Visual Studio/2022/Community" \
        "/c/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools"
    do
        [ -d "$c/VC/Tools/MSVC" ] && { printf '%s' "$c"; return 0; }
    done
    return 1
}

if ! vs_probe >/dev/null; then
    say "installing Visual Studio 2022 Build Tools with the VC++ workload (~2-3 GB)"
    tmp="$(mktemp -d)"
    curl -sSL --max-time 600 -o "$tmp/vs_buildtools.exe" \
        https://aka.ms/vs/17/release/vs_buildtools.exe || exit 1
    # --includeRecommended pulls the MSVC toolset and a Windows 11 SDK. The SDK that lands with it
    # carries ucrt and um import libraries; check afterwards rather than trusting the exit code.
    "$tmp/vs_buildtools.exe" --wait --quiet --norestart --nocache \
        --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended
    rm -rf "$tmp"
fi

say "VS root: $(vs_probe || echo none)"

# ------------------------------------------------------------------- sanity gate
cd "$(dirname "$0")/.." || exit 1
if source scripts/dev-env.sh; then
    printf 'fn main(){}\n' > "$(mktemp -u "${TMPDIR:-/tmp}/aporia-XXXXXX").rs"
    probe="$(mktemp -d)"
    rustc -O "$probe.rs" -o "$probe/probe.exe" 2>"$probe/err" && say "link probe ok" \
        || { warn "link probe failed:"; cat "$probe/err"; rm -rf "$probe" "$probe.rs"; exit 1; }
    rm -rf "$probe" "$probe.rs"
else
    warn "dev-env.sh could not find a usable toolchain"
    exit 1
fi

# ------------------------------------------------------------- optional extras
# These are not required to build or test APORIA; they are required to *run* the corresponding
# parts of the project, and the repository says so rather than pretending otherwise.
if ! command -v nvcc >/dev/null 2>&1; then
    say "note: no CUDA toolkit (and no NVIDIA device driver) on this machine — gpu/ will not build"
fi
if ! command -v julia >/dev/null 2>&1; then
    say "note: no Julia on this PATH — julia/references cannot be cross-checked"
fi
if ! command -v nasm >/dev/null 2>&1; then
    say "note: no nasm; the handwritten kernels use Rust's asm! macro, which needs no external assembler"
fi

say "done. run:  source scripts/dev-env.sh && cargo test --workspace"
