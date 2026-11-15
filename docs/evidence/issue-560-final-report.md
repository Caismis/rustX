# Issue #560 engineering report

Rust 1.100.0 / MSRV 1.100 is the single baseline; Edition 2024 is retained.
The implementation removes Cargo-directory inference, consolidates dependencies
and diagnostics, splits private owners, and strengthens native/browser test
lifecycle proofs. Targeted proof cost falls, while the additional documentation
and configuration checks add work. No overall CI or runtime speedup is claimed.

This is the source/evidence handoff before publication. Independent review and
the final hosted FULL verdict are required afterward. The PR description records
the actual final source SHA, tested merge identity and run links, avoiding a
documentation commit that invalidates its own final-head CI evidence.

## Identities and scope

| Checkpoint | Actual commit | Production `src` tree | Locked/compiler inputs |
| --- | --- | --- | --- |
| Issue base | `33234e5b19f9074924a5fba1315c23dcf7210c32` | `0de389cb45025a28b503d9bb62f206bd841e53ff` | Rust 1.98.1 |
| B0 measurement | `8fab9994784918b788aa795656d042d28761b65d` | `0de389cb45025a28b503d9bb62f206bd841e53ff` | Original lock, Rust 1.98.1 |
| Minimal B1 | `dc454cff7b55271bbcdab94438a37697a7619d67` | `10e64d390d6b6458e12994664fd8ee35356ccefc` | Same original lock, Rust 1.100.0 |
| Production B2 measurements | `6cb5a60de79ad1a9c22bee0f7dddc7e8b7e3d12e` | `3e33166137ae2937ce6ff6a1a556d0176685c56c` | Cleaned lock, Rust 1.100.0 |
| Final checker/check measurements | `053df8602db46a849292c15fcd68957a8dca91ad` | `3e33166137ae2937ce6ff6a1a556d0176685c56c` | Same B2 production/lock/toolchain |

Original lock SHA-256:
`d5065bbe2d9e8308393d92f6b3fee8ef569c9e16f646392b73aa9211be5ecc74`.
Final lock SHA-256:
`81d7ea1fd34fcff7c1288ec03f70651243ae78507fbf72216c352d2e77bb7c07`.
Final manifest SHA-256:
`38781f2ad336a55380ae7e0b2301c75aa2d396d033eea3a74e84309c91f789d8`.
The [portable JSON](issue-560-measurements.json) records full artifact, source,
toolchain, profile, feature, command, status, sample and local-log hash receipts.
Its command paths use the explicitly declared `${DEV}` / `${CONTROL}` variables.
Local receipts are retained outside Git; they are not hosted download links.

Between production B2 and the final checker checkpoint only the checker and two
documentation files changed. Production/release/runtime inputs remain identical;
their recorded HEAD remains **6cb5a60d**, never the later evidence commit.
All-target checking includes the modified example and was measured again.
Documentation is a runtime input of `contracts`; its timing stays historical,
and current documentation receives separate functional checks.

## Ownership, dependencies and diagnostics

The [guard/deployment map](issue-560-stage1-guard-disposition.md) explains removal
of `build.rs` and `profile_dir`. Production companions resolve only beside the
current executable. Deployment must install those siblings. Embedded guard bytes
are runtime-owned; a read-only preprovisioned sibling is accepted only after byte
and executable validation. Missing guard storage fails closed for Python lookup.
Native tests use Cargo's emitted artifact metadata and explicit physical staging,
including ordinary-library versus libtest/profile/feature identity. No old-layout
guessing or production environment override remains.

The [module map](issue-560-stage3-module-disposition.md) separates workspace
Python guard/interpreter/owner, Session Controller reads/uploads/lineage/settings,
and Workflow compiler/catalog responsibilities through private modules. Public
paths, admission/commit/settlement boundaries and ownership remain unchanged.
Module splitting creates no Cargo compilation boundary and earns no build credit.
`Commands::start` releases State before watch publication; observers release watch
borrows before acquiring State. The controlled real-owner negative experiment
fails when the State release is omitted, without hanging.

The [cleanup map](issue-560-cleanup-disposition.md) traces every dependency,
feature, dead-code and suppression decision. Direct `grep-matcher` and redundant
dev `nix` edges are removed; direct `tower/util` activation is narrowed but remains
transitive. Already-locked `serde_yaml_ng` replaces deprecated `serde_yaml`
directly, preserving the 16 MiB preflight, recursion/alias limits and format
validation. TLS, SQLite hooks/durability and lossless serialization remain.
The 106 baseline strict-rustdoc errors are repaired at their owners. Obsolete
dead-code allows and broad auxiliary lint exceptions are removed or narrowed;
retained exceptions have explicit reasons. Generated protocol/schema changes are
descriptions/JSDoc only. Strict first-party checks pass locally; upstream
PostCSS's missing-`from` warning remains visible and is not called clean output.

