# Choosing Between Layer Split and MoE Expert Sharding

Research record for [Research #46](https://github.com/jimoto-no-llm/mesh-llm/issues/46):
how an operator should choose between a dense layer split and MoE expert
sharding. It records the mechanism as implemented, the measurement protocol, and
a **provisional** selection guide.

Status: **no measurements were taken for this document.** The guide below is
derived from how each mechanism works and from the project's own reported
experience, not from a benchmark run. It must not be quoted as a performance
result, and it deliberately avoids statements like "layer split is always
faster".

## What each mechanism does today

| | Layer split | MoE expert sharding |
|---|---|---|
| Unit of division | A contiguous layer range per stage | Overlapping expert assignments per node |
| Cross-node traffic | Activations at each stage boundary, per token | Zero cross-node expert traffic (assignments are local, routing is session-sticky) |
| Status in this repo | Supported; the standard split path | Implemented, described as **experimental** |
| Reported experience | Working path for dense and MoE families | `ROADMAP.md`: "most results show this doesn't perform as well as one would hope, more research is needed" |

Both are exercised for MoE families: `skippy/docs/FAMILY_STATUS.md` records MoE
expert-stage smoke passing for one-stage, split-middle, and split-final layer
ranges, so MoE models on the layer-split path are not a special case.

## What could not be measured here, and why

The issue's comparison needs the same MoE model run three ways (single node,
layer split, expert sharding) on more than one machine. That was not possible in
the environment used for this document:

- the native runtime cannot be built (no `just`/`sccache`; the host builds
  backend-neutral only), so no inference can be run at all;
- there is one host and no GPU, so node-to-node effects (RTT, relay paths,
  slowest-stage limiting) cannot be observed.

Per the issue, this is recorded rather than worked around. Numbers produced on
unrelated hardware would be worse than no numbers.

## Measurement protocol

Fix these before comparing anything:

1. One MoE model, one quantisation, identical prompt set and context lengths.
2. Per node: VRAM, memory bandwidth, network bandwidth, RTT, direct or relay
   path, and the layer/expert assignment actually used.
3. Report prefill and decode separately; single request and concurrent requests;
   short and long context. Do not average them into one figure.
4. For each configuration, record: intermediate activation volume, expert
   selection frequency, synchronisation frequency, the slowest node's share, and
   GPU idle fraction.
5. For small-VRAM participants (4 GB class), record separately whether the model
   fits at all, then what speed remains.

## Provisional selection guide

Read these as hypotheses with their invalidation condition, not as results.

| Situation | Provisional choice | Why | This is wrong if |
|---|---|---|---|
| Dense model that does not fit on one node | Layer split | It is the only split available; capacity is the constraint | — |
| MoE model, capacity-bound, little headroom | Layer split | Predictable per-stage cost; expert assignment needs headroom for the overlapping sets | Expert assignment fits comfortably and measures faster |
| MoE model, expert locality is strong, interconnect is slow or relayed | Expert sharding (experimental) | It moves no expert activations between nodes, where layer split pays per-token activation transfer | Routing/sync cost exceeds the saved transfer; today's reports say this is common |
| Many heterogeneous nodes | Layer split with ranges balanced by measured per-stage time | Throughput is set by the slowest stage, so balance matters more than node count | Node count is low enough that RTT dominates |
| High RTT or relay-only paths | Fewer, larger stages | Fewer boundaries means fewer per-token transfers, each of which is latency-bound | The larger stage no longer fits in memory |
| Small-VRAM nodes only | Layer split with small ranges; expect limited speed | Capacity can be met, speed is bounded by interconnect | The measured link is fast enough to hide the transfers |

Two generalisations to avoid, per the issue: neither mechanism is uniformly
faster, and "MoE is slow" is not a property of MoE but of a specific routing and
synchronisation cost.

## Next steps

1. Run the protocol above on 2–3 nodes with one MoE model and one dense model.
2. Publish the measurement table, including the configurations where the
   comparison was impossible and why.
3. Only then open a performance optimisation issue; do not mix implementation
   changes into the measurement.
4. Replace the provisional rows above with measured conditions, or delete rows
   the measurements do not support.
