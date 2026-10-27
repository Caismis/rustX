# PR #485 continuation evidence, 2026-10-27

Delivered source: D `e8d6380fa6ea0724b752970473d093c629cb8a63`, production tree `d6d193e07b16db0ab9b1ce646a254a1c1674c80e`; preserved ordinary release conformance SHA-256 `8ec26a4a08bde085f46ee40159d3870f7ead675bec39f42cb4f3e55df1d1d233`. E was rejected and fully reverted in `6e1ca189abfa9879fe87d2e4ecd053530434a491`. See the [report](../../2026-10-27-continuation.md) and [retention record](retention.json).

## Archive

[raw.tar.xz](raw.tar.xz) is 7,233,600 bytes, SHA-256 **`808dc22f1d1230379e74332856ab35fcc5c82db58284da9584d84dffe73a6998`**. Its [external manifest](raw.tar.manifest.json) lists **2,146 payload files** totaling 1,690,655,299 uncompressed bytes. An identical embedded `MANIFEST.json` is the additional archive member. Original evidence and executables remain preserved locally; the archive excludes executable binaries, dependency checkouts and node_modules. The packer verified every member. The separate [extraction verification](archive-verification.json) records independent checks and numeric reproduction.

The payload contains 300 formal products across retained/rejected direct comparisons, final Pi comparisons, append diagnostics and the three-engine reference. All raw request/provider/process records, compressed SDK logs, failed functional/setup attempts, checks, identities and command plans are retained. The 7,168-call Reader diagnostic is separate. Some analysis attachments were completed after the archive snapshot and are delivered outside it: [final cost analysis](final-cost-analysis.txt), [DeepSeek analysis](deepseek-final-analysis.txt). They do not replace or mutate raw evidence.

## Recompute from extracted data

Extract into a new empty directory; the following readers use Python's standard library. Replace `RAW` with that extraction directory. Direct comparisons reproduce the original JSON byte for byte. Raw-only audits intentionally cannot reverify executables or source checkouts on a different machine; original strict audits retain those checks. Archived absolute paths are identity records, not portable installation paths.

```sh
python3 recompute-direct.py --root RAW > d-direct-recomputed.json
python3 recompute-direct.py --root RAW/candidate-e-direct > e-direct-recomputed.json
python3 recompute-pi.py --root RAW/final-cohorts --raw-only \
  --identity RAW/candidate-d-build/identity.json \
  --expect-rust-sha256 8ec26a4a08bde085f46ee40159d3870f7ead675bec39f42cb4f3e55df1d1d233 \
  > pi-recomputed.json
python3 audit-final-append.py --root RAW/final-cohorts \
  --binary-verification off --before-identity RAW/baseline-b-build/identity.json \
  --after-identity RAW/candidate-d-build/identity.json \
  --expect-before-sha256 f80f3b7ba0f2be6035c7459c87cf04e69ec596e64ccba0d72d43ddce7d8ebdb3 \
  --expect-after-sha256 8ec26a4a08bde085f46ee40159d3870f7ead675bec39f42cb4f3e55df1d1d233 \
  --output append-audit.json
python3 recompute-three-engine.py --dataset RAW/three-engine-final --raw-only \
  --freeze RAW/deepseek-setup/final-source-identity.json \
  --expected-rust-sha256 8ec26a4a08bde085f46ee40159d3870f7ead675bec39f42cb4f3e55df1d1d233 \
  --expected-harness-revision 6e1ca189abfa9879fe87d2e4ecd053530434a491 \
  --output three-engine-audit.json
```

These scripts independently recompute numeric counters, dimensions, accepted statuses, process bindings and recorded semantic evidence without importing producer statistics. D/E independent retention audits additionally reconstruct expected HTTP request bodies on the original host. Raw-only numeric reproduction is not a second implementation of all product semantics or a crash-durability test.

## Build and validation

The [D](d-build-identity.json) and [E](e-build-identity.json) build identities separate ordinary O3 measured binaries from root-O0 correctness builds. [validation.json](validation.json) records counts and limitations, including the copied-test companion failure resolution. No UI-suite rerun, full repository CI or main merge is claimed. The [final plan](final-plan.json) records final product ordering from 00:30:05 UTC and strict audits after all product runs.

To repeat product measurements, use the repo's `benchmarks/runtime/build-revision.py`, pinned checkouts, and recorded commands after adjusting paths. The DeepSeek setup and source-profile contract are documented in [the adapter guide](../../../../benchmarks/runtime/deepseek-reference.md). Run product cohorts serially, without builds/profiling/compression. The archived `run-retained-cohorts.py` is host-specific and intentionally asserts the recorded source/binary identities.
