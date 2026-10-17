# Offline runtime durability measurements

The ignored `conformance` measurement reuses its real Session/App Server driver,
production provider adapter, HTTP/SSE parser, Agent Loop, tool runtime, SQLite
store and exact historical request reader. Only the external Python provider is
scripted. The fake key is fixture text; the driver injects no real credentials.
The Python runner also strips credential-like environment variables.

Build ordinary optimized code (no instrumentation feature):

```sh
cargo test --release --locked --test conformance --no-run --message-format=json > build.jsonl
```

Keep the `conformance` executable named in Cargo's compiler-artifact output,
its SHA-256, and the benchmark revision before rebuilding. This executable links
the ordinary library, not the `cfg(test)` library with process-death/profile hooks.
All samples use fresh native stores under the output directory (choose the same
real filesystem for both revisions), a new runtime/driver process and a new
provider process. Do not run builds or other task-owned benchmarks concurrently.

```sh
python3 benchmarks/runtime/run.py --before /absolute/before-conformance \
  --output /absolute/evidence/before --pairs 1
# After implementation: three paired, interleaved repeats, every case.
python3 benchmarks/runtime/run.py --before /absolute/before-conformance \
  --after /absolute/after-conformance --output /absolute/evidence/paired --pairs 3
# Separate instrumentation; never mix these times with headline results.
python3 benchmarks/runtime/run.py --before /absolute/before-conformance \
  --after /absolute/after-conformance --output /absolute/evidence/trace \
  --pairs 1 --case short-small --trace
```

`short-small` seeds 8 ordinary turns and measures the next 32; `short-large`
seeds 64 and measures the next 32. Both include all seed costs separately.
`stream` delivers 128 chunks with explicit provider gates and 10 ms workload
pacing between them. `large` returns 32,768 text bytes in 128 chunks. `tool`
executes native Read and verifies the real result in request two. `cancel`
cancels at an observed provider gate and proves the connection closes and the
attempt settles without another request. No timing threshold judges correctness.

Every case asserts terminal ownership/uniqueness/order through the shared driver,
exact output, actual request count, paginated exact frozen request reconstruction,
and original canonical read-back after shutdown/reopen. Provider expectations
also assert ordered current inputs and prior replies. Failures retain logs and
process results and make the command fail; never drop a slow or failed sample.

Recorded windows: initialization, per-turn inbound acceptance, first consumed
publication, settlement, runtime unload, connection close, final writable
open/close, reopen with canonical validation, and filesystem cleanup. Historical
correctness reads are included in process wall time and the final process sample;
they have no separate wall-time field. The stream/cancel driver controls provider
gates before draining notifications, so its first-consumed-publication measurement
is **not** first emission latency. It must not be compared with short-response
publication latency. Process samples bracket the entire execution and verification;
per-turn CPU deltas distinguish seeded/measured work from final exact read-back.
`stat` ticks, `io` and `smaps_rollup` belong to runtime plus driver only, excluding
the provider process. Missing proc files are null, not zero. RSS/PSS are sampled,
not peaks. Files record both logical length and allocated blocks; WAL and SHM are
explicit files. The relay is joined after abort before driver drop. A read-only owner can leave
WAL frames after the writer closes, so a measured final writable open/close
completes checkpointing before cleanup; the harness asserts no WAL frames remain.
This maintenance step is identical in the corrected BEFORE and AFTER binaries.
The original Stage A binary and results remain preserved separately.

Strace observes OS system calls, not physical-media traffic. `pwrite64` arguments
and filenames distinguish SQLite, WAL and other files; `fsync`/`fdatasync` distinguish
barriers. `/proc/io` reports OS process accounting, not disk-device latency.
Owner counts, SQLite operations and syscall counts must never be conflated.
SQL statement and allocation counts are unavailable in this ordinary runner.
Existing opt-in in-crate request attribution can separately investigate inclusive
Source phases; those times overlap and are not independent CPU shares.

## Pi Durable

`pi.mjs` refuses a checkout other than
`90d863f7c1183b965f099855715d6e88c7b8a52c`. It uses the original Pi Durable Harness,
SQLite backend and local faux provider, checks ordered requests, completion,
answers and reopen. It never runs the coding-agent or JSONL backend.

```sh
node --conditions=source benchmarks/runtime/pi.mjs \
  /absolute/pinned-pi-checkout short-small /absolute/evidence/pi.json
```

Run `short-small`, `short-large`, `large`, and `stream` in fresh processes three
times. Dependencies come from the pinned checkout's lockfile. Pi's provider is
in-process and its streaming token pacing differs from rustX's explicit HTTP
gates. Pi also has a different retained-fact/Source/inspection contract. Thus the
comparison measures each product's actual work, not interchangeable execution or
proof of durability/feature parity. Pi's SQLite defaults are WAL/NORMAL, 1,000-page
autocheckpoint and a 5-second busy timeout; its close explicitly truncates the WAL.

File inventories are sequential observations, not atomic filesystem snapshots.
A temporary journal removed between `read_dir` and `metadata` is retained as
`vanished_during_scan` with null sizes; totals include only observed files. The
initial trace exposed this legitimate race and its failure is retained.
Hardware `perf` counters are probed without changing host permissions; refusal
is recorded, not bypassed. No allocation profiler is installed on the measurement
host. Neither unavailable metric is reported as zero.

The corrected Pi fixture (version 2) fixes faux chunk sizes: 256 characters with
microtask scheduling for unpaced replies, and 16 characters at 425 tokens/second
for streaming (Pi counts four characters per token). RustX streaming uses
17-character numbered chunks gated every 10 ms. The original Pi preliminary
runs used random default chunk lengths and a timer per unpaced chunk; retain them
as fixture evidence, but use only version 2 repeats for the final comparison.

`trace_summary.py` joins unfinished/resumed syscall records by PID and reports
completed calls, failures, successful returned write bytes and sync calls by path.
An unresolved count must be reported, not silently treated as complete accounting.
`summarize.py` derives CPU from `/proc/stat` user+system ticks and reports sampled
RSS/PSS, process write-byte deltas and all per-run values. CPU per turn is the
measured-window total divided by turn count, including driver/sample overhead;
it is not an exclusive runtime profile or an allocation count.