## Proof preservation and final lane ownership

The [consolidated test map](issue-560-stage4-test-disposition.md) and
[CI map](issue-560-stage5-ci-disposition.md) trace removed, replaced and moved
proofs to compiled namespaces. The two removed contracts are the synthetic Cargo
parent-depth assumption and the weak external post-parent-exit descendant check.
Native replacement tests witness actual group admission and physical settlement.
Linux child reaping and macOS group-absence proofs retain separate semantics.
Provider cancellation uses the existing before-header gate. Browser teardown
uses its existing page/proxy-drain-before-Host-stop owner and native shutdown.
All original nine negative doctests retain intended API-restriction errors, with
six valid positive counterparts; explicit Linux `--doc` executes all 15.

| Final additional/strengthened proof | Compiled owner | Executing CI owner |
| --- | --- | --- |
| `inactive_goal_can_idle_unload_and_policy_is_user_only`; `one_reaper_timer_wakes_from_manual_clock_and_unloads_automatically` | `local_runtime::session_runtime_manager::tests::residency_policy::` | Linux FULL `rust-contracts`; FAST conditional local-runtime/App Server step |
| `issue552_rpc_header_and_chunk_authorization_routing_and_warmed_cache_denial` | `local_runtime::session_runtime_manager::tests::workflow_run_snapshot::` | Same Linux owners; exact Attempt publication witness retained |
| Empty ignored-cache reuse; absent/nonempty global; exact features/target/success; warning/error preservation; excluded argument native equivalence | `example check_test_lanes`, `tests::{empty_global_ignored_shortcut_reuses_single_atom_and_keeps_positive_native,absent_or_nonempty_global_ignored_runs_native_without_prefetch,empty_global_ignored_requires_exact_features_and_target_and_success,diagnostics_and_warnings_are_preserved_across_global_positive_and_failed_queries,excluded_argument_shapes_delegate_and_match_uncached_native_outcomes}` | Linux FULL `rust-contracts --examples`; 34 runnable / 2 parent-invoked ignored child fixtures |

macOS FULL intentionally skips the manager test prefix above and does not run
the example's test matrix. Its ordinary checker still enforces actual macOS
lane selection. Do not infer native macOS execution from Linux `cfg` inspection.
Nine external integration targets retain their owners. The earlier integrated
run's 33 ignored cases comprise child fixtures, offline measurements, one corpus
regenerator, optional git-lfs, external MCP measurement and five live paid API
tests; the later checker adds a second ignored fixture child. Live paid API
tests were not run and are not a mandatory correctness prerequisite.

FAST/FULL classification, platform/browser jobs, trusted-main-only cache writes,
artifact provenance, test selection and current-head merge verification remain
authoritative. No scheduler, persistent target registry, timeout increase,
generic retry, sleep-based proof or relaxed assertion is introduced.

## Build and test cost observations

Host: Fedora 45 x86_64, Core Ultra X7 358H, 16 logical CPUs, about 30 GiB RAM;
unrelated desktop/services were running. Native jobs=4, incremental=1; release
jobs=2. Cold means a new target directory, with warm OS/download caches.
`time -v` RSS is a maximum command-child observation, not aggregate RAM.

| Work, seconds | B0 | B1 | B2 |
| --- | --- | --- | --- |
| Cold bins default / all features (one each) | 195.43 / 198.29 | 185.24 / 190.70 | 185.40 / 188.05 |
| Mtime-only dependency-hit build, both samples | 22.51, 23.07 | 20.75, 19.88 | 20.16, 19.93 |
| Actual leaf edit: all-target checks, three samples | 19.38, 18.91, 19.02 | 17.92, 17.76, 17.47 | Initial 18.03, 17.60, 17.53; final 17.36, 17.45, 17.50 |
| Actual leaf edit: lib test build + 25 todo tests, three samples | 35.36, 35.78, 34.18 | 34.17, 33.01, 32.63 | 34.78, 32.19, 32.33 |
| Native lib preparation | 268.29 | 371.22 | 369.14 |
| Warm checker, three uninstrumented driver-wall samples | Not recorded | 11.8699, 11.5684, 11.5873 | Initial 104.0387, 104.4486, 104.3143; final 60.7963, 60.9316, 60.5896 |
| Bash descendant proof, three samples | Not recorded | 2.5389, 2.5527, 2.5551 | 0.3299, 0.3433, 0.3406 |

