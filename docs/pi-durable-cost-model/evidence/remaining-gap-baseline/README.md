# Inspectable baseline diagnostic inputs

These are the final complete diagnostic pair's analyzer, phase/database snapshots,
and raw before/after Pi SQL counters from #484's `remaining-pi-gap.tar.xz`.
The JSON values are unchanged; only whitespace is compacted to avoid duplicating
large pretty-printed Source metadata. `manifest.json` records both hashes.

The raw syscall traces and V8 CPU profile remain in that archive. To reproduce the
analyzer, extract `complete-rust/calls.strace`, `complete-pi/calls.strace` and
`complete-pi/pi-steady.cpuprofile` into this directory, then run `python3 analyze.py`.
No live product or provider is needed for analysis. `phases.json` exposes the
external boundary observations; the analyzer selects the actual socket phase
markers in the trace and rejects missing markers.

This is one instrumented pair, excluded from headline timing. WAL write commits
are not read-only transaction counts; page-owner snapshots cannot exclude every
intermediate page reuse. Rust's one unmapped frame remains explicit. SQL native
elapsed observations include tracer stalls. Cumulative CPU stacks overlap and
V8 samples are not directly comparable process-CPU percentages.
