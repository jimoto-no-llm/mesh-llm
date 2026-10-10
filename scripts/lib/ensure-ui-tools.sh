#!/usr/bin/env bash

# Fail fast when the mesh-llm UI toolchain is missing.
#
# `build-ui.sh` only reaches for `pnpm` after the Rust host build, so a
# missing binary wastes many minutes before failing with a bare `command not
# found`. Call `ensure_ui_tools` on the stale-build path -- before `pnpm
# install` -- to explain the fix instead. Up-to-date `dist` output needs no
# toolchain, so the fresh path stays unchecked. The check uses shell builtins
# only, so it runs even on a minimal PATH.
ensure_ui_tools() {
    local missing=()
    local tool
    for tool in node pnpm; do
        command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
    done
    if ((${#missing[@]} > 0)); then
        printf '%s\n' \
            "Error: ${missing[*]} is required to build the mesh-llm UI but was not found on PATH." \
            'Install Node.js 24, then enable pnpm 10 with: corepack enable && corepack prepare pnpm@10 --activate' \
            'Or skip the UI for this build with: export MESH_LLM_SKIP_UI=1' \
            'See CONTRIBUTING.md Prerequisites.' >&2
        return 1
    fi
}