B0 lib preparation builds only the harness; B1/B2 also build/stage ordinary
library and four binaries. This mandatory compiler/deployment repair, including
the late two child-fixture path repairs, earns no B2-only saving. Final check
prewarm 17.95s reuses an already warm cache and cannot be compared with the initial
123.49s preparation. All temporary source edits are restored byte-for-byte.
Cold builds have one sample each and support no statistical improvement claim.

The checker already evaluated focused targets before this change. Acquiring each
successful metadata-bound libtest binary once removes repeated Cargo acquisition
within a run, while native libtest retains filter authority. Doctests still use
Cargo. A cached successful **empty** global ignored set can serve only one
nonempty ASCII filter atom on the exact same features/target key; other argument
shapes, positive queries and absent/failed/nonempty global sets remain native.

The separate forwarding-shim diagnostic observes 30 Cargo calls initially
(1 metadata + 15 acquisitions + 14 doc listings), then 24 (1 + 15 + 8).
Six redundant filtered ignored-doc listings disappear; all six positive owner
queries remain. Formal median falls **104.3143 -> 60.7963s** (-41.72%, sample
stdev 0.1722s / range 0.3420s finally). It remains **5.2468x B1**, whose proof
coverage omitted explicit documentation ownership. Added proof and strict
configuration checks cost time. Instrumented residuals include shim/logging
overhead and are not isolated causal native-work savings.

The stronger Bash proof's median is 0.3406s versus 2.5527s for the weak historical
proof (cost comparison only). Three provider-cancellation cases also have three
samples each in JSON; millisecond differences are descriptive. Whole `tools`
libtest body time instead regresses **24.37 -> 29.72 -> 38.76s**; the exact cause
is unisolated. B0 wall 24.61s includes Cargo freshness/startup; B1/B2 direct-harness
walls 29.74/38.78s do not. Matched libtest-body timing is the appropriate boundary.
B2 external prep/body seconds: contracts 84.70/1.51 (historical documentation),
provider 13.05/42.69, process 5.16/11.14, durable 3.13/10.41, tools 4.61/38.76.

## Release/runtime comparability and negative results

The original provider runtime attempts failed first on a hardcoded uv cache
location, then on a stale benchmark oracle. Those failed runs remain evidence,
not accepted performance samples. A five-file neutral harness overlay was replayed
on original B0/B1 production/lock/compiler snapshots in detached worktrees:
`8a986746fd89281f0dc8a476c5f810ada2c12ce2` /
`2642de97019e36a0f1402f994ef78a3a4dfb0ed0`.
No production code or provider fixture was overlaid. Independent fixed input,
provider binding, full schema/envelope, exact declared workspace and live/reopen
oracles are verified before timing is accepted; comparator normalization cannot
derive expected workspace from captured requests.

Actual shared-harness binaries SHA-256:
B0 `bc4fb693935b1247e024c7d32e2eac16642b050d7c4f9587de8b19a6358f08a6`;
B1 `d915d77b1f984e4d4b984bcd152a2c9edbe43179358fe4b207ef17cfd10133fb`;
B2 `fa74f9fce8cf8c7599f58b4a63c160cdb04a13c346d9ec4751e97adcc3a6edab`.
B2 uses the existing release helper, default features, fresh target and
`verified: true` / root artifacts `fresh: false`. Its 850.98s build is not
comparable with the warm-dependency historical-overlay preparation (517.48 /
465.55s). Original older-harness cold release observations also have a different
harness scope.

Each comparison accepts 18 process/runtime samples and nine alternating pairs
across short-small/tool/cancel: all product/provider exits zero, all oracles pass,
live=reopened=wire counts 40/2/1, ten monotonic lifecycle phases, and 129 full
request pairs equal (258 requests). Each append comparison accepts 24 samples,
three pairs per history 0/64/512/2048, with 32 measured appends each and unchanged
storage footprint. JSON retains every run, process/result hashes, raw append
durations, counters, paired ratios and observed memory/storage values.

| Median of paired ratios, after/before | B0 -> B1 turn / lifecycle | B1 -> B2 turn / lifecycle |
| --- | --- | --- |
| short-small | 1.0174 / 1.0047 | 0.9966 / 0.9976 |
| tool | 0.9898 / 1.0003 | 1.0006 / 1.0402 |
| cancel | 1.0734 / 1.0756 | 0.9270 / 0.9775 |

