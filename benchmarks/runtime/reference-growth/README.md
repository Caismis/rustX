# Reference session growth benchmark

`run.py` drives unmodified pi-rust CLI RPC and the real Pi Durable `Harness`,
production `openAICompletionsApi` HTTP/SSE adapter and file-backed Node SQLite.
It never writes into the two reference repositories. `pi-imports.mjs` resolves
workspace packages to the reference's actual TypeScript source, and external
dependencies to an existing installation. It does not substitute a provider,
storage implementation, session reducer or fake task scheduler.

```sh
taskset -c 30 python3 benchmarks/runtime/reference-growth/run.py \
  --output /absolute/new-output-directory
```

The default is one session, 512 sequential turns, 32-turn windows, both engines
and both scenarios. `--engines pi-durable pi-rust --scenarios tool text` reverses
the order for a repeat. `--reopen 32 128 512` closes and restarts the actual
product between windows; pass bare `--reopen` for uninterrupted residency.
The authenticated existing pi-rust release binary is reused, without a rebuild.
Paths, revisions and the SHA-256 pin are explicit in `run.py`; use `--help` for
path overrides. Pi source and the external dependency installation are separate
options; the default dependency tree matches the reference lockfile for loaded
packages (its `openai` is 6.40.0, despite a newer range in package.json).

## Workload and observation

The workload matches sibling `../turn-growth.py` as observed on 2026-11-10:

* user content `turn growth`, model `turn-growth`, context window 1,000,000,
  output limit 4,096;
* `text`: `Stream(Text('ok'), Finish(), Usage(16, 1))`, one content delta;
* `tool`: first `Stream(ToolCall('read-%08d', 'read',
  '{"path":"fixture.txt"}'), Finish('tool_calls'), Usage(16, 1))`, then the text
  stream above. Each product executes its own real `read` tool against the
  same 18-byte `fixed tool result\n` file;
* same repository Fake Provider codec and server, deterministic SSE bytes and
  flush boundaries, `Connection: close`, no artificial delay or gates;
* complete model history is retained and resent every request. Provider checks
  user/assistant/tool counts and every historical user/answer/tool result. It
  records compact request lengths/hashes and canonical SSE chunk lengths/hashes;
* native tool catalogs and system prompts are retained. Pi Durable installs its
  four `CodingTools`; pi-rust and rustX have their own larger catalogs. This is
  equal conversational work, not an identical native request envelope;
* pi-rust terminal: RPC `agent_settled`, after `agent_end`, successful outcome,
  expected streamed text and exactly one tool completion when applicable;
* Pi Durable terminal: `submission.wait()` done plus consumed `watchEvents`
  `run_end`. The committed message/tool events are validated. The observer keeps
  only counters/current-turn validation, rather than retaining copies of all
  emitted versions. Native Harness/view retention is included;
* full stored transcript counts are checked before close and after every
  reopen, outside measured windows. Pi scans all entry pages, not just the
  first 1,000 records. Final counts must be 512 user + 512 assistant for text;
  512 user + 1,024 assistant + 512 toolResult for tool.

## JSON schema (aligned with Agent A)

`identity.json` records source trees, clean tracked trees, executable hash,
Node hash/version, dependency manifest hashes/versions, input hashes, affinity,
workload and sampling settings. Each `<engine>-<scenario>/` contains:

| File | Contents |
| --- | --- |
| `turns.json` | One row/turn: `turn` (1 based), `settled_ms` (controller submit -> received terminal), `accepted_ms` |
| `windows.json` | One table row/32 turns; fields below |
| `startup.json` | Fresh empty-session startup and loaded process sample |
| `reopen.json` | Fresh-process restore at 32/128/512 turns; total `startup_wall_ms`, `loaded` CPU/RSS/PSS/I/O, `argv`, native readiness info and verification |
| `closes.json` | Close wall time, pre-close sample, whole-process `wait4` CPU/blocks/peak RSS and post-close footprint |
| `provider.json` | Complete-history oracle, request count, per-request byte length/hash and canonical response chunk lengths/hashes |
| `result.json` | Accepted flag, the above results, OLS slopes and transcript oracle |
| `process-*.stderr` | Unmodified product diagnostics for each process generation |

`windows.json` fields:

