#!/usr/bin/env bash

# Fail fast when the repository Cargo configuration cannot be satisfied.
#
# `.cargo/config.toml` sets `rustc-wrapper = "sccache"`, so every `cargo`
# invocation fails with a cryptic `could not execute process sccache ...`
# when sccache is missing. Call `ensure_sccache` at the top of a build
# entrypoint -- before any expensive native build -- to explain the two
# supported fixes instead. An explicitly emptied `RUSTC_WRAPPER` is the
# documented bypass (see CONTRIBUTING.md Prerequisites) and skips the check.
ensure_sccache() {
    if [[ "${RUSTC_WRAPPER-__unset}" != "__unset" ]]; then
        return 0
    fi
    if command -v sccache >/dev/null 2>&1; then
        return 0
    fi
    cat >&2 <<'EOF'
Error: sccache is required by .cargo/config.toml but was not found on PATH.
Install it with: just bootstrap-build-tools
Or build without the compiler cache for this shell with: export RUSTC_WRAPPER=""
See CONTRIBUTING.md Prerequisites.
EOF
    return 1
}
