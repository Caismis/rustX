# Active Trace inspector repair

The independently checked `c2caa5b61bd9bd29568103f2ec4046b33e2f0916` evidence packet fixes the correctness regressions of the rejected Trace-demand candidate. It does not establish Pi parity. Its actual browser workload exposes a substantial transfer of cost into explicit Trace repair. In three alternating pairs, large-history inspector wall time was 2.54 times the pre-Trace-demand `93ab467a0be8d37feb1ab34e0d6e87ade38eb8d0` baseline, Native CPU 2.03 times and browser CPU 2.24 times. These are medians of paired ratios. The separately reported log-mean intervals are not confidence intervals for those medians.

## Diagnosis

The frozen browser fixture uses the actual composer and Trajectory inspector, Native App Server and Workspace Host, and the same local HTTP/SSE fake provider. Small and large histories use respectively 8 and 64 warm turns, followed by 32 measured turns with short prompts and replies. No real model service or credential is used.

Inspection of all twelve archived short browser products found one checkpoint and one subscription per product. Repeated reconnect is not the cause. The candidate already coalesces its roughly nine Trace invalidations per turn into approximately one primary repair pass. However, each pass snapshots every loaded row into batches of eight lifecycle interests. Between 93 and 98 percent of Trace RPCs are these secondary batches; the large-history window reaches 480 rows. Their browser-facing round trips were typically 17–19 ms. That duration includes transport and scheduling and is not exclusive server CPU time.

Four separate instrumented products preserve raw Native and Chrome profiles. They are diagnostic and ineligible for timing comparisons. The large candidate Native profile includes 684 of 1,544 samples under `ClientInner::trace_page`, including 341 under pure checkpoint fitting. These are inclusive and overlap. Chrome's large candidate profile contains 40,326 non-idle main-renderer samples: 14,192 classified in Trajectory processing, 9,643 in protocol identity/field work and 6,956 in React. Classification uses a declared disjoint context priority; individual function counts remain inclusive. Idle, garbage collection and unmapped stacks are retained. Browser profile counts do not cover all Chrome processes.

The hot path repeatedly reconstructs equal row objects, recomputes inline native identity strings, and scans all records once per retry owner. Normal fixture rows carry inline native identities; these results do not justify caching deferred Source selectors or guessing an affected owner. Completed tool rows still require repair: terminal Journal evidence can precede a later canonical result binding.

## Projection change

An equal repaired record now retains its previous object identity, and an entirely unchanged record sequence retains its array identity. Equality covers the complete serialized record. Read cuts still advance independently; pending admitted detail readers keep their original ownership. Changed fields, canonical-only result bindings and positive lifecycle changes replace the row normally. Unknown tool outcomes remain unknown.

Retry placement groups requests by their native owner and step in one pass. It preserves the original owner order, the last owner envelope, predecessor matching and placement order. Weak maps reuse only inline Attempt and Step identity encodings; each lookup checks the actual inline ID. Deferred Source selectors remain uncached and authoritative.

Five focused regressions exercise equal-row/cut behavior, canonical-only tool binding, unknown outcomes, changed identity inputs and retry placement ordering. All 10,157 cache folds from six original candidate short products produce identical serialized public cache values and retry outputs with the original and changed implementations. This replay does not independently reproduce Native execution, browser scheduling or field I/O.

## Exploratory measurements

The unchanged fixture then produced ten successful, unprofiled exploratory products: two opposite-order large-history trios (baseline, c2 and projection change), one small-history trio and one changed-source detail/resync workload. Each source, production Web file and optimized Native executable is hashed. Native executables retain the ordinary optimized c2/baseline build identities. The changed Web source is identified by its c2 base plus patch SHA-256 `b3156cbdce1b825d359cfb6e908cd6032d2cc1cf3d9c2aed3fb382b8951a6acf`.

All ten producer oracles passed. The standalone readers validate numerical records and the 100-Hz process clock but trust those archived semantic flags. These small exploratory sets are explicitly ineligible for acceptance; no statistical intervals or Pi-browser comparison are claimed.

| Large-history metric | Projection / c2, pair 0 | Projection / c2, pair 1 |
| --- | ---: | ---: |
| Mean measured-turn UI and repair wall | 0.7974 | 0.6928 |
| Browser process CPU per measured turn | 0.8096 | 0.7238 |
| Native process CPU per measured turn | 1.0863 | 0.9403 |
| Lifecycle process-write bytes | 1.0021 | 1.0017 |
| Measured-phase Trace RPC count | 1.1028 | 0.9655 |

The median of the two wall ratios is 0.7451 and browser CPU 0.7667. Native CPU and process writes are approximately flat; Trace request volume did not decrease. Direct comparisons within these same trios still put the projection change at about twice the pre-Trace-demand wall time. The one small-history trio has projection/c2 wall 0.9157 and browser CPU 0.9245. These ratios are not multiplied by results from earlier stages.

The detail/resync product preserves the held original detail, exact-cut page equality, real read-tool success, an actual resync-required boundary after at least 4,097 publications, and the reopened request/canonical evidence. It contains 6,155 pressure-control RPCs and is not an ordinary one-turn chat latency measurement.

Native CPU includes the Rust workload driver. Browser process CPU is recorded separately and excludes the Node Workspace Host, provider and Playwright controller. Process-write counters are not physical device writes. Lifecycle counters include startup and cleanup; measured-window counters do not. Sampled PSS and JavaScript heap are not memory peaks or leak proofs. No forced garbage collection is used.

## Remaining work

The projection change reduces browser work without reducing full-window repair requests. A next change must reduce repeated bounded repair admission and checkpoint work while retaining every loaded row's refresh coverage, explicit read errors, retry behavior and original-cut readers. Merely skipping terminal rows is incorrect. The main Rust/Pi CPU and process-write gaps also remain.

Independent source review, exact-head FULL CI, fresh optimized source identities and matched final measurements are required before accepting a new performance claim. The original PR #485 branch is untouched and must not be merged without fresh explicit approval. The earlier writer's unavailable unpushed work is not represented as recovered.
