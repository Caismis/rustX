# DeepSeek Harness performance reference

This reference runs the official `deepseek-ai/deepseek-harness` checkout at
`687f282e8625c6b8d11ff58211a90524be2e4317` (`0.2.1-rc.2`) through its shipped
`dsh --profile sdk` application. The fixture uses a real agent loop, the
production pi-ai OpenAI-compatible HTTP/SSE provider, native Zstd JSONL session
persistence, projection caching, and native session resume. It does not replace
the store or provider with an in-process fake.

The additional `sdk-minimal` configuration is a separate diagnostic reference.
It uses uncompressed JSONL and omits the checkpoint policy and projection cache.
Results must keep these configurations separate.

## Completion and durability

`session/wait` returns after the agent is idle and notifications have been
written. SDK notifications contain committed session events and status changes;
they do not publish partial token/text updates. The `stream` fixture still
delivers all 128 HTTP/SSE deltas, but the SDK forwards the final assistant
message. This surface differs from rustX/Pi live partial observation.
It does **not** await a final session flush. The full SDK checkpoints
the logged request prefix before a model request and before the next step.
Its projection cache starts a flush on `turn/end` without awaiting that flush.
The backend also drains live events on a 200 ms timer. Its append path awaits
filesystem sync, and shutdown attempts to drain and close remaining writers.
However, the pinned SDK's `disposeAndExit` awaits `Promise.allSettled` on root
disposal and exits zero even when disposal rejects. Exit zero is therefore not
a positively acknowledged sync-success certificate. Complete closed-log reads
and native resume prove readable/reconstructed content, not crash or power-loss
durability.

Consequently, SDK idle latency and writes observed at idle are not equivalent
to rustX/Pi durable settlement. The reference records them as native API
completion costs, not equal-durability speedups. Native shutdown/exit cost and
`wait4` resource usage are retained separately. After close, a second native
process resumes the session and issues one verification-only request against a
separate sibling provider. That request must contain every preceding prompt
and response in order. The extra 32 KiB reply is outside the measured turns.
Complete lifecycle costs are not ranked against rustX/Pi because this additional
verification and source-launch startup differ.

The exact pinned implementation locations are:

- `packages/sdk/server/src/server.ts`: `prompt`, `wait`, and `resume`.
- `packages/session/session-checkpoint-policy/src/index.ts`: awaited request
  prefix checkpoints.
- `packages/session/session-projection-cache/src/index.ts`: asynchronous
  `turn/end` checkpoint.
- `packages/session/session-persistence-jsonl/src/storage.ts`: 200 ms live-event
  buffering, flush, and close.
- `packages/session/session-persistence-jsonl/src/index.ts`: compressed append
  and filesystem sync.

## Workloads and assertions

Supported cases reuse the existing sibling provider unchanged: `short-small`
(8 seed + 32 measured turns), `short-large` (64 + 32), `large` (32 KiB), and
`stream` (128 gated deltas with the existing 10 ms pacing). The SDK has no
mid-turn cancel API. Its native text `read` tool requires `file_path` rather than
the shared fixture's `path`; the minimal profile also omits text read. Those two
cases are explicitly unsupported by this adapter, not counted as successes.

The history oracle independently specifies the ordered benchmark prompts and
scripted replies. It compares every HTTP request, every assistant output, SDK
event notifications, the closed native log, its unchanged prefix after resume,
and the verification continuation. It rejects missing/reordered old messages,
changed outputs, native prompt or schema drift, and invalid reopen evidence.
The one native `session/end-seed` marker appended during resume precedes the
SDK's subscription; it is checked explicitly rather than treated as a missing
notification.

`deepseek-sdk-envelope.json` is a **reviewed capture**, taken from the pinned
full SDK in an empty workspace with an isolated `DSH_AGENTS_HOME`. It fixes the
native system text, runtime-context template, and 25 tool definitions. It is
not an independently authored specification of DeepSeek's native prompts. Only
the two exact workspace occurrences in that context become `<WORKSPACE>`;
arbitrary native content is never removed. The independently specified history
is kept separate from this captured envelope. Full cross-engine request
equivalence is explicitly false: native system/context/tools differ, and the
SDK merges the first prompt and reminder into one user message.

## Setup and execution

Keep the reference checkout isolated from the user's checkout. Check out the
exact revision, run `pnpm install --frozen-lockfile`, then
`pnpm run build:native-system` for the matching POSIX flock addon. The adapter
asserts a clean tracked tree and records source tree, lockfile, loader, native
addon, and adapter hashes. No production API key is used.

The supported source launch is `node --import tsx/esm apps/cli/src/bin.ts` with
the CLI's tsconfig. For `sdk`, the adapter passes the same shipped
`apps/cli/src/sdk-source.cordis.patch.yml` that the official TypeScript SDK's
source-launch resolver uses. That patch disables the unavailable generated
Typert loader. This is a supported source profile, not a built release
executable. Transpiler startup/cache costs remain a limitation for cold-start
comparisons. Telemetry is disabled and Harness/agent homes are isolated; native
profile tools, permission policy, persistence, and checkpoints remain enabled.

From the rustX repository root:

```sh
python3 benchmarks/runtime/deepseek-reference.test.py
python3 benchmarks/runtime/deepseek-reference.py \
  --checkout /path/to/pinned-deepseek --output /new/functional-output --profile sdk
python3 benchmarks/runtime/deepseek-compare.py \
  --rust /path/to/ordinary-release-conformance \
  --pi /path/to/pinned-pi --deepseek /path/to/pinned-deepseek \
  --output /new/rotating-comparison --repetitions 3
```

The comparison rotates Rust/Pi/DeepSeek order across three repetitions and
preserves failed runs. A rotation enters `metrics.json` and aggregate summaries
only when all three product validations and the Rust/Pi cross-oracle pass.
`product-metrics.json` separately preserves individually validated products,
including products from a rejected rotation. Rust/Pi retain their existing live-subscriber oracle;
DeepSeek uses its native mandatory SDK notification surface. The summary
reports per-engine medians/ranges without equal-durability ratios.

Raw `/proc` stat, I/O and `smaps_rollup` snapshots are retained for the native
CLI and extant children reachable through main-thread children lists. The four
functional cases had only the native CLI process at their sample points;
arbitrary descendants spawned by other threads are not claimed as covered.
CPU self and waited-child counters remain distinct.
Changed PID/start-time sets or decreasing counters make a window metric
unavailable with a reason. Sampled process-attributed writes are not device
I/O or a complete process-tree I/O total; closed-process `wait4` counters remain
separate. Memory is sampled PSS, not a peak. The Python controller and sibling
providers are excluded from CLI counters, while existing Rust/Pi counters
include their in-process drivers. SDK framing/notification wall time remains
inside the measured native call. Visible file prefixes after idle are recorded
only as visibility observations, never proof that a sync completed.

`deepseek-read-log.mjs` uses the pinned backend's frame scanner and decoder to
read **every** concatenated Zstd frame after close. This verification process is
separate from the measured CLI; native resume then independently exercises the
restored history through another real model request.
