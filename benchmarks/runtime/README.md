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

Every case asserts terminal ownership/uniqueness/order, exact output and request
count. Oracle version 2 independently constructs the **complete ordered input**
for every request from the bounded workload: all prior user/reply pairs, the
first generated time context, and the Read call/result when present. The pinned
`expected_tools.json` is a normative fixture for the seven declared native tool
definitions, rather than an expected value learned from the current run.

The oracle compares captured HTTP bodies with an independent projection of the
reconstructed provider-neutral request, without calling the production adapter
translator. Wire/frozen equality preserves timestamps and workspace paths exactly;
only JSON argument spelling is canonicalized. Comparison to the independent
workload normalizes the strictly parsed generated UTC time and the one exact
fixture workspace path. It never drops history, messages, roles, tool schemas,
call/result linkage, model configuration or other semantic content. The bounded
workload expects one initial time contribution (the product refresh interval is
30 minutes). A second contribution fails rather than being silently removed.

Each frozen snapshot's identity, provisional Assistant ID, semantic fingerprint,
ordered Original references and Journal request-start fact must agree. After all
runtime owners close, reopening repeats the complete wire/expected/frozen checks
and compares the exact serialized snapshots and reconstructed requests with the
live values. Five negative regressions prove omitted/reordered old history,
frozen-content/time mismatch, changed tool schemas and wrong identity/reference
linkage fail. The server's cheap per-step substring guards are not the acceptance
oracle. All providers remain local scripted/faux processes.

The runner requires exactly one passing named measurement test plus a valid
version-2 sample, successful live/reopened oracle counts and every lifecycle
endpoint. Exit zero with no matching test, missing/malformed output or an invalid
oracle is a failed run. Logs, exit codes and validation errors are retained.
Run focused Python regressions with:

```sh
python3 -m unittest discover -s benchmarks/runtime -p 'test_*.py' -v
cargo test --test conformance --all-features --locked request_oracle
```

Recorded wall windows include initialization, per-turn acceptance/publication/
settlement, unload, driver drop, final writable open/close, reopened validation,
and cleanup. Ordinary process CPU/I/O endpoints now start **before fixture/runtime
initialization** and follow initialization, all turns, exact verification, shutdown,
driver drop, final writer open/close, reopened verification, cleanup and provider
teardown. Product lifecycle and product post-turn totals end at **after_cleanup**,
including shutdown, final writer checkpoint/close, reopened verification and cleanup.
Provider teardown is separate: Linux [`/proc/self/io`](https://man7.org/linux/man-pages/man5/proc_pid_io.5.html)
includes waited-for children, so that phase's I/O delta can include the provider's
entire lifetime I/O, not merely teardown work. Product I/O stops before this provider
reap (and still includes any other children already waited for). CPU uses self
`utime+stime` throughout and excludes child CPU; it does not share the I/O scope.
Product wall ends at cleanup; the separate teardown wall includes waiting for the
provider. OS/test-runtime startup and final result serialization/writing are outside
these endpoints. Missing counters remain null, not zero. Turn-only counters remain
separately labeled. Summary accounting version 3 derives these scopes from unchanged
version-2 raw samples; historical through-teardown summaries remain retained.


The earlier Stage C CPU/I/O window began after initialization and ended before
shutdown. It did **not** cover the whole lifecycle. Its wall windows and separate
short-small syscall trace already included deferred close/checkpoint work; the
version-2 correction supplies the missing ordinary phase CPU/I/O across all cases.
Old evidence is retained and identified as pre-review, not relabeled as complete.

The relay is joined after abort before driver drop. A read-only owner can leave
WAL frames after the writer closes, so a measured final writable open/close
completes checkpointing before cleanup; the harness asserts no WAL frames remain.
The same harness, oracle module and pinned tool fixture overlay both revisions.
Stream/cancel gates are controlled before notification draining, so their first-
consumed-publication figure is not first emission latency. RSS/PSS are sampled,
not peaks. Inventories record both logical length and allocated blocks.

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
