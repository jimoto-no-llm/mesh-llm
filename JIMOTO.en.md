# JIMOTO — Local Fork Guide (English Draft)

> Note: This fork follows a Japanese-first policy; this English version is a reference translation. The Japanese original takes precedence.

This is the dedicated guide for jimoto-no-llm/mesh-llm. In addition to the upstream usage ([README](README.md)), it summarizes what this fork can do and what it intends to do.

## Fork Policy

- Develop and operate in Japanese. Documentation and PR descriptions are Japanese-first.
- Make it faster. Start with measurable improvements, such as reducing allocations on the inference hot path.
- Don't break it. Maintain compatibility with the mesh protocol and plugin conventions.
- Make operations visible. Lock in operations with main branch protection, labels, and CODEOWNERS.
- Track upstream. Regularly merge upstream's main and keep the diff small.

## Getting Started (Shortest Path)

```bash
git clone https://github.com/jimoto-no-llm/mesh-llm
cd mesh-llm
just build
./target/debug/mesh-llm setup
./target/debug/mesh-llm serve --auto
```

Open `http://localhost:3131` in your browser for the web console; the API is at `http://localhost:9337/v1`. To check fork information, run `mesh-llm jimoto` (add `--json` for machine-readable output).

## Supported Environments

- Linux (x86_64), macOS, and Windows are the primary environments.
- CONTRIBUTING and `install.sh` / `install.ps1` are the reference procedures for each environment.
- Records of successful runs following the procedures are collected in Issue #52.

## Verified Environments

| Environment | Version | Date | Record |
|---|---|---|---|
| — | — | — | — |

No records yet. If it works for you, please report in Issue #52.

## Using It from a Smartphone

### What you can do today: use the smartphone as a client

Expose mesh-llm on a home server and access it from a smartphone browser or app. Publish it on the LAN from the server side:

```bash
./target/debug/mesh-llm serve --auto --listen-all --port 9337 --console 3131
```

Opening `http://<server-LAN-address>:3131` in the smartphone browser gives you the web console. You can also send requests directly from apps to the OpenAI-compatible API (`:9337/v1`). Heavy inference runs on the server side, so you don't need to worry about the phone's battery or heat.

There are two caveats:

- `--listen-all` listens on all interfaces (`0.0.0.0`) for both the API and the web console. There is no per-user authentication, so any device on the same network can use it. Use it only on a trusted network such as your home LAN.
- Runtime operation APIs — fetching/applying configuration and loading/unloading models — only accept connections from localhost (others get a 403). Perform these operations on the server side.

### Running directly on an Android device (experimental)

Upstream publishes Linux ARM64 (`aarch64-unknown-linux-gnu`) builds, which may run as Linux binaries inside a Termux + proot Ubuntu environment on Android. The conditions and caveats are as follows:

- 8GB+ RAM recommended. Start with small quantized models (Q4, 1B–8B class).
- A full `just build` set is heavy on a handset, so first verify a standalone serve with a small model plus `--local-model-only`.
- Heat and battery drain are significant, so it is not suited for always-on server use. For always-on operation, the recommended setup is a home server with the smartphone as a client.

### Running on iOS (future)

In the upstream ROADMAP, iOS (Swift + XCFramework) comes first, followed by Android. `mesh/sdk/swift` contains the XCFramework build instructions, and embedded clients (iOS, macOS, Android) are officially supported targets. When this fork reaches the stage of shipping an iOS app, the steps will be added here. Until then, browser-based use is the reliable option.

## About Releases

### Current status

- There are no fork-specific release binaries yet.
- To just try it out, use a source build ([Quickstart](README.md#クイックスタート)) or the upstream distribution binaries.
- `install.sh` / `install.ps1` can switch the download source via the `MESH_LLM_INSTALL_REPO` environment variable. Once the fork starts publishing releases, you can fetch them with `MESH_LLM_INSTALL_REPO=jimoto-no-llm/mesh-llm`.

### Conditions for publishing fork releases

Upstream's `just release vX.Y.Z` launches the Release workflow and automates everything from version bump, SDK assets, tagging, all-platform builds, through publication. To run it in the fork, the following are needed:

1. Build environments for GPU, macOS, and Windows (upstream uses Depot and dedicated runners. This fork's Actions are currently not running).
2. Release signing keys (key pair for `release-attestation`) and tidying up the publication destination.
3. A version-number policy (track upstream vs. fork-specific versioning).

Once the environment is ready, we will start with a small release (e.g. CPU-only) and solidify the procedure.

## Operating Rules

- main is protected (1 approval required, stale dismissal, thread resolution required, squash-only). Only Organization admins can bypass PR requirements.
- Commit subjects and PR titles follow Conventional Commits. Check commits with `just check-commits` and PR titles with `python3 scripts/check-conventional-commit.py --message "<title>"`. Descriptions are 4 lines in Japanese (overview / changes / verification / caveats) plus an Architecture section as needed.
- Use the labels `perf`, `skippy`, `scheduler`, `cache`, `maintenance`, `documentation`, and `ci:canary`, plus the fork-specific `jimoto`, `upstream-sync`, and `mobile`.
- Rust changes require check, clippy (warnings denied), and related tests for the affected crates. The hot 4 crates and the workspace have a `clippy::perf` gate.

## Tracking Upstream

- Regularly merge main from `Mesh-LLM/mesh-llm`.
- Keep fork-specific diffs (localization, speedups, rules) small, and resolve conflicts on the fork side.
- Send improvements that can go back upstream as PRs to upstream.

## Roadmap

The overall direction is discussed in Issue #48. Progress is tracked with milestones.

- [Phase 0: Finalize the overall direction](https://github.com/jimoto-no-llm/mesh-llm/milestone/1)
- [Phase 1: Procedures and release preparation](https://github.com/jimoto-no-llm/mesh-llm/milestone/2)
- [Phase 2: Launch operations](https://github.com/jimoto-no-llm/mesh-llm/milestone/3)
- [Phase 3: Expand supported environments](https://github.com/jimoto-no-llm/mesh-llm/milestone/4)
- [Phase 4: English support and SDK work](https://github.com/jimoto-no-llm/mesh-llm/milestone/5)
- [Phase 5: Distributed inference and reward design](https://github.com/jimoto-no-llm/mesh-llm/milestone/6)