These three-pair observations establish no general runtime improvement.
B0 -> B1 cancel CPU is 1.0769x (+20ms), lifecycle +20.5ms, and is explicitly
excluded from broad CPU-parity descriptions. CPU is quantized at 10ms; runtime
RSS is sampled, not a peak. Append paired mean-latency ratios span
0.9830–1.0066 (B0/B1) and 0.9806–1.0109 (B1/B2); retain the individual samples.

The live omitted-State-drop experiment fails in 0.03s and is reverted. Its
production file restores exactly; its test file subsequently changes only by
rustfmt. Two initial integrated runs also fail legitimately: one reaper/observer
quiescence race, then one unobserved completed-Attempt publication. Their real
owner gates fix the races; no assertions/time budgets are weakened. The later
integrated run passes 6259/0 failed/33 ignored; subsequent checker changes have
their own 34-pass/2-ignored focused checks.

Invalid control observations are kept separate: unsupported timing syntax,
mistyped zero-test filters, a local cold-driver `finally` bookkeeping bug
(original aggregate 1 versus actual driver/12 raw commands all 0), and a
postprocessing `KeyError` after valid runtime execution. Original files remain;
reconciliation checks raw statuses/hashes rather than trusting a completion
marker. See JSON `invalid_experiments` and `local_receipts`.

## Verification and reproduction

The following are actual successful local source-scope checks; full commands,
log hashes and measurement heads are recorded in the companion JSON and maps.
They are not a claim that hosted final-head acceptance has already passed.

| Command / scope | Local result |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings`, plus `--all-features` | exit 0, both configurations |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked` | exit 0 |
| `python3 test-support/prepare_native_tests.py -- cargo test --all-targets --all-features --locked --no-run`, then `cargo test --workspace --all-targets --all-features --locked` | exit 0, integrated 6259/33 ignored before five additional checker tests |
| `cargo test --workspace --doc --locked`, plus `--all-features` | exit 0, 15 each |
| `cargo test --locked --example check_test_lanes`, plus `--all-features` | exit 0, 34/2 ignored each |
| `cargo run --profile test --all-features --locked --example check_test_lanes` and each Linux job | exit 0, native discovery and explicit doc ownership |
| Python CI policy / benchmark / native-prep / local provider tests | 66 / 38 / 11 / 55 pass |
| `node --test benchmarks/runtime/compare-http.test.mjs benchmarks/runtime/pi-oracle.test.mjs` | 17 pass |
| Protocol `pnpm check`, `pnpm typecheck`; Dev typecheck/tests | exit 0; 43 Dev tests |
| TUI typecheck/tests | 1361 pass, 1 skip |
| Web typecheck/tests/build/i18n/provenance | exit 0; 4889 pass, 4 skip |
| Default-feature Cargo-emitted bin build + three native Web test files | exit 0; 24 native tests |
| Pinned Playwright 1.63 container, model-refresh + controlled Host teardown | 10 tests pass once; complete hosted browser matrix pending |

Use a writable target directory and uv cache, Python 3.12/uv, the pinned Rust and
Node/Playwright toolchains, SQLite development headers, and the existing provider
environment (`uv sync --frozen`). Set `RUSTX_REQUIRE_PROVIDER_EMULATOR=1` for native
conformance and `UV_CACHE_DIR` to caller-owned storage. Prepare native siblings
before tests. Reproduce release inputs from each actual measured checkout:

```sh
python3 benchmarks/runtime/build-revision.py --repo "$DEV" --target "$TARGET" --output "$OUTPUT"
UV_CACHE_DIR="$CACHE" python3 benchmarks/runtime/compare-revisions.py \
  --before "$BEFORE/conformance" --after "$AFTER/conformance" \
  --output "$RESULTS" --pairs 3 --case short-small --case tool --case cancel
python3 benchmarks/runtime/compare-append.py \
  --before "$BEFORE/conformance" --after "$AFTER/conformance" \
  --output "$APPEND_RESULTS" --pairs 3
```

Run the build helper from that checkout so rustup uses its pinned toolchain.
The historical replay uses the recorded neutral overlay and cache-only historical
driver adaptation, not today's production files copied over the old revision.
Use the existing browser scripts/container and artifact provenance; do not replace
the native Host or test framework. Serial timing avoids competing task-owned
builds, while unrelated desktop work remains an explicit limitation.

Final hosted acceptance must cover configured Linux/macOS/provider/protocol/
TUI/Web/browser obligations and `Merge verification` at the PR's current HEAD.
Baseline FULL was not wholly green (macOS timeout and browser teardown failure);
neither is substituted for candidate evidence. Assertions, durability/isolation,
native test authority, selected suites and trusted cache writers remain intact.