| Field | Unit / definition |
| --- | --- |
| `turn_start`, `turn_end`, `turn_midpoint`, `turns` | Same meanings as Agent A's `turn-growth.py` |
| `wall_ms_per_turn` | Arithmetic mean of `settled_ms` |
| `wall_p50_ms`, `wall_p95_ms` | Sorted latency at zero-based `min(n-1, floor(n*q))`; same p95 convention as A |
| `cpu_ms_per_turn` | Window delta of **product PID only** `/proc/PID/stat` utime+stime, / turns; 100 Hz (10 ms) counters |
| `wchar_per_turn` | Window `/proc/PID/io` wchar delta / turns, bytes; includes outbound HTTP/RPC and persistent write syscalls |
| `write_bytes_per_turn` | Window `/proc/PID/io` write_bytes delta / turns, bytes; kernel-attributed storage writes, not physical device/SSD traffic |
| `rss_bytes`, `pss_bytes` | Product `/proc/PID/smaps_rollup` Rss/Pss at window end |
| `session_bytes`, `session_allocated_bytes` | Sum of logical file sizes / st_blocks*512 under storage only; excludes workspace, controller logs and provider evidence |
| `sqlite_main_bytes`, `sqlite_wal_bytes` | Pi `.db` and `.db-wal` sizes; zero for JSONL pi-rust |
| `process_generation` | 1 initially, incremented at each checkpoint reopen |
| `before`, `after` | Raw /proc counter text plus parsed counters, complete file inventory and monotonic/epoch timestamps |

The top-level `result.json` has `accepted` and `results[]`; each result identifies
`engine` and `scenario`, with `windows`, `reopen`, `closes`, `slopes`. Slopes use
OLS against `turn_midpoint`, in metric units per additional history turn, and
include `intercept` and `r_squared`, matching A's naming. RSS/PSS slopes are
resident bytes per history turn, not memory allocated per turn.

`summarize.py` combines completed repeat directories into `summary.json` and
`windows.csv`. It adds provider request body bytes/turn, byte residuals, per-run
and across-repeat summaries, and steady-window slopes excluding the first
window after each process start. Full-window fits remain available; restarts,
JIT and lazy provider imports strongly distort CPU/latency fits if mixed.

```sh
python3 benchmarks/runtime/reference-growth/summarize.py \
  --runs /path/run-1 /path/run-2 /path/run-3 --output /path/summary
python3 benchmarks/runtime/reference-growth/audit.py \
  --runs /path/run-1 /path/run-2 /path/run-3 --output /path/summary/validation.json
```

`audit.py` rechecks window arithmetic, measured adapter hashes, provider chunk
identity and closed SQLite integrity without rerunning either product. Restore
summaries include the first subsequent user turn's latency at histories 32/128;
there is no extra 513th turn after the final restore.

## Measurement limits and durability

Every measured product is launched through `taskset -c 24-27`; sibling provider
through `taskset -c 31`; controller is pinned to 30 by the example invocation.
`TOKIO_WORKER_THREADS=4`. These are physical E cores on this host. Other agents
remain active; affinity does not reserve cores or isolate shared LLC, power,
filesystem, kernel writeback, or memory bandwidth. Compare repetitions and CPU
as well as wall time. Node's background threads and GC are included. The fake
provider retains growing request bodies in a separate PID and is excluded.
Agent A's observed default affinity is CPUs 2/8 (P cores); exact wall/CPU
multipliers require a rerun on the same core class. The I/O byte counts remain
useful without frequency normalization. A also includes per-turn retirement
`thread/read` probes in its resource interval; these references do not issue
an additional inspector/state read after each terminal.

CPU/I/O windows include the default 0.25 s **end-of-window** settle interval,
but wall latencies do not. Full verification, close, startup and restore are
excluded from per-turn counter windows. Restore verification happens before
the next measured window and can warm the runtime/database cache. Per-process
reopen includes runtime/module startup, storage restore and observer readiness;
the OS page cache is warm. Pi's `info.open_and_subscribe_ms` excludes module
loading and records the native open/root/watch span. The asynchronous
`before_open` process sample is diagnostic, not an exact barrier. pi-rust
readiness is a successful `get_state`, which includes native stats/context
resolution. These are useful operational endpoints, not identical API calls.

Pi uses the unmodified adapter defaults: SQLite WAL/NORMAL, 4 KiB pages,
1,000-page auto-checkpoint, 5 s busy timeout; explicit TRUNCATE checkpoint on
close. pi-rust uses its normal JSONL and progress companion append/recovery
path; the OS append implementation calls `write_all` then `flush`, without
per-append `sync_all`/`sync_data`. No stronger fsync policy is added, and no
durability option is disabled. A crash/reopen transcript check is not evidence
of equal power-loss guarantees. A window can charge an occasional checkpoint
or progress compaction; compare several windows and close accounting too.

This benchmark does not imply constant end-to-end CPU or memory. All products
resend complete history through stateless Chat Completions, requiring growing
serialization/network work. Pi keeps growing context/view arrays; pi-rust may
rehydrate and authenticate cold payloads before provider serialization. An
approximately flat append write volume does not prove flat request preparation
or RSS/PSS. rustX also persists native journal, frozen request and independently
inspectable Trace/Statistics/old cuts/replay surfaces which these paths do not
provide with identical contracts. Pi has durable tasks/submissions and native
views; neither reference enables a rustX Inspector equivalent here.
