# Proof of Inference: PoC Specification

Design and threat-model record for [RFC #43](https://github.com/jimoto-no-llm/mesh-llm/issues/43).
It specifies a lightweight, experimental verification PoC for split inference.

Status: **specification, not a proof**. Nothing here claims cryptographic
soundness. A cheap digest comparison is not a "cryptographic proof" and this
document deliberately avoids that language. Confidential inference
([#47](https://github.com/jimoto-no-llm/mesh-llm/issues/47)) is out of scope.

## Terminology

| Term | Meaning here |
|---|---|
| Integrity of artifacts | A package matches its recorded SHA-256. Already implemented. |
| Transport confidentiality | QUIC end-to-end encryption of the wire. Already implemented. |
| **Inference verification** | Evidence that a remote stage executed the *specified* computation on the *specified* input. This is what #43 is about, and it is not provided by the rows above. |

## Threat model

| Threat | Example | Cheap detection |
|---|---|---|
| Value tampering | Stage returns perturbed activations | Sampled recompute / residual comparison |
| Zero or random output | Stage returns zeros or noise instead of computing | Receiver anomaly check (all-zero, non-finite) |
| Replay | Stage resends a previously valid activation for a different request | Nonce/request binding in the challenge |
| Wrong-layer computation | Stage applies the wrong layer range | Plan-time boundary validation (already present) plus boundary re-assertion in the challenge |
| Simple computation error | Wrong dtype/scale from a misconfigured node | Tolerance-based comparison |
| Collusion | Two stages agree on a wrong result | **Not detectable** by this PoC |

## What already exists, and why it is not verification

- **Plan-time boundary validation**: the topology planner rejects invalid
  boundaries and errors (`inference/skippy/topology.rs`), so a *plan* cannot
  name mismatched layer ranges. This constrains configuration, not runtime
  behaviour.
- **Artifact digests**: packages are validated against recorded SHA-256. This
  binds bytes at rest, not the computation performed.
- **Stage control identity**: claim/load/stop are bound to the authenticated
  QUIC peer (`mesh/stage_control_identity.rs`). This says *who* asked, not
  whether the answer is correct.
- **QUIC**: protects the path; the computing peer still sees plaintext.

## Reproducibility conditions

A challenge is only meaningful when the disputed computation is reproducible.
The PoC must therefore pin and record:

1. model id, package ref and package SHA-256;
2. the exact layer range of the disputed stage;
3. the token position and the request/step identity;
4. the input activation (or its digest plus the sampler's indices);
5. KV-cache correspondence for the token position;
6. the numeric policy: dtype, quantisation differences, and the comparison
   tolerance;
7. the sampling policy, and whether the path is deterministic for the seed.

Where any of these is unavailable, the PoC must report **undetermined** rather
than a match. Sampling randomness and quantisation differences are the two most
likely reasons a genuine node fails a recompute, so they must be settled before
any detection rate is quoted.

## Candidate detections, by cost

| Candidate | Cost when idle | Cost when challenged | Detects | Notes |
|---|---|---|---|---|
| Receiver anomaly check (all-zero, non-finite, shape/version mismatch) | ~0 | ~0 | Zero/random output, obvious faults | Cheapest; runs on every exchange; no verification claim |
| Sampled canary (probe a small deterministic segment) | ~0 | Small | Value tampering on the sampled range | Detection probability scales with sample size |
| Selected-range recompute by the receiver | ~0 | High on one node | Tampering in the recomputed range | Needs the artifact and an idle slot; blocks nothing if run off the critical path |
| k-node residual comparison | ~0 | k× compute spread over peers | Tampering unless k collude | Needs eligibility rules; k=1 in phase 1 |

Latency rule: verification must not sit in the synchronous decode path. The PoC
evaluates off-path verification (idle resources or a separate task) and reports
when that is impossible. These are hypotheses to measure, not measured results.

## What the PoC must state honestly

- False negatives against colluding nodes.
- False positives from quantisation, sampling, or KV divergence, and the
  tolerance that produced them.
- That a passing check is evidence about the sampled range only.
- That nothing here proves a node ran the full computation.

## PoC plan

1. **Opt-in only**, on a trusted test mesh, behind an explicit flag.
2. Instrument the existing stage path to record the reproducibility fields
   above per exchange.
3. Implement the anomaly check first (it is cheap and catches broken nodes even
   if verification is never trusted), then the sampled challenge.
4. Measurement matrix, on 2–3 nodes with one small model:
   - detection rate: honest node vs a node that tampers with the sampled range;
   - false-positive rate: honest repeats across restarts;
   - added compute and memory per challenge;
   - TTFT and decode tok/s with verification off, on the idle path, and
     synchronously (to show the synchronous cost is unacceptable).
5. Compare the two-node doubling of compute against receiver-side verification
   and record what neither can prove.

Steps 4 requires real nodes and a native runtime; the specification above is
what a contributor needs in order to run it.

## Go / No-Go

Proceed to a real verification design only if the PoC shows a detection rate
that is useful at a tolerance that keeps false positives near zero on honest
nodes. Otherwise the honest conclusion is that split inference can detect
*misbehaving* nodes but not *lying* ones, and #44's quarantine policy must not
be automated.
