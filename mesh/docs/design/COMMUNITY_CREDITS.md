# Community Credit Rules

Design record for [RFC #45](https://github.com/jimoto-no-llm/mesh-llm/issues/45):
an **opt-in, non-monetary** internal credit for nodes that contribute compute.

Status: **design plus a simulation**. No production accounting exists. This
document does not introduce a token, a sale, or a monetary value of any kind.

## Scope

| In scope | Out of scope |
|---|---|
| Rules for earning and spending internal credit | Crypto assets, token issuance, trading |
| Abuse cases and their defenses | Cash value or redemption |
| Ledger placement options and their trust boundaries | Changing the free default mesh behaviour |
| Audit and privacy requirements | Enforcement across meshes |

The default mesh stays free. Credits apply only to meshes that explicitly opt in.

## What counts as a contribution

A contribution is one completed unit of verified work:

| Signal | Why it is used |
|---|---|
| Tokens actually processed | Base unit of work |
| Model class | A larger model costs the contributor more per token |
| Stage role (first / middle / last) | End stages do more work than a middle slice; the split is not symmetric |
| Completion status | Failed work earns nothing |
| Verification state | Unverified work earns nothing (see #43/#44) |
| Requester identity | Blocks self-dealing |

Deliberately **not** used: raw self-reported token counts as the authoritative
record, wall-clock time reported by the contributor, or GPU model name (a fast
GPU does not mean more useful work).

The weights live in one place and are shared, so any node can recompute a
balance from the same inputs. In the simulation they are
`MODEL_WEIGHTS` and `STAGE_WEIGHTS` in
[`mesh/evals/credit_ledger_simulation.py`](../../evals/credit_ledger_simulation.py).

## Issuing, spending, expiring

- **Issue**: on a verified, completed contribution, per the weights above.
- **Spend**: consumed by inference requested on the same mesh. A balance can
  never go negative; an unaffordable request is refused, not partially filled.
- **Expire**: unused issued credit expires after a TTL (30 epochs in the
  simulation) so a balance cannot be hoarded indefinitely.
- **Opt in**: a node that does not opt in neither earns nor spends, and sees no
  behaviour change.

## Abuse cases and defenses

| Abuse | Defense in the rules | Simulated case |
|---|---|---|
| Self-loop (a node requests work from itself) | Requester == contributor earns nothing | `test_self_loop_earns_nothing` |
| Failed job | Only completed work earns | `test_failed_job_earns_nothing` |
| Duplicate report of the same job | Job ids are single-use; a repeat earns nothing | `test_duplicate_job_report_is_paid_once` |
| Unverified claim | Verification state gates issuance | `test_unverified_work_earns_nothing` |
| Inflated totals | Balance is recomputed from entries, not trusted from the reporter | `test_balance_is_recomputable_from_the_entry_log` |
| Overspend | Refused when the balance is insufficient | `test_overspend_is_refused_and_never_goes_negative` |
| Hoarding | TTL expiry, applied once | `test_expiry_removes_unused_credit_once` |
| Sybil (many identities) | **Not solved.** Credits are per identity; see "Limits" | — |
| Collusion to farm credit | **Not solved.** Needs #43's detection to be sound first | — |

Run the simulation and the cases:

```bash
python3 mesh/evals/credit_ledger_simulation.py
python3 -m unittest mesh.evals.test_credit_ledger_simulation
```

## Ledger placement

| Option | Cost | Trust boundary | Verdict |
|---|---|---|---|
| Node-local ledger | Lowest | Only the local node sees it; no mesh-wide spending | Enough for a simulation, not for shared spending |
| Mesh-distributed ledger | Replicated state, conflict handling | Peers must agree; Sybil resistance is unsolved | **Assumed for phase 2**; needs a concrete replication rule |
| External ledger / chain | Highest, plus a new dependency | External operator becomes the authority | Not required. The RFC asks not to start with a chain |

The simulation implements the accounting rules only, with no replication, which
is the part that must be right before any placement decision matters.

## Privacy and audit

- Entries record: entry id, kind, node id, amount, epoch, reason, job id.
- They never record prompts, model input or output, or personal data.
  `test_entries_carry_no_prompt_or_content_fields` pins the field set so a
  future change cannot quietly add content.
- Every balance is recomputable from the entry log, so a disagreement can be
  replayed rather than argued.

## Operational and legal notes

- The Ops cost is the verification needed to mark work verified; without it,
  issuance is just self-reporting. That dependency is why this design cannot be
  implemented before #43 shows whether verification is sound.
- Running a credit scheme publicly raises terms-of-service, tax and consumer-law
  questions even when nothing is sold. Those are owner decisions, not engineering
  ones, and are out of scope here.

## Go / No-Go

**Go** only when: verification (#43) is sound enough to gate issuance; a
replication rule exists for the chosen ledger placement; and the owner has
decided the legal/ops questions above.

**No-Go** while issuance would rest on self-reported numbers, or while the
scheme would apply to the default mesh.
