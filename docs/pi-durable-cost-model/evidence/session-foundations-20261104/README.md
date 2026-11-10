# Session foundations: raw evidence and offline replay

Production `57c7082ea63b82137c38ccd67e032565d9a1744d` against saved d50125842. Only rustX was run. The prior Pi 128 memory sample is copied as numeric points authenticated against its original audit; no Pi or DeepSeek runtime/build is included or executed.

Three archives contain this turn only: ordinary-fixed (eight primary processes), ordinary-rotating (eight), and diagnostics (two extended ordinary/cache, three CPU and two sampled-heap processes, plus common checks/functional/provenance). All 128 conversations have exactly eight turns at the seed endpoint. The 130 s cache point instead has four ×14 plus 124 ×8. Instrumented allocator/CPU products do not enter ordinary memory/timing medians.

The manifest authenticates every archive and member. Full structured HTTP reports, exact native histories, closed consistent SQLite backups, raw PID counters, complete CPU/heap profiles, symbolization, source/check/build maps, failures and frozen helpers remain. Provider-body comparison ignores no field and matches by session/turn rather than concurrent arrival order. Tables preserve all pairs/tails, including adverse cells.

Run with standard Python 3 only:

```sh
python3 docs/pi-durable-cost-model/evidence/session-foundations-20261104/replay.py \
  --directory docs/pi-durable-cost-model/evidence/session-foundations-20261104 \
  --output /tmp/rustx-session-foundations-replay.json
```

Use an output path that does not exist. Replay authenticates 9,884 members, independently reparses 23 native products /2,944 SQLite images, eight functional products, nine complete HTTP-body pairs, three CPU profiles and 24 complete 64 KiB heap profiles, then reproduces all tables exactly, including one final LF. It invokes only archived Python arithmetic/audits, no product/compiler/git/symbolizer/network. Old helper filenames referring to Pi are imported arithmetic functions, not engine runners.

Stored prearchive byte proofs cover deliberately omitted binaries, root libraries, profiler libraries and heap exporter. Replay validates those stored proofs rather than pretending it can rehash excluded artifacts. All 974 checked source/test/Cargo inputs and two capacity helpers match the fresh O3/default root build; correctness uses a separate root-O0/all-features build. All 22 groups pass: 4,379 unique tests /nine existing ignored. Target serde_json features are identical; the separate proc-macro host artifact is retained in the source audit. Full current source patch is included.

Raw heap headers/sums are complete and independently Poisson-corrected. Class-charged live totals, estimated requested allocations and ordinary GNU-allocator PSS are different denominators. Inclusive groups overlap; disjoint priority groups assign each allocation once and are not PSS components. CPU counts are finite samples, not exact attribution of the earlier fixed 64-call adverse cell. Shared-host compiler observations include interference and are not an exclusive-host certificate.

Mutable workspaces/live databases, dependency trees, authentication, duplicate provider stdout and executable/compiler/library payloads are excluded. The archived consistent backups remain complete. No historical archive was recopied. Full CI/browser E2E/main merge were not performed; PR485 remains draft with existing conflicts.

[Report](../../2026-11-04-session-foundations.md) · [All values](../../2026-11-04-session-foundations-tables.md) · [Manifest](manifest.json) · [Successful replay](replay-result.json).
