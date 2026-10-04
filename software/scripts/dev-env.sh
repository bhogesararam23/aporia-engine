# dev-env.sh — put a plain Git Bash / MSYS2 shell into the state a "Developer Command
# Prompt for VS 2022" would give you, so `cargo` can find the MSVC toolset and the
# Windows SDK without vcvars64.bat.
#
# Source it, do not execute it:      source scripts/dev-env.sh
#
# Why this exists: rustc can locate link.exe on its own, but it only learns the
# ucrt/um library and include directories from LIB / INCLUDE. In a shell that never
# went through vcvars, those are empty and linking fails with LNK1181 on the first
# import library. Committing the detection here keeps every recorded benchmark run
# reproducible from a clean checkout.

_aporia_vs_root=""
for _cand in \
    "/c/Program Files/Microsoft Visual Studio/2022/Enterprise" \
    "/c/Program Files/Microsoft Visual Studio/2022/Professional" \
    "/c/Program Files/Microsoft Visual Studio/2022/Community" \
    "/c/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools" \
    "/c/Program Files (x86)/Microsoft Visual Studio/2022/Community"
do
    [ -d "$_cand/VC/Tools/MSVC" ] && _aporia_vs_root="$_cand" && break
done

if [ -z "$_aporia_vs_root" ]; then
    echo "dev-env: no Visual Studio 2022 with the VC++ workload found." >&2
    echo "dev-env: install it with  scripts/bootstrap-windows.sh" >&2
    return 1 2>/dev/null || exit 1
fi

# Highest installed MSVC toolset version (they sort lexicographically well enough at
# fixed-width 5.5 numbering: 14.44.35207).
_aporia_msvc="$_aporia_vs_root/VC/Tools/MSVC/$(ls "$_aporia_vs_root/VC/Tools/MSVC" | sort -V | tail -1)"

_aporia_sdk_root="/c/Program Files (x86)/Windows Kits/10"
if [ ! -d "$_aporia_sdk_root/Lib" ]; then
    echo "dev-env: Windows SDK not found under $_aporia_sdk_root" >&2
    return 1 2>/dev/null || exit 1
fi
_aporia_sdk_ver="$(ls "$_aporia_sdk_root/Lib" | sort -V | tail -1)"
_aporia_sdk="$_aporia_sdk_root"

# POSIX -> Windows path conversion for the values MSVC tooling expects.
_aporia_win() {
    # $1: /c/... style path -> C:\... style
    local p="${1#/}"
    local drive="${p%%/*}"
    local rest="${p#*/}"
    printf '%s:\\%s' "$(printf '%s' "$drive" | tr 'a-z' 'A-Z')" "$(printf '%s' "$rest" | tr '/' '\\')"
}

export APORIA_VS_ROOT="$_aporia_vs_root"
export APORIA_MSVC_VERSION="$(basename "$_aporia_msvc")"
export APORIA_SDK_VERSION="$_aporia_sdk_ver"

export INCLUDE="$(_aporia_win "$_aporia_msvc/include");$(_aporia_win "$_aporia_sdk/Include/$_aporia_sdk_ver/ucrt/x64");$(_aporia_win "$_aporia_sdk/Include/$_aporia_sdk_ver/um/x64");$(_aporia_win "$_aporia_sdk/Include/$_aporia_sdk_ver/shared")"
export LIB="$(_aporia_win "$_aporia_msvc/lib/x64");$(_aporia_win "$_aporia_sdk/Lib/$_aporia_sdk_ver/ucrt/x64");$(_aporia_win "$_aporia_sdk/Lib/$_aporia_sdk_ver/um/x64")"
export LIBPATH="$LIB"
export PATH="$PATH:$_aporia_msvc/bin/Hostx64/x64:$_aporia_sdk/bin/$_aporia_sdk_ver/x64"

# Rust lives here on this machine and rustup did not modify PATH.
if [ -d "$HOME/.cargo/bin" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

unset _aporia_vs_root _aporia_msvc _aporia_sdk _aporia_sdk_ver

if [ "${1-}" = "--print" ]; then
    echo "MSVC toolset : $APORIA_MSVC_VERSION"
    echo "Windows SDK  : $APORIA_SDK_VERSION"
    echo "INCLUDE      : $INCLUDE"
    echo "LIB          : $LIB"
fi
