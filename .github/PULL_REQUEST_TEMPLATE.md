## Title

Use an accurate title that describes the user-visible change or fix.

## Original problem

What was broken, missing, confusing, or risky before this PR?

## Diagnostics

How did you verify the problem? Include commands, logs, screenshots, or links when useful.

## Fix

How does this PR fix the problem? Call out compatibility, protocol, or migration concerns if any.

## Validation

- [ ] I ran the relevant local checks, or explained why they do not apply.
- [ ] UI changes include screenshots or video, or explain why visual aids are not needed.

<!-- Rust changes: format touched files, run cargo check plus cargo clippy --all-targets -- -D warnings per touched crate (-p mesh-llm too when reachable from the binary), run the relevant tests, and keep just check-commits green for the title. See AGENTS.md Pre-Commit Checklist. -->
