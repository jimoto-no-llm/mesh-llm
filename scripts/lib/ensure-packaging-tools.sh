#!/usr/bin/env bash

# Fail fast when the target platform's runtime-packaging tools are missing.
#
# `package-native-runtime.sh` only reaches for these tools after the long
# native and release builds, so a missing binary wastes many minutes before
# failing with a bare `command not found`. Call `ensure_packaging_tools`
# with the target runtime OS right after the target triple is resolved --
# before `prepare-llama.sh` -- to explain the fix instead. The check uses
# shell builtins only, so it runs even on a minimal PATH.
ensure_packaging_tools() {
    local runtime_os="${1:?usage: ensure_packaging_tools <linux|macos|windows>}"
    case "$runtime_os" in
        linux)
            command -v patchelf >/dev/null 2>&1 || {
                printf '%s\n' \
                    'Error: patchelf is required to package Linux native runtimes but was not found on PATH.' \
                    'Install it with: just bootstrap-build-tools' \
                    'Or on Debian/Ubuntu with: sudo apt-get install -y patchelf' >&2
                return 1
            }
            ;;
        macos)
            local tool
            for tool in install_name_tool otool; do
                command -v "$tool" >/dev/null 2>&1 || {
                    printf '%s\n' "Error: $tool is required to package macOS native runtimes; install the Xcode command line tools with: xcode-select --install" >&2
                    return 1
                }
            done
            ;;
        windows) ;;
        *)
            printf '%s\n' "cannot check packaging tools for runtime os: $runtime_os" >&2
            return 1
            ;;
    esac
}
