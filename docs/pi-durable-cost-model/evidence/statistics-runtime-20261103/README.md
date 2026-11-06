# Statistics history evidence

[Report](../../2026-11-03-statistics-history.md) · [Every paired value, adverse result and inspection product](../../2026-11-03-statistics-history-tables.md).

Baseline is f78d1b51a (schema 89), with its prior ordinary O3/default-feature binary reused explicitly: actual compiled source 09e388f8 has identical src/tests/Cargo inputs. Initial candidate ecf0efa00 and final candidate c9d5f73e5 (schema 90) each have clean ordinary O3/default-feature build records. Correctness root-O0/all-feature artifacts do not supply performance products.

The initial candidate's adverse batch is retained separately. A normal-batch fold-invalidation issue was then corrected with monotonic-prefix checks; its new measurement is in `refined/`. Counters from the two candidates are not pooled.

- Explanation stage: 40 **diagnostic-only instrumented** products. Eight stream products contain 64 requests with phase CPU/counters; 32 A/B/A or threshold products distinguish schema/plan initialization from per-event work. Patches/builds/instrumentation are retained and excluded from ordinary headline timing. Earlier stream reversals remain valid observations; the longer experiment does not uniquely attribute their cause.
- Initial candidate: 32 formal products / 16 pairs / 1,088 matched requests, 12 Statistics products, 16 ordinary threshold products, four separately traced threshold products, eight columns/WAL products = 72.
- Final candidate: 32 formal products / 16 pairs / 1,088 matched requests, 12 Statistics products, eight columns/WAL products = 52. Source threshold implementation is byte-identical between candidates, so its 20 products are not rerun in the final phase.
- Total 164 products have different purposes. They are **not** 164 equivalent performance samples. Every formal cell has four balanced interleaved pairs, pinned CPUs 2,8, and descriptive statistics only. No confidence interval, WebUI/AppServer transport or Pi-parity claim.
- Final 16-command validation: 2,372 distinct passed, zero failed, one existing ignored; Fmt and release/all-target/all-feature Clippy deny-warnings pass. Final source maps match 970 src/tests/Cargo inputs in the ordinary final build. Earlier failed checks and the initial candidate's 2,371 successful tests remain separate.
- Statistics old cuts use a checkpoint plus at most 15 suffix facts. Read-only probes preserve exact public values/reopen; concurrent probes mix 32 appends with 64 fixed old-cut reads. First means first API access in a process, not reset OS disk caches. CPU endpoints are 10ms. Process-wide write counters during concurrency include the writer.
- WAL is parsed separately from logical SQL column bytes and process write_bytes. No new state row/table/index or background writes. Ordinary hot write commits remain five per turn. Every retained WAL header/payload pair is checked by the strict parser.
- Pi and DeepSeek are not executed. No fresh ratio uses old unrestricted-CPU data.

The frozen initial auditor used the old pre-schema89 Source column name for its baseline diagnostic and stopped with KeyError. The original script/failure remain. `audit-results-corrected.py` changes only that lookup to the column present in both schemas; exact hashes/replacement are in `postmeasurement-corrections.json`. No product reran to fix the audit, and no formula/parser/product/source changed. The final phase froze the corrected lookup before measurement.

The archive contains regular retained records, source patches and identities, all build/check failures and successes, scripts, provider/observer/input proofs, raw process counters, column snapshots, WAL traces, inspection results/databases and accepted audits. Private directions, source checkouts, executables, rlibs, environments, symlinks and bytecode are excluded. Absent binaries are identified by retained build records; replay does not freshly verify or execute them.

`raw.tar.xz` contains **1,276 regular payloads**, **9,210,420 compressed bytes**, SHA256 **`e7385339cc7d9c465a073705b5c2d346fb39f243ae63bb137f62920367b150c3`**. `manifest.json` records every member length/hash.

Portable replay verifies every member length/hash and safe regular path, rederives the explanation counters, reparses all four WAL summaries and recomputes both complete candidate audits. Recomputed data must exactly match retained values, apart from the audit's raw-only flag. It never executes products, compilers or network calls.

```sh
python3 replay.py --archive raw.tar.xz --manifest manifest.json --output replay-new.json
```

Full repository CI/browser E2E and a main merge were not performed. Existing main conflicts remain; PR stays draft.
