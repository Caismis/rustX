# Diagnostic attribution (not headline timings)

Use locally unpacked gperftools 2.16 and preserve its package URLs/hashes.
No system installation or security changes are needed. Compile the bridge:

```sh
gcc -Wall -Wextra -Werror -O2 -shared -fPIC \
  benchmarks/runtime/profiling/bridge.c -ldl -pthread \
  -o /workspace/issue-481-profilers/bridge.so
cargo test --release --locked --test conformance --no-run --message-format=json
```

Copy the resulting conformance executable to an immutable path and record its
SHA-256. The library build/features must match the ordinary release baseline.
For each mode (`control`, `cpu`, `heap`) use a fresh output directory:

```sh
python3 benchmarks/runtime/profiling/run.py \
  --binary /workspace/issue-481-evidence/attribution-conformance \
  --profiler-root /workspace/issue-481-profilers \
  --output /workspace/issue-481-evidence/new-cpu-small \
  --mode cpu --case short-small
```

`short-large` is the other supported case. CPU mode requests 250 Hz signal
samples, keeps the system allocator, and records separate phase files. Heap
mode replaces the allocator with tcmalloc and tracks every allocation; it is
extremely intrusive. Neither mode supplies headline timing/RSS/write samples.
Ordinary controls contain no profiler preload or boundary handshake.

The synchronous socket bridge rotates CPU profiles and dumps cumulative heap
profiles at named endpoints. Heap dump reasons are in the product log. For the
short cases, dumps 4 and 5 bracket measured turns. Analyze same-process deltas,
not differences between unrelated processes or medians:

```sh
nm -n -C /path/to/immutable-conformance > /path/to/symbols.txt
readelf -l /path/to/immutable-conformance
python3 benchmarks/runtime/profiling/analyze-heap.py --binary /path/to/immutable-conformance \
  /path/to/symbols.txt \
  /path/to/alloc.0004.heap /path/to/alloc.0005.heap > /path/to/steady-allocations.json
/path/to/google-pprof --text --cum --no_strip_temp --nodecount=200 \
  /path/to/immutable-conformance /path/to/before_measured_turns.cpu
```

The compact heap analyzer requires a PIE ELF whose first PT_LOAD virtual
address is zero. Confirm with `readelf`; the file-offset/virtual-address gap
of later segments must not be mistaken for an ASLR base. Nearest function
symbols have no inline expansion; generic identical-code folding may share
names/addresses. Validate aggregate totals against heap headers and selected
call sites against google-pprof before interpreting ownership.

Logical writes can be profiled with `syscalls.py --binary ... --output ...`,
then `profile-source.py OUTPUT/calls.strace`. The external controller attaches
a sibling tracer to its product child before releasing provider readiness.
Both engines still use the real HTTP/SSE provider; do not preload its process.
Keep profile runs separate from formal paired matrices. Report profiler
control, file-output and sampling overhead explicitly.
