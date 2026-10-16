# Inference Verification Trust Policy

Design record for [RFC #44](https://github.com/jimoto-no-llm/mesh-llm/issues/44):
what to do **after** a split-inference result is disputed. Detection itself is
[#43](https://github.com/jimoto-no-llm/mesh-llm/issues/43) (Proof of Inference
PoC) and is out of scope here.

Status: **design only**. No node is quarantined, penalised or evicted by the
code in this repository today, and this document does not add that.

## Goals

- Decide who may dispute a stage result, with what evidence, and within what
  deadline.
- Make the outcome of a dispute visible and reversible, with the evidence that
  produced it retained.
- Limit routing to a suspect node instead of jumping straight to permanent
  exclusion.
- Reuse the existing local reputation and block machinery rather than inventing
  a second one.

## Non-goals

- No economic penalty, staking or slashing. The RFC defers that until #43 shows
  that detection is even sound.
- No automatic permanent ban. A wrong detection must be recoverable without
  operator heroics.
- No global consensus on reputation. Trust state stays **local per node**,
  exactly like `NODE_REP.md`; nodes may disagree and that is acceptable.
- No change to the mesh wire protocol in this phase.

## Threat model

| Threat | Description | Handled by |
|---|---|---|
| Byzantine stage | A stage returns a wrong activation/result | Challenge + re-execution (this design) |
| False accuser | A node disputes an honest result to disrupt a peer | Evidence requirement + reviewer-side re-execution |
| Silent continuation | A replacement stage pretends the interrupted request continued | [#42](https://github.com/jimoto-no-llm/mesh-llm/issues/42) policy (fail loudly, do not fake continuity) |
| ID rotation | A quarantined operator restarts with a new endpoint key | Explicitly **not** solved; local state is per-endpoint |
| Sybil / collusion | Many identities vote to quarantine an honest node | Explicitly **not** solved; decisions are local and evidence-based, not majority-based |
| Evidence tampering | A party fabricates the material it submits | Evidence is bound to identities; see "Limits" below |

## Trust states

Three states, per remote endpoint, local to the observing node:

| State | Routing effect | Entered by | Left by |
|---|---|---|---|
| `unverified` | Normal, but never used as an *adjudicator* | Default for a newly seen peer | A passing re-execution, or enough clean exchanges |
| `verified` | Normal, may act as adjudicator | Clean observed exchanges / passed challenge | A sustained challenge, or evidence of a wrong result |
| `quarantined` | **Excluded from stage placement and routing** (existing block mechanism) | Confirmed wrong result, or a missed challenge deadline with evidence | Explicit review, or an expiry the operator sets |

Transitions are one step at a time and every transition stores: the state, the
reason, the evidence digest, the observing node, and a timestamp. Quarantine is
never silent: it is reported on the status surface (see below) and through the
existing event pipeline, so an operator sees it without reading logs.

## Challenge flow

1. **Detect.** A receiving stage (or the coordinator) observes a result that
   fails the #43 check. Without a detection signal there is no challenge, and
   normal operation costs nothing extra.
2. **Challenge.** The observer sends a challenge naming the disputed stage, the
   request identity, and the evidence digest it holds. The disputed node
   answers with the material it used (its own input digest and output digest).
3. **Adjudicate.** The observer picks one of the options below. Adjudication is
   local: the observer decides, and other nodes may decide differently.
4. **Record.** The outcome updates the disputed node's trust state and is
   surfaced with its evidence. `unverified` is the resting state; only a
   confirmed mismatch moves a node to `quarantined`.
5. **Review.** Quarantine carries an expiry and an explicit review path. A
   passing re-execution, or an operator decision, restores the previous state.

Timeouts: a challenge that goes unanswered within the deadline is recorded as
*unanswered* (a weak signal that lowers trust one step at most), never as proof
of guilt. The deadline should match the existing stage-control budget rather
than introduce a second timing model.

## Adjudication options

| Option | Cost | Latency | Notes |
|---|---|---|---|
| Local recompute of the disputed range | High on the observer, zero on the mesh | Slow | Only viable for small ranges or when the observer has the artifact |
| Ask k independent peers to re-execute | k× compute, spread across the mesh | Parallel, bounded by the slowest | Needs peer eligibility rules; k=1 in the first phase |
| Deterministic re-execution with a fixed seed | Moderate | Moderate | Requires the split to be seed-deterministic; not true for all sampling paths |

The design assumes **k = 1 in the first phase**, with the counter-evidence and
the reviewer's own findings retained so an operator can settle a disagreement.
Raising k is a later decision, gated on #43's detection quality.

## Integration with existing code

- Local reputation, cooldowns and blocks already exist: `NODE_REP.md`,
  `network/peer_blocks.rs`, `network/target_health.rs`. Quarantine should be a
  new reason on that path, not a parallel registry.
- Stage placement already excludes nodes with reasons:
  `api/split_readiness.rs` (`SplitReadinessExclusion`). A quarantined node must
  appear there with an explicit reason so `mesh-llm doctor split` explains it.
- Stage control already binds requests to identities
  (`mesh/stage_control_identity.rs`, and the accepted-claim check added for the
  `Stop` path). Challenge and response messages should reuse that binding.
- Stage loss reporting is described in
  [DESIGN.md](DESIGN.md) and implemented in
  `runtime/local_split/coordinator.rs`; a challenge is *not* a stage loss and
  must not withdraw the topology by itself.

## Limits this design cannot cover

- **Endpoint identity is not a person.** Quarantine is per endpoint key; a
  restart with a new key starts clean. Without external identity binding this
  is unprovable, and the design says so rather than implying otherwise.
- **Majority is not truth.** Because decisions are local, a colluding majority
  can quarantine an honest node *from their own routing*. That is a routing
  preference, not a network-wide verdict, and is reversible.
- **Evidence is claimed, not independently proven.** Digests bind a party to a
  claim; they do not prove the party ran the computation as declared. Closing
  that gap is precisely what #43 must establish first.

## Phasing

1. **Design (this document).** No behaviour change.
2. **Manual review.** Operator-initiated quarantine with the evidence surfaced;
   no automatic transitions. Requires #43's detection to be available.
3. **Semi-automatic.** One-step trust transitions from confirmed mismatches,
   with expiry and review.
4. **Economic penalty.** Only after 1–3 produce a defensible false-positive
   rate; explicitly out of scope today.

## Open questions

- What evidence can a receiving stage actually show for a *partial* result when
  the request was interrupted ([#42](https://github.com/jimoto-no-llm/mesh-llm/issues/42))?
- Should `unverified` peers be excluded as adjudicators (this design says yes)
  or merely deprioritised?
- What expiry is defensible for quarantine in a long-running public mesh?
- Do we need a status-surface field for "challenge outstanding", or is the
  event stream enough?
