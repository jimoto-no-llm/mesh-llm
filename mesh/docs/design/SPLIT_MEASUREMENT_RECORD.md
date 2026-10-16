# Split Serving Measurement Record

First measurements taken with a working native runtime on a single Linux host.
These are **baseline numbers for one CPU-only machine**, not a performance
claim about GPU meshes or real networks. Raw material for
[#41](https://github.com/jimoto-no-llm/mesh-llm/issues/41) and the protocol in
[SPLIT_STRATEGY_SELECTION.md](SPLIT_STRATEGY_SELECTION.md).

## Environment

| Item | Value |
|---|---|
| Host | Fedora 44, kernel 6.19.10, x86_64, 16 cores, 31 GB RAM |
| GPU | RTX 3070 present, **not used** (no CUDA toolkit; runtime built with `backend=cpu`) |
| Build | `just skippy backend=cpu`, `just mesh` |
| Model | `qwen2.5-3b-instruct-q4_k_m.gguf` (36 layers) |
| Nodes | separate processes on the same host, distinct node keys and ports |
| Link | QUIC loopback, reported `RTT 0–5ms (direct)` |

## Method

One prompt, greedy decoding, identical for every configuration:

    Count from 1 to 40, one number per line, no other text.

`max_tokens=64`, `temperature=0`, three runs per configuration. The response
`timings` block supplies `prompt_ms` (time to first token),
`prompt_per_second` (prefill) and `predicted_per_second` (decode).

## Results

| Configuration | prompt_ms (TTFT) | prefill tok/s | decode tok/s |
|---|---|---|---|
| Single node (whole model) | 507–517 | 91–93 | 17.2–17.5 |
| Split, 2 nodes, coordinator API | 355–388 | 121–133 | 13.0–15.0 |
| Split, 2 nodes, via proxying node | 349–359 | 131–135 | 14.4–15.0 |
| Split, 3 nodes available | 395–534 | 88–119 | 11.2–12.3 |

Stage layout for the 2-node run: `stage-0` layers 0–20 on one node, `stage-1`
layers 20–36 on the other, both `ready`.

## What these numbers show

- Decode is **slower** with a split than on a single node (roughly 13–15 vs
  17–18 tok/s): the second stage costs a transfer per token.
- Prefill was **faster** in this CPU setup. Two processes each holding about
  half the layers appear to use cache better than one process holding all of
  them. This is a CPU-only effect on one machine and must not be generalised
  to GPUs without measuring there.
- Adding a **third node did not help** and did not create a third stage: with
  36 layers and this budget the planner kept two stages while the third node
  stood by. Its numbers were also taken while the topology was still
  rebalancing, so they are not comparable to the settled runs.

## Limits of this record

- Every node is on one host. Real RTT, relay paths, and per-node hardware
  differences are not exercised, so the slowest-node effect cannot be shown.
- CPU backend only. GPU placement, VRAM limits and expert sharding are untested.
- Per-stage execution time and activation transfer volume are not available
  from the status surface, so they are not reported here.
- Three runs is enough to see the direction, not enough for a variance claim.

## Reproducing

    just skippy backend=cpu && just mesh
    MESH_LLM_NODE_KEY_PATH=/tmp/n1.key ./target/debug/mesh-llm serve \
      --gguf /path/to/model.gguf --split --max-vram 6 --port 9337 --console 3131
    # second node: same model, own key, own ports/bind-port, --join-file <token>

The invite token is in the `token` field of `/api/status`; two processes on one
host need `MESH_LLM_NODE_KEY_PATH` each or the join is rejected.
