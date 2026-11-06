# Final simplification: raw evidence and offline replay

Baseline e7289ac18, Statistics 35ae68b5f, final 3d5055779. No Pi/DeepSeek builds, engine products or performance reruns. Existing Web provenance only hashes pinned reference sources.

[Decision report](../../2026-11-04-final-simplification.md) · [Every value and adverse cell](../../2026-11-04-final-simplification-tables.md).

Three archives contain only this round:

- fixed.tar.xz: original four fixed pairs under shared load and the separate four-pair fixed retake, including every adverse product.
- rotating.tar.xz: four rotating pairs.
- validation.tar.xz: two deep pairs, one extended cache pair, independent hot matrices and one navigation retake, public Statistics/Request Source probes, WAL/page/column diagnostics, functional/reopen products, checks/build/source/host records, source patch, frozen inputs, rejected preflights and audit programs.

There are 30 native products / 3,360 consistent closed SQLite images. Replay checks canonical messages, full JSON request bodies/field order, cancellation/unload/reload, integrity/FKs and sequence/fact-length/payload bindings for 112,992 complete Statistics records. This is not a TCP-stream lexical byte comparison.

The original hot matrices have 64 products, the navigation retake 32, non-chat functional coverage 8, Statistics probes 24, Source probes 6 and WAL diagnostics 6. Different denominators are never pooled. Fixed/rotating have 4 pairs; deep 2; cache/Source/WAL are diagnostics, not timing-confidence cohorts.

Run standard Python 3 only, choosing a new output path:

~~~sh
python3 docs/pi-durable-cost-model/evidence/final-simplification-20261104/replay.py \
  --directory docs/pi-durable-cost-model/evidence/final-simplification-20261104 \
  --output /tmp/rustx-final-simplification-replay.json
~~~

The recorded successful replay authenticates 12,006 members, verifies 112,992 full-record bindings and reproduces every summary/table byte. Replay authenticates every archive/member and reruns frozen read-only audits without product/compiler/git/Node/network. It reproduces summary/table bytes. Only filesystem artifact basenames are normalized after extraction; wire/native values, IDs, schemas, counters and measurements are not normalized. Actual executables/libraries were checked before omission; replay authenticates stored byte proofs rather than rehashing absent binaries. Mutable runtime/workspaces, dependencies, duplicate provider stdout, authentication and build payloads are excluded. Baseline build metadata is reused, not old data archives.

Correctness is separate root-O0/all-features and default-debug browser helpers: 5,383 Rust passes / 12 existing ignored; 1,183 TUI / 1 skipped; 2,896 Web / 4 skipped; 41 dev; 48 Python harness tests; 323 browser tests. Fmt, all-target/all-feature Clippy, documentation tests, protocol regeneration/type checks and Web provenance/i18n pass. 2,220 tracked verification inputs match the final production-source subset; v52 schema/types/fixtures remain identical. No macOS or main-migration integration claim.

Shared-host interference is explicit. A foreign test wrote ~61 MB/s and caused severe I/O pressure; old-binary metadata creation/runtime latency also spiked. All polluted data remains. The one retake waited for compiler absence and low instantaneous device busy while global PSI remained high. Host-monitor coverage is incomplete for 5 native windows; raw/inline counters remain. One accepted product's parent ended 143 after closed images/completion; coordination UTC bounds were recovered from file mtimes, explicitly marked and never used as latency. No sample value was repaired or discarded.

CPU uses 10 ms PID ticks. Native latency excludes held first round; CPU/write includes it. Process writes are not device writes; sampled PSS/RSS is not live heap or exact peak. 32 s cache point is 128×8; 130 s has 4×14 and 124×8. Current-byte integrity and length certificates remain distinct from independent original-capture authentication.

Short-history CPU, single-concurrency latency, cold recovery and deep-history adverse values remain in the report. This is bounded simplification with retained resource benefits, not universal acceleration. Performance scope is frozen; main integration is separate.

[Manifest](manifest.json) · [Successful replay](replay-result.json).
