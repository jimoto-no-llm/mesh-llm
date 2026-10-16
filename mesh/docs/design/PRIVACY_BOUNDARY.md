# Distributed Inference Privacy Boundary

Research record for [Research #47](https://github.com/jimoto-no-llm/mesh-llm/issues/47).
It surveys what a participating node can observe, compares options for
confidential inference, and states Go/No-Go conditions.

Status: **research only, deferred**. This document is not a commitment to
implement confidential inference, and nothing here asserts a cryptographic
guarantee. Priority sits below making ordinary distributed inference stable.

## What we are trying to protect

| Asset | Where it lives during a split | Protected today? |
|---|---|---|
| Input prompt / chat history | Sent to the node(s) that compute the first stage | No (the computing peer sees it) |
| Model output | Produced by the last stage | No (that peer sees it) |
| Intermediate activations | Raw `f32` values on the stage wire (`SKIPPY_SPLITS.md`) | No (the receiving stage sees them) |
| KV cache | Held by the serving runtime on each node | No (local to that node) |
| Model weights | Materialised from the package on every participating node | No (each node holds them) |
| Membership / attribution | Advertised as coarse admission state | Partially (see below) |

## Trust boundaries

| Boundary | Who can read the payload | Notes |
|---|---|---|
| Nodes you operate | You | Baseline; the only boundary with no third party |
| Trusted community peers | That peer | Private meshes with invite tokens; still a third party |
| Public mesh | Any admitted peer that serves a stage | Admission is not confidentiality |
| Relay / path observer | Nobody, if QUIC holds | `MESHES.md`: traffic is end-to-end encrypted, and a relay forwards ciphertext it cannot read |
| Local host / OS | Anyone with access to the machine | Out of scope for this document |

Transport encryption is **not** peer confidentiality: QUIC protects the path,
while the peer that runs a stage necessarily receives the plaintext it computes
on. `MESHES.md` states this boundary explicitly for transit.

## What is already mitigated

- **Transit**: QUIC (iroh) end-to-end encryption; relay fallback carries
  ciphertext only.
- **Admission**: only coarse admission state is advertised, not raw activity
  (`MESHES.md`, Blackboard privacy section). Private meshes gate membership by
  invite token.
- **Blackboard**: posts are ephemeral and scrubbed for obvious PII; they stay
  inside the mesh and are visible to every participating peer, which is called
  out in `mesh/docs/AGENTS.md`.

None of these protect the assets in the first table against a peer that is
admitted and computing.

## Candidate approaches

| Option | Threat coverage | Runtime cost | Implementation difficulty | Verdict |
|---|---|---|---|---|
| Trusted-nodes-only (private mesh, self-hosted) | Strong against strangers, none against a trusted peer that turns | None | Low — exists today | **Recommended now**; document it as the answer for sensitive work |
| TEE (CPU/GPU confidential computing) | Strong against the host operator, if attestation is verified end to end | Moderate to large; GPU TEE availability and attestation plumbing are the blockers | High | Not adopted; revisit when target hardware and attestation are available |
| FHE / MPC | Strongest on paper | Orders of magnitude over plain inference for transformer workloads | Very high | Not adopted; revisit only if the field's practical cost changes |
| Activation obfuscation (masking, noise, layer shuffle) | Partial and hard to bound; no clear security claim | Low to moderate | Medium | Not adopted; no convincing threat model without a proven scheme |

### Why no benchmark numbers

This environment has no confidential-computing hardware and no GPU, and the
native runtime is not buildable here, so the speed/memory comparison demanded by
the issue cannot be produced honestly. Reporting numbers from unrelated
hardware would be worse than reporting none. The comparison above is therefore
qualitative, and the first measurement a future contributor should take is
plain inference versus a TEE-attested run of the same small model.

## Go / No-Go conditions

**Go** for a confidential-inference line only when all hold:

1. A concrete user need that private meshes cannot serve.
2. Target hardware with usable GPU/CPU confidential computing and an
   attestation chain we can verify, not just a vendor claim.
3. A measured overhead budget (for example: under a stated multiple of plain
   decode tok/s) that users would accept.
4. A clear trust statement: who attests what, and what remains unverified.

**No-Go** while ordinary split inference is unstable, or while the only
available option is a scheme whose security claim we cannot state precisely.

## User-facing caveats

- Serving on a public mesh means the peers running your request can read the
  prompt, the output, and the activations they compute on.
- For sensitive work, use a private mesh (invite-token gated) or your own nodes.
- Blackboard posts are visible to the whole mesh. Do not post secrets.
- The above holds even though the transport is encrypted.

## Dependencies

Deliberately independent of the verification work
([#43](https://github.com/jimoto-no-llm/mesh-llm/issues/43),
[#44](https://github.com/jimoto-no-llm/mesh-llm/issues/44)): correctness and
confidentiality fail differently and should not share a schedule. This issue
being open is not an implementation commitment.
