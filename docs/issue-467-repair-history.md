# PR #478 repair history

Archived chronological investigation and validation evidence. Statements such as
"pending" describe their recorded stage, not current readiness. Current results
and protocol versions are in [issue-467-validation.md](issue-467-validation.md).
Earlier passing runs do not override later failures or current verification.

## Native command ownership review (uncommitted correction)

Combined library compilation passed (`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4
cargo test --lib --all-features --no-run`, 2m43s). The four compaction-head tests
passed in 2.00s; all 30 native read-layout tests passed in 2.49s. The first run of
the cancellation group failed its initial identical-cursor assertion (15 versus
14), then poisoned the projection mutex during assertion unwinding. Accepted-input
presentation may advance independently of Attempt publication. The test now
captures outside the guard and asserts the exact original settled Attempt A while
native B is active; it does not require unrelated presentation to stand still.
The corrected test has not yet been rerun at this recorded stage.

The normal binary build passed in 1m19s. The first standalone reading benchmark
launcher failed before startup because bare Node could not resolve the existing
extensionless TypeScript import; the established loader was restored for the actual
measurement. This launcher failure did not execute an Attempt or modify fixtures.

The same original 300-Attempt preparation with only cached selected-record SQL
preparation (reading132) exhausted the unchanged 240s guard after 292 settled
Attempts. It still advanced normally at Attempt 289 (237.43s), and the original
process/directory were gone after cleanup (240.623s). CPU was 227.64s at failure;
startup took 16s and the first ten Attempts 41.15s, versus 3.46s for the previous
baseline. These timing differences prevent a clean end-to-end improvement claim.
The statement cache alone is not considered a resolution. The remaining three
cancellation regressions passed on the original combined binary (0.86s): held
receipt versus successor, unavailable source, and blocked physical read.

Review134 passed all four native cancellation tests (1.28s), four compaction-head
tests (2.04s), 30 fragment tests (43.05s), and eight request-shape tests (14.56s).
This includes both new captured-header byte/geometry/tamper tests. Review135 strict
Clippy then rejected a test mutex guard with an explicit `drop` before later
cleanup awaits. The test now clones the finite latest-compaction value in one
statement and asserts outside the guard; no lint suppression or production change.
Full validation restarts from that correction (review136).

## Final-head CI and bounded reading diagnostics (b0928347)

Run [38816665394](https://github.com/Caismis/rustX/actions/runs/38816665394)
passed both Linux native lanes. The contract lane executed 3,866 library tests
(zero failures, five existing ignores); the boundary lane executed 237 in-crate
and 451 external tests, including the original skill-version storage-lock test.
Both new publication-contention regressions, source repair, shutdown/recomposition,
model no-op and DTO regressions passed. Protocol, lint, TUI and macOS Node also
passed. macOS native compilation completed; its library run passed 3,178 tests
and failed three delivery-source tests (four existing ignores, 894 filtered).
The external targets were not reached. Two fixtures declared files under a
canonical root but passed the raw TempDir root to the strict no-symlink reader.
On macOS `/var` is a symlink, so ancestor walking returned `NotADirectory`. A
third fixture similarly passed a noncanonical foreign root, producing
`Unauthorized` before reaching its intended file-identity mismatch. Fixture
roots are corrected consistently; production `O_NOFOLLOW` admission and original
byte/error assertions are preserved. This requires another actual macOS run.

Web unit tests passed 2,823 cases (four existing skips), but the complete browser
run passed 315 and failed the two `reading.spec.ts` scenarios. Both failures were
the unchanged 240,000ms preparation deadline for 300 real durable Attempts,
before browser assertions. The CI log did not identify the last completed
Attempt or the pending preparation operation. This failure is not relabeled an
infrastructure exemption or a passing browser run.

The exact existing production binary then completed the same 300 Attempts plus
bounded source lookup and Fork locally in 208.378s (reading128). A separately
qualified profiling run completed in 223.145s (reading129). Both used the default
filesystem and original deadline; both original native processes and temporary
directories were absent after cleanup. The first run spent 23.79s in admission
and 183.52s waiting for real terminal events. In the second run, an owned native
thread was observed in `jbd2_log_wait_commit`. This proves local physical waiting,
not the cause of the remote failure. No lost terminal event was observed.

The current binary's eight-second CPU sample at Attempt 200 attributed 27.36%
inclusive to request Source indexing (13.03% shape preparation) and 11.14% to
publication encoding. Sampling perturbs timing; these overlapping percentages
are diagnostic, not a performance improvement claim. No unmeasured production
optimization, reduced workload or longer timeout was introduced.

The maintained fixture now records bounded failure-only evidence: preparation
phase, completed/in-flight Attempt, the last eight admission/settlement timings,
and the original spawned native process's CPU/I/O/wait counters. PID start-token
checks fence process reuse; at most 64 worker wait channels are read. The timer
only observes the existing deadline and never cancels, retries, performs an RPC,
or changes execution. It is cleared before browser assertions and cleanup.

The maintained focused browser command
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 RUSTX_BINARY=/home/capi/Documents/codes/rustX-issue-467/target/debug/rustx bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts`
then passed both original scenarios (7.6 minutes). This validates the diagnostic
fixture but does not replace the failed remote complete browser result.

## PR #478 repair and latest-main integration (in progress)

Final102 `cargo test --all-targets --all-features` PASSED (635.101s command),
including4086 successful library tests,5 existing library ignores and all external
targets. Both earlier full-run failures passed. The original Linux regression
`definitions::skill_version_identity_is_frozen_across_the_boundary` passed inside
the47-test subagent suite and independently afterward (0.823s command); the whole
subagent suite passed again (35.369s). Maintained Linux boundary groups, lane
checking, generated protocol drift and Provider Emulator validation continue.
Full browser and new-head CI are still pending; no publication has occurred.

Browser101 passed BOTH maintained reading scenarios with unchanged300-Attempt
fixtures and240,000ms setup guards. Exact command:
`bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts` (451.141s).
Required binary/example build passed79.337s; filesystem writeback completed20.194s
before testing. Distant rail/locales and Fork exact-source/cancellation assertions
both passed. This is the first passing maintained reading pair on the current
repair production pin; the complete browser suite still must run.

Final102 now checks the two test-only changes (same-Job Running synchronization,
bounded failure-only progress diagnostics), then compiles/runs complete Rust and
the original/containing Linux boundary targets. Production remains the passing
Final99/100 targeted and Browser101 source. No new-head CI success is claimed.

Focused101 reran the unchanged Tool-progress module: all4 passed (1.98s test,
2.245s command), which does not waive the full-run timeout. The background
conformance fixture now preserves initial dispatch assertions and waits for the
same parked Job's actual Running publication before its status read. That avoids
reading while Starting→Running changes the native registry cut; it neither
retries latest nor accepts ReadCutExpired. Response diagnostics now include RPC
identity. A failure-only retained-Store diagnostic is being added to locate the
Tool-progress timeout before considering any production transaction change.

Final100 built all bins (79.465s) and all test targets (136.717s). After filesystem
writeback, the full command ran4084 passing,2 failing,5 existing ignored tests
(511.98s). Failures: retained parallel Tool progress exceeded its unchanged10s
guard after both tools physically completed; direct background-execution
conformance received ReadCutExpired. The previously failing model no-op case,
physical shutdown/panic/abort joins, same-cut source repair and real Child Unix
recovery all passed in this full run. The two failures are under investigation;
remaining external boundary/lane/protocol commands were not reached by100.

Final100 passed whitespace, formatting and strict Clippy (72.692s). The corrected
complete SQLite suite passed314 tests, with one existing ignored (88.54s test
execution; 248.291s command including compilation). All three same-transaction
snapshot oracles and both certified-root write/rollback tests executed and passed.
Production is unchanged from the successful targeted Final99 checks. Binary build,
all-target compilation and complete runtime/boundary validation continue.

Final99 passed whitespace, formatting, strict Clippy (72.618s), preflight9,
model-noop6, prepared capability3, prepared Trace4, cached Trace-cut2, and Native
layout30 tests. SQLite ran 313 passing, one failing and one existing ignored.
The sole failure was a new corruption fixture attempting to alter the start J
without updating its required optional source-id relationship; SQLite rejected
it before the old/new validation oracle. Production is unchanged. The fixture
is corrected without disabling SQL constraints before rerunning the full
containing SQLite suite. Binaries/full Rust/boundaries were not reached by99.

Final97's completed diagnostic also reported a test-only let-else and a missing
semicolon. Final98 passed whitespace/formatting and stopped only on the remaining
test semicolon; that was corrected. Final99 restarts the entire pipeline on the
frozen corrected source. Neither failed lint run is counted as test execution.

Final97 passed whitespace and formatting, then strict Clippy stopped on
`commit_canonical_publication` exceeding the repository function-length limit
(101/100 lines). No subsequent Final97 tests ran. The small refactor is corrected
without a lint exemption before Final98 reruns validation.

Clients96 passed all ten maintained client checks: protocol TypeScript, dev
TypeScript/tests, TUI TypeScript/tests, WebUI TypeScript/tests/build/i18n/provenance.
The ledger is `/tmp/478-validationclients96-results.json`. Main was fetched again
and remains `8cb41624eb2dc2ca87e4409961f42553ceb029cf`.

Final97 freezes the next production pin: same-transaction validated request reuse,
certified root write deduplication, exact-owner prepared Trace record admission,
and unchanged exact string admission with a single raw-byte debit. Independent
read-only reviews found no removed source, identity, publication or admission
checks. New deterministic tests retain original-path/error/budget oracles and
physical SQL update/read counters. Formatting, strict Clippy, targeted oracles,
full Rust and maintained boundary validation are now running; results remain
pending and no timing improvement or PR readiness is asserted.

Browser95 failed both maintained reading scenarios at the unchanged 240,000ms
fixture setup budget (`bash web-console/scripts/browser-tests.sh
test/e2e/reading.spec.ts`, 493.054s). The prerequisite binary/example build passed
(79.817s), and filesystem writeback completed before execution (96.945s).
Read-only inspection found 287 requests after the first timeout; the second
had reached 290 at approximately 222 seconds but also timed out. These results
remain blockers, not environment-qualified passes. Actual-browser CPU sampling
identified request-source construction and publication encoding as substantial
costs. Same-transaction duplicate frozen-request validation is being reduced
without removing any integrity, identity, start, outcome or publication checks.
No final full-suite or new-head CI success is claimed.

Final94 passed whitespace, formatting (4.531s), strict Clippy (72.755s), all three
prepared-capability tests after the fixture identity correction (162.890s command
including compilation), and `cargo build --bins` (82.509s). Browser95 now uses the
maintained reading command after its exact required fixture build and completion
of filesystem writeback; no timeout, workload, filesystem or compiler-profile
change is made. Native/source tests remain the passing Final93 production-source
evidence; the latest full Rust command and full browser suite still must run.

Final93 strict Clippy passed (73.299s). The corrected no-op module passed all six
tests in 3.62s. Prepared capability tests passed the exhaustive budget and large
initial-item oracles, but their ownership fixture used a non-UUID Conversation
identity; the fixture now uses a canonical UUIDv7 with production admission
unchanged. On the same compiled production source, direct lib-binary runs passed
all 30 `runtime::read_layout::` tests (1.12s) and all 309 `durable::sqlite::` tests
(one existing ignored, 88.35s). Exact logs are `/tmp/478-native-direct93.log` and
`/tmp/478-storage-direct93.log`; only the unrelated invalid capability fixture
identity changed before these runs. All four new packed-hole selectors executed
successfully. Final94 rebuilds the corrected fixture and binary, then the
maintained reading browser scenarios are the next performance gate.

Final93 is the combined frozen-source validation: Source SQLite64 packed own-hole
metadata, prepared capability heads, and the minimal no-op publication-order
repair. No source edits occur while it runs. New packed-hole selectors are:

- `t08_t14_t19_packed_message_hole_facts_reconstruct_every_default_field_and_original_byte`
- `t08_t12_t19_packed_request_holes_reject_corruption_before_allocation_and_restore_exactly`
- `t10_t12_t19_packed_hole_geometry_cannot_be_rehashed_or_laundered_into_new_requests`
- `t10_t14_t19_packed_hole_update_failure_never_refuses_native_request_or_journal`

The private pack has a hard 16KiB/32-fact bound, participates in the original
request/item witness and origin reuse checks, and contains geometry rather than
content. Existing logical field/page/range oracle assertions remain. Store root
pages admit at most two bounded metadata BLOBs; points/ranges one. The actual
selected-field public route remains at most three metadata BLOBs; original
selected content still touches at most two 64KiB segments. Formatting and scoped
whitespace checks passed before central execution; full results are pending.

The stage-only no-op diagnostic (unchanged 30-second guard) identified the newly
added `admitted projection settled` wait as the hang. That extra schedule was
removed: the test now establishes idle publication before admission, checks the
actual synchronous Admitted transition, and verifies Pending at the identical
model-selection boundary. All original cursor/state/configuration assertions
remain. Combined validation is pending.

Prepared capability reuse is now pinned but not yet executed: existing native
prepared heads retain normalized rows plus exact encoded charges and the old
conservative admission threshold. All three capability mutation arms update the
same bounded head handle; constructor admission retains its original 16KiB
item policy. Three independent old-capture comparisons cover every budget,
large initial rows, and exact source ownership across model/availability changes.
A separately measured Source normalization is in implementation: finite own
provenance-hole field metadata remains bound to each original request and its
witness, rather than inserting each generated descendant separately. No timing
or final-readiness claim is made before its oracle and integration checks.

Final90 strict Clippy and four Source-copy oracles passed. Its no-op module
exposed an incorrect newly-added test assumption: parking the derived worker
does not block synchronous native AttemptAdmitted publication. Final91 corrects
that test boundary: admission must publish while the derived worker is parked;
only the identical model-selection operation must leave the subscription Pending.
All original assertions and the original liveness guard remain.

The opt-in current-source native 300-turn encoding measurement passed (234.58s
whole test; 230.456s measured execution), with 300 original requests and 3,135
native display records. Request Source layout/fields consumed 30.087s/31.833s;
original byte copy consumed 0.370s. Projection capture/fit/probe consumed
31.006s/19.917s/9.341s. These phase totals overlap other owner timings and are
not additive end-to-end shares. A successful authorized read-only `sudo -n perf
record -F 99 --call-graph dwarf,65528 -p <owned test process> -- sleep 8` collected
301 samples; this instrumented no-tools fixture is diagnostic, not a browser
pass or a comparison against another product.

Reading89 maintained browser command failed both scenarios at the unchanged
240,000ms fixture setup limit. `perf record -F 49 -g -p <owned process> -- sleep 10`
was denied by `perf_event_paranoid=4`; no samples were collected and no system
profiling policy was changed. Final90 validates two subsequent changes: explicit
pre-admission activation publication ordering in the no-op test, and a private
bounded original-Source copy with one file owner and no repeated root-index
lookup. The source-copy oracle covers exact bytes/extent, 64KiB windows, EOF,
destination errors, and file ownership; no speedup is yet claimed.

Final89 full Rust execution: 4,064 passed, one failed, five existing ignored,
551.91s test execution. Tool progress passed in the complete suite. The sole
failure is `model_selection_noop::identical_explicit_selection_is_inert_during_an_attempt`
at the unchanged 30-second liveness guard. Its complete six-test module then
passed in 1.11s; this isolated result does not waive the full-suite failure.
The activation-publication synchronization is under investigation. Maintained
reading browser scenarios are now executing on the final89 binary.

Final89 passed whitespace, formatting, strict Clippy, all four event-preview
original-path oracles, all four Tool progress tests in isolation, all 306 SQLite
tests (one existing ignored), binary build, and all-target test compilation.
The full Rust and boundary commands are now running; isolated progress success
is not treated as closure of the earlier full-suite timeout. A final read-only
cross-component audit found no additional concrete shutdown or unavailable-state
violation, but does not replace execution evidence.

Epoch87 freezes Source-only event preview pruning: fourteen closed typed event
families need one to four known field probes instead of twenty-four, and their
fixed short root keys cannot contribute to the previous key-header loop. Other
variants keep the complete existing path. Four real-native oracle tests compare
all witness bytes, every indexed field, original segments and frontiers against
the original path; they also assert nonempty evidence and unchanged C/J/read cuts.
No progress batching, durability boundary or execution-owner change was made.
Final87 Clippy found one test-only empty-String spelling, corrected before
final88 Clippy passed (72.829s). Final88 ran all four oracles: three passed;
the adoption/turn fixture failed native admission because its Turn envelope
omitted TurnId. Final89 supplies the same real TurnId for start/completion,
retaining all native validation and oracle assertions. Execution/performance
validation remains pending; no failure is waived.

Final85 full Rust execution failed one test: 4,060 passed, one failed, five
existing ignored (536.44s). The sole failure remains
`scripted_suites::agent::tool_progress::parallel_calls_bound_progress_per_call_and_keep_canonical_order`.
Its new failure-only evidence reports both tools started and physically completed,
controller finished, and only one model request: the failure precedes the second
model turn, after physical tools settle. The original ten-second guard remains.
No batching/changed progress durability is implemented; Source redundant-work
removal is being examined within the existing per-event commit boundary.

The same source passed the exact original Linux storage-lock regression
`cargo test --all-features --test subagent definitions::skill_version_identity_is_frozen_across_the_boundary -- --exact --nocapture`
(0.823s command), its complete subagent target (35.558s command), and all 236
in-crate boundary tests (123.90s test execution). Boundary86 also passed all 451 external Linux boundary tests (225.806s
command), actual Cargo lane discovery (15.466s), generated App Server protocol
drift checking (80.378s), and all 54 provider-emulator Python tests. These are
local Linux results; remote CI still targets the original PR commit.

Final validation epoch85 follows two bounded refinements: the existing Native
connection reuses prepared ordinal/key statements, and cache-hit admission
checks exact header length without constructing a second header. Independent
old-encoder comparison covers the 16KiB boundary and an allocation counter proves
zero admission header allocations followed by exactly one output allocation.
The Tool progress timeout now reports retained watches/controller/model counts
only upon failure; its original ten-second guard and assertions are unchanged.
Final whitespace, formatting and strict Clippy passed (73.088s); compilation and
remaining execution are in progress. A fresh fetch still resolves main to
`8cb41624eb2dc2ca87e4409961f42553ceb029cf`.

Maintained browser reading84 failed both unchanged scenarios during fixture
setup (`Fixture "reading" timeout of 240000ms exceeded`, lines 52 and 186;
source location 10:19). Command: `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 RUSTX_BINARY=/home/capi/Documents/codes/rustX-issue-467/target/debug/rustx
bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts`.
The default filesystem, pinned browser and original assertions were retained.
This remains a validation blocker, not a claimed infrastructure waiver.

The final Native query refinement (`native83`) passed strict Clippy, all 25
Native layout tests, all 300 SQLite tests (one existing ignored measurement),
and binary build. Its default-filesystem diagnostic (`reading83`) failed at
200/300 settlements after 240,006.923ms; cleanup completed at 240,188.206ms and
all owned processes/directories were verified absent. During the stall, the
physical Native worker repeatedly remained in kernel `jbd2_log_wait_commit`,
with almost no CPU progress between approximately 27s and 121s of the run;
I/O pressure and process data were saved in `/tmp/478-reading83-io-block.json`.
This explains a concrete stall, but is not a passing performance result or
proof that all earlier reading failures were external. The maintained browser
reading scenarios are being run again on this source without changing budgets.

Current client validation (`clients82`) passed protocol TypeScript checking;
Dev typecheck and all 41 tests; TUI typecheck and 1,158 tests (one existing skip);
Web typecheck and 2,819 tests (four existing skips), production build, i18n and
provenance checks. These results do not cover browser conformance or remote CI.
The Native query-elimination refinement adds four independent geometry/query-work
regressions. Its first strict Clippy run (`native82`) rejected a nested Option
return type; it is replaced by an explicit private imported-field result.
The corrected `native83` strict Clippy passed (72.924s). Native and SQLite
execution passed: 25 Native layout tests and 300 SQLite tests (one existing
ignored measurement), including the new query-work and corruption checks. Binary
build remains in progress. A further corruption regression
gates page path/key cells by SQL type and byte size before Rust allocation,
for both shared-shape and ordinary pages.

The shapes81 binary built successfully (77.314s), but the unchanged default-
filesystem reading81 diagnostic still failed: 293/300 settled Attempts at
240,008.092ms; physical fixture cleanup completed at 240,795.689ms. The owned
harness/process IDs and temporary directory were verified absent afterward.
There were no provider errors beyond its serving announcement. This is not a
browser pass or an external infrastructure qualification. Native certification
query work is under inspection before another end-to-end attempt.

The corrected shapes81 source passed strict Clippy (72.835s) and the complete
300-test SQLite selection: 299 passed, zero failed, one existing ignored
measurement (95.61s test execution, 256.889s including compilation). The previous
span/custom-layout and schema-expectation failures are closed by that containing
suite. All eight shape regressions were discovered and passed. Binary build,
unchanged reading budget, full native/client/browser commands and new-head CI
remain outstanding; these focused results do not establish overall completion.

Schema-63 validation at shapes80: strict all-target/all-feature Clippy passed
(72.881s), formatting passed, fragment selection passed 19 tests (20.56s test
execution), and native layout passed 21. A direct full SQLite run of the same
compiled library discovered 300 tests: 296 passed, three failed, one existing
measurement stayed ignored (89.97s). All eight new shape regressions passed,
including caller-absence classification, original-capture witness corruption,
rollback, exact legacy field/page/range comparison and restart. Two failures
belong to old physical-row assumptions/custom-layout test setup; the third is a
remaining schema-62 expected version. Corrections and a full rerun are pending.
The preceding shapes78 failure was an incorrect assertion against a different
summary API; the corrected proofs directly require failed optional field/page
reads and independently readable original RequestStart facts. Clippy79 reported
style/type-conversion diagnostics, all fixed before the passing shapes80 lint.

The first schema-63 shape run (`shapes77`) compiled and ran 17 fragment-selected
tests: 12 passed and five new shape tests failed before reaching their assertions
because their fixture changed the prompt after freezing its input fingerprint.
The fixture now freezes only after all inputs are set. A direct run of that
compiled library's 21 native read-layout tests passed 20; the imported-ancestor
fixture incorrectly expected more than its actual 20 static descendants. Its
replacement assertion counts the independent original traversal and retains full
field equality. These corrections await rerun. Static review also requires
rebased path-length admission and matching root-coordinate/count evidence before
shared metadata can be accepted. No test failure is waived.

Latest maintained browser check (reading76) **failed both reading scenarios** on
prepared74 source: `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 RUSTX_BINARY=/home/capi/Documents/codes/rustX-issue-467/target/debug/rustx bash web-console/scripts/browser-tests.sh test/e2e/reading.spec.ts`.
Both failures were `Fixture "reading" timeout of 240000ms exceeded during setup`
at `reading.spec.ts:10:19`; the tests are at lines 52 and 186. The unchanged
pinned browser container ran one worker. `/tmp/478-reading-focused76.log` and
the two Playwright error contexts/traces record the failures. This is an open
implementation/validation blocker, not a proven external infrastructure failure.
Private Source field-metadata normalization is now under implementation; its
correctness and performance are not yet established. No new-head CI or PR
readiness is claimed.

The same default-filesystem 300-turn diagnostic stopped at 211 settled Attempts
in 240 seconds (reading40). After same-transaction RequestStart validation reuse,
per-item Source header capture, and bounded JSON parse reuse, reading44 stopped
at 229/300 in the unchanged budget. Eight new focused regressions, formatting,
Clippy and bins passed at perf43, but the browser performance blocker remains.
The first profile used 16 KiB DWARF stacks; reading45 repeated with 64 KiB stacks
and recovered substantially more native ancestors. Its early/late windows place
request-observation capture at roughly 41–46% of sampled CPU, publication encoding
at 18–21%, and model history reconstruction at 4–5%. Inclusive percentages overlap
and are not additive. The profiled run also stopped at 229/300. These diagnostic
runs are not browser passes. Owned diagnostic processes were joined/removed.

The subsequent locality46 checks passed formatting, Clippy and bins, plus all
nine read-layout tests and the three existing request-header capture tests.
An attempted additional inline-Text binding filter was based on a mistaken
baseline: existing capture already omits complete small-text references. Its
first focused run passed three tests and failed four new tests that assumed
those references existed. The speculative filter and four misbased tests are
removed in favor of an equality proof for the existing policy. The four
request-header tests passed at optimized48, including
`t08_t12_t19_request_headers_already_inline_complete_text_and_retain_deferred_sources`;
no request-descriptor omission policy change survives this investigation.

`Capture::item` and `Capture::page` additionally reuse the already admitted parsed
value for finite display DTOs instead of parsing the same capped bytes again.
Lexical JSON carriers still use their original capped-byte decoder; this does
not change raw JSON bytes, integer domains, page limits or source authorization.
The exact byte-oracle capture test passed. The reading47 diagnostic used the
previously built locality46 binary, without this latest Capture change. It stopped
at 228/300 at 240,005.759 ms; cleanup completed at 241,088.609 ms with no owned
processes or fixture directory remaining (`/tmp/478-reading47-cleanup.json`).
Optimized48 formatting and whitespace checks passed; Clippy then found a
test-local import after statements, which was corrected. Full optimized-source
Rust, client and browser validation remains pending.

The private `Inline`/`OriginalSpan` header representation is now implemented in
SQLite schema 62. It retains every original selected path and raw byte range
while avoiding redundant per-field header copies. Normalized header reads admit
at most two source rows and 16 KiB of original bytes; direct raw-range reads do
zero header materialization. The legacy normalized field/page comparisons,
corruption checks and duplicate-key last-wins behavior remain explicit proofs.
No public descriptor policy, raw JSON bytes or execution identity changes.

The first spans51 Source run passed five tests and failed one new fault-fixture
assertion; the full SQLite group passed 283, failed that same test and retained
one existing ignored measurement. The optional layout failure was wrongly
attributed to the independent native Journal owner. The corrected fixture proves
the exact insertion trigger fires once, all partial optional rows roll back,
selected Request detail is unavailable, native R/C/J are unchanged and independent
Journal facts/readiness remain healthy at `(0,0)`. No production repair was needed
for that assertion. Corrected spans52 formatting and whitespace checks passed.
At spans53, Clippy
passed in 74.464 seconds, all six Source proofs passed, and bins passed in 77.412
seconds. The unchanged 240-second reading53 harness then stopped at 233/300 at
240,006.961 ms; teardown completed at 240,749.370 ms. Late profiling/report work
overlapped this diagnostic, so it is neither a clean comparative timing result
nor maintained-browser evidence. Exact owned harness PID 3610930, native PID
3611048 and `/tmp/rustx-web-console-CgB9qr` were all absent on cleanup verification.
The browser blocker remains unresolved until the current binary completes the
original reading and full browser commands.

| Local optimization evidence | Executed result |
| --- | --- |
| perf43: `git diff --check`, `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo build --bins` | All passed; Clippy 72.629 seconds, bins 76.247 seconds. Ledger `/tmp/478-validationperf43-results.json`. |
| perf43: `request_start_validation_tests`, `request_header_capture_tests`, `structural_item_reuses_admission_without_changing_bytes_counts_or_integer_domains` | Four, three and one passed respectively. Logs `/tmp/478-request43.log`, `/tmp/478-header43.log`, `/tmp/478-parse43.log`. |
| locality46: same four quality/build commands | All passed; Clippy 72.483 seconds, bins 76.085 seconds. Ledger `/tmp/478-validationlocality46-results.json`. |
| locality46: `runtime::read_layout::tests::`, `request_header_capture_tests` | Nine and three passed respectively. Logs `/tmp/478-layout46.log`, `/tmp/478-header46.log`. |
| optimized47: `dto_capture_value_decode_matches_original_bytes_at_every_admission_boundary` | One passed, 0.03 seconds; `/tmp/478-capture47.log`. |
| optimized47: `request_header_capture_tests` first run | Three passed, four failed; `/tmp/478-headers47.log`. The four tests assumed already-omitted complete text references existed; speculative filter/tests were removed. No new omission policy or browser pass claimed. |
| optimized48: `request_header_capture_tests` | Four passed, zero failed, 1.02 seconds; `/tmp/478-headers48.log`. Formatting and whitespace checks passed; Clippy failed on a test-local import after statements. That import was moved; the corrected source still requires a new lint/build run. |
| spans50 build | Explicitly aborted before any tests because a Source oracle codec correction was not yet saved; exit 143, no owned processes remain. `/tmp/478-aborted-build50.json`. No pass claimed. |
| spans51: `request_span_` | Five passed, one fault-fixture assertion failed; `/tmp/478-request-spanspans51.log`. Selectors include `t08_t14_t15_t19_original_span_headers_match_every_legacy_field_and_advancing_page`, `t08_t15_t19_original_span_raw_ranges_preserve_original_bytes_without_materializing_headers`, `t08_t19_original_span_duplicate_ordinal_keys_preserve_legacy_last_wins_header`, and `t10_t19_original_span_insert_failure_rolls_back_optional_layout_and_preserves_native_start`. |
| spans51: read-layout / request-header / Capture DTO / request-start groups | Ten / four / one / four passed. Exact commands and logs: `/tmp/478-validationspans51-results.json`. |
| spans51: `durable::sqlite::` | 283 passed, one failed (the same fault-fixture assertion), one existing ignore; 88.58 seconds execution. `/tmp/478-new-storagespans51.log`. |
| spans52/53: formatting, whitespace, Clippy, six Source proofs and bins | spans52 formatting/whitespace passed; spans53 Clippy 74.464 seconds, Source six passed / zero failed (0.27 seconds execution), bins 77.412 seconds. `/tmp/478-validationspans53-results.json`. |
| clients49: protocol/Dev/TUI/Web typechecks, Dev tests, i18n, provenance, fake-provider tests | All passed; Dev 41 tests and fake-provider 54 tests. Exact commands and results: `/tmp/478-validationclients49-results.json`. |
| clients49: `pnpm --dir web-console test`, `pnpm --dir tui test`, `pnpm --dir web-console build` | Web 2,819 passed / 4 existing skips; TUI 1,158 passed / 1 existing skip; build passed. These run the unchanged client sources while the next private Store representation is under development. |

Integrated54 failed its complete library run: 4,018 passed, five failed and five
existing measurements ignored in 713.85 seconds (about 834 seconds for the
command). Each failure reached the unchanged 30-second `test liveness` guard at
`tests/scripted/app_server/mod.rs:410`; the final output does not identify the
await inside the scenario. The runner stopped before the remaining targets.
The failed selectors were:

- `app_server_file_read::delivery_cancellation_is_admitted_as_the_sixteenth_in_flight_request`
- `app_server_file_read::committed_present_reads_exact_native_scope_through_current_authorized_attachment`
- `app_server_file_read::delivery_cancellation_is_request_scoped_at_every_native_interleaving`
- `app_server_file_read::delivery_revocation_before_publication_commit_suppresses_sensitive_responses`
- `app_server_model_selection::children_created_after_selection_inherit_the_parent_and_running_children_allow_selection`

The same-binary isolated committed-present selector passed in 8.22 seconds
(`/tmp/478-failure-probe54.log`). This does not explain or supersede the full-run
failures (`/tmp/478-rustintegrated54.log`), and they are not classified as external
failures. A briefly started compiler began after the initial five failures and
was suspended until the run ended, so it cannot explain their onset. Codec55
quality/build results are recorded below; no budgets or assertions were weakened.
Group59 then ran all five file-read boundary scenarios and the exact Child
inheritance scenario on the already-built codec56 binary with the original
four-thread setting: all six passed in 3.70 seconds
(`/tmp/478-failed-group59.log`). This establishes the unchanged containing-group
rerun, not the cause of integrated54's failures. Full library run60 subsequently passed all five formerly failing cases, but
finished with 4,027 passed, one failed and five existing ignores in 476.38 seconds.
Its sole failure was
`scripted_suites::agent::tool_progress::parallel_calls_bound_progress_per_call_and_keep_canonical_order`: the unchanged ten-second run guard expired at
`tests/scripted/agent/tool_progress.rs:408` (`/tmp/478-lib60.log`). The whole
four-test tool-progress group then passed on the same codec56 binary, with the
original four-thread setting and ten-second guard, in 2.17 seconds
(`/tmp/478-tool-progress61.log`). The full-run timeout remains unlocalized; this
containing-group pass is not a cause or fix claim. Full final-source execution
and external targets remain required.

Codec55's structural codec group passed all 13 tests. Its Clippy run failed on
two test-local `items_after_statements` findings; only test-item placement changed.
Codec56 then passed Clippy (73.237 seconds), bins (75.703 seconds) and all-target,
all-feature no-run compilation (206.822 seconds). Build-artifact filesystem
writeback completed before the next timed diagnostic. These are focused/quality
results, not a replacement for integrated54's failed full run.

The unprofiled reading56 harness reached 240/300, then failed at 240,006.802 ms
and finished cleanup at 240,402.492 ms. At the 240th settlement (238,772.520 ms),
the native process had consumed 247.82 CPU seconds and reported 2,629,853,184
physical write bytes (about 2.63 GB). The same-binary reading57 run reached
254/300 before failing at 240,006.223 ms; cleanup completed at 241,013.297 ms.
Its intended strace attachment missed the deadline: the collector rejected the
already-gone process before attaching, so this run supplies no syscall trace.
Reading58 sampled at turn 200 for 8.001501 seconds, then failed at 252/300
at 240,005.910 ms; teardown finished at 240,504.299 ms. Its strace sample is
diagnostic evidence, not an unperturbed performance proof. Calculating the
`journal_lifetimes` array in `/tmp/478-reading58-pager.summary.json` gives 112
complete lifetimes: 107 normal commit patterns and five with mid-transaction
resync evidence (five journal resyncs after a database write). The sample counted
11,433 journal writes / 234 journal syncs and 14,347 database writes / 112 database
syncs. Journal sync calls accumulated 0.198223 seconds and database sync calls
0.097725 seconds; the target consumed 7.17 CPU seconds during the sample. These
observations do not by themselves locate the dominant CPU work or establish a
performance fix.

Direct cleanup verification found no reading56 harness/native PIDs 3759134 and
3759173 or `/tmp/rustx-web-console-r0UHjX`, and no reading57 PIDs 3782307 and
3782337 or `/tmp/rustx-web-console-w2AM8W`. Exact ownership metadata and progress
remain in `/tmp/478-reading{56,57}-meta.json` and corresponding `.log` files.
Reading58 metadata additionally identifies harness PID 3813729, native PID
3813752 and `/tmp/rustx-web-console-b3BqP8`; both processes and that directory
were absent on direct cleanup verification.
Cross-round canonical-fragment reuse is now implemented, with validation still
in progress. Producer-certified immutable subtrees are keyed by original scope,
digest, canonical position and current header evidence. Each request retains its
own source bytes and durable field rows; request-occurrence sources are rebound
to its actual sequence/item. Cache admission follows both optional-layout and
native transaction commitment. No cache performance result or complete validation
is claimed. Current-head CI, the original Linux exact regression rerun, a passing
complete native run and the maintained focused/full browser commands remain pending.

Fragments62 `cargo check --lib --all-features` failed on `Option::is_err` and an
unused `params` import; these were corrected to `is_none` and import removal.
Fragments63 bins passed in 118.229 seconds with two test-only dead-code warnings;
those helpers have since been scoped to tests. Fragments64
`cargo test --lib --all-features fragment -- --nocapture` compiled in about 160
seconds and ran seven passing / one failing tests in 1.27 seconds. Its sole
failure,
`t12_t19_canonical_fragments_preserve_reordered_repeated_and_compacted_native_occurrences`,
incorrectly unwrapped a native `InvalidReference` refusal at test line 375. The
fixture correction will assert refusal/no effects before using a real compaction
retry; it has not yet been rerun. Passing proofs include full source/field/range
equality, current-header/native-identity fencing, commit-only cache admission,
pending-memory bounds, and both Native layout fragment tests proving rebased
fields/ancestor previews, parser-row avoidance and bounded fallback. Evidence:
`/tmp/478-check-fragments62.log`, `/tmp/478-validationfragments63-results.json`,
`/tmp/478-fragmentsfragments64.log`.

The clean reading65 diagnostic used the bins63 binary under the original
240-second limit without heavy overlap. It failed at 254/300 at 240,006.447 ms;
cleanup finished at 240,331.669 ms. At turn 250 (233,581.544 ms), native CPU
consumption was 243.33 seconds and physical writes were 2,735,337,472 bytes.
Harness PID 3924087, native PID 3924127 and `/tmp/rustx-web-console-DJhBEE` were
absent on direct verification. This is no performance proof: the prior same-budget
reading57 also reached 254 (the traced reading58 reached 252). Runner66 is
validating the keys-only Source scan and corrected forbidden-reorder fixture;
fragment tests and Clippy are still running, with no pass claimed. The mandatory
maintained-browser blocker remains unresolved.

Fragments67 subsequently passed all 11 tests in the exact `fragment` group
(17.98 seconds execution / 177.816 seconds command duration), including the
corrected native refusal/compaction fixture. In
`t20_reading_300_pairs_report_actual_canonical_fragment_role_coverage_and_capture_work`,
16 genuine commits populated all 600 original descriptors (300 User and 300
Assistant). Resident retained bytes were 5,702,424; peak reserved bytes were
8,380,416, below 8 MiB. The next request hit all 600 entries with zero header
constructions; its parser imported 6,000 fragments and avoided 13,200 descendant
rows. These counters prove structural reuse, not a 300-turn execution-time or
browser pass. The small mixed-media fixture now admitted 13 entries / 125,400
resident bytes, versus the prior 12 / 577,441 bytes; changed eligible media means
those figures are not a like-for-like speed comparison.

The Native pruning proof
`t08_t14_t20_small_fragment_previews_are_not_retained_but_inline_views_remain_exact`
retained 5,581 bytes versus an eager 21,681 bytes for 11 nodes, while asserting
full Inline/OriginalSpan/Skipped header, byte and field parity. Exact evidence is
`/tmp/478-fragmentsfragments67.log`. Fragments67 Clippy passed in 72.776 seconds
and bins passed in 78.323 seconds. The clean reading68 diagnostic ran the
current binary after `sync -f target`, on the default filesystem under the
unchanged 240-second limit and without heavy overlap. It reached 281/300, then
failed at 240,005.040 ms; teardown completed at 240,449.062 ms
(`/tmp/478-reading68.log`). The profiled reading69 run reached 271/300 and failed
at 240,005.522 ms, with teardown complete at 240,927.307 ms
(`/tmp/478-reading69.log`). Profiling perturbs execution, so its wall time is not
a comparable performance result. All 11 targeted fragment tests and the
Clippy/bins passes remain valid evidence; latest-source full validation and the
mandatory focused/full browser commands are still pending. No completion or
maintained-browser pass is claimed.

Final70 client/protocol validation passed the commands below. These compile and
exercise the protocol/client state before the final native performance pin: native request-field path and insertion experiments were not yet pinned. The
path reuse remains; the batch experiment was subsequently removed as recorded
below. Final native/browser gates remain pending; no current-head CI pass is claimed.

| Final70 command | Actual result |
| --- | --- |
| `pnpm --dir protocol/app-server check` | Generation and drift check passed, 84.440 seconds. |
| `pnpm --dir protocol/app-server typecheck`, `pnpm --dir dev typecheck`, `pnpm --dir tui typecheck`, `pnpm --dir web-console typecheck` | All four passed. |
| `pnpm --dir dev test` | 41 passed, zero failed. |
| `pnpm --dir tui test` | 1,158 passed, zero failed, one existing skip; 104 suites, 40.393 seconds execution. |
| `pnpm --dir web-console test` | 2,819 passed, zero failed, four existing skips; 178 files passed / one skipped, 34.842 seconds command duration. |
| `pnpm --dir web-console build` | Passed, 5.651 seconds. |
| `pnpm --dir web-console check:i18n`, `pnpm --dir web-console check:provenance` | Passed; 276 source records and notices for 264 production packages verified. |
| `uv run --frozen pytest` in `test-support/fake-provider` | 54 passed. |

Exact command environments, durations and log paths are retained in
`/tmp/478-validationfinal70-results.json`. The fresh reading69 inclusive CPU
report places request indexing at 29.05%, field indexing at 18.55%, its callback
at 13.93%, and SQLite `sqlite3VdbeExec` at 18.37%. These inclusive stacks overlap;
24 ATOM samples were excluded. The percentages cannot be added or used to infer
wall time or a completed performance fix. New path/batch implementations still
require their own proofs and the original maintained browser run.

Batch71 formatting and whitespace passed, but its first batch test build failed
because `ValueRef` does not implement `ToSql`. The correction uses rusqlite's
borrowed `ToSqlOutput` adapter; no new encoding or parameter-copy policy was added.
Batch72 then passed three field-batch tests (0.35 seconds execution), all 15
read-layout tests (0.87 seconds), all 13 fragment tests (17.37 seconds), Clippy
(72.932 seconds), bins (77.778 seconds) and all-target/all-feature no-run
compilation (224.017 seconds). Exact evidence is in
`/tmp/478-validationbatch{71,72}-results.json`.

Before that Clippy/build sample, the full statement path gained `clear_bindings`
after either execution success or error, so SQLite cannot retain the previous
batch's parameters. The regression asserts cleared bindings through
`expanded_sql` NULL evidence. The current batch73 rerun passed all three tests
in 0.43 seconds execution (5.522 seconds command duration), plus formatting and
whitespace checks (`/tmp/478-validationbatch73-results.json`). These checks describe the historical batch experiment, which is no longer in
final source. Reading73 reached 276/300, then failed at 240,006.719 ms; cleanup
finished at 240,437.238 ms (`/tmp/478-reading73.log`). No end-to-end benefit was
established. The bundled SQLite immediate-foreign-key multirow path requires a
statement journal even with `OR FAIL`: see `fkLookupParent` near line 131840,
`sqlite3VdbeMakeReady` at 87507, `OP_Transaction` near 97163 and the pager write
path near 63195 in bundled `sqlite3.c`. These are source locations, not sampled
counts. Batching and its implementation claims were removed; earlier passing
batch tests remain historical evidence only. Request-field path reuse remains.

Prepared74 tests existing cached-statement reuse for the same SQL at seven hot
function sites: reading-cut capture, canonical-provenance selection (two queries),
Trace header lookup, inherited boundary lookup, ordinary User-by-ID selection,
fragment-key selection and `field_from`. This adds no result cache or budget
change. The reading69 stack evidence (`/tmp/478-reading69-stacks.log`) places
statement preparation at roughly 9% inclusive sampled CPU, not a promised 9%
speedup. Prepared74 passed all 11 fragment tests (17.05 seconds execution), six
request-span tests (0.36 seconds), 15 read-layout tests (4.53 seconds), Clippy
(73.092 seconds) and bins (77.177 seconds). Exact commands/results are in
`/tmp/478-validationprepared74-results.json`.

The subsequent reading75 diagnostic failed at 262/300 at 240,005.810 ms; cleanup
finished at 240,495.530 ms (`/tmp/478-reading75.log`). Harness PID 4142252, native
PID 4143805 and `/tmp/rustx-web-console-JC4rz8` were absent on direct verification.
No end-to-end speed fix is established. The maintained actual focused browser
command is now running (`/tmp/478-reading-focused76.log`), after which the current
full-test and real-browser failure inventory must drive remaining repairs and CI.
Full validation remains pending; no environment attribution or completion claim
is made.

Current verification gate: final39 default-ext4 browser reproduction still fails
both original `reading.spec.ts` fixtures at their unchanged 240,000 ms setup
budget, before UI assertions. The same final39 full Web suite passed 2,819 tests
(four existing skips), TUI passed 1,158 (one existing skip), and Web build passed.
The browser failure is unresolved; the final37/remainder38 results below describe
the source before the ensuing native request-validation optimization and must
not be treated as final validation of that optimization. No browser workload,
fixture timeout, worker count or filesystem override was changed.


The repair starts at PR HEAD `6f9cfbdb3ebd4b145ba5d54c044a91b0832fada3`.
The first remote fetch selected main `8cb41624eb2dc2ca87e4409961f42553ceb029cf`,
one squash commit beyond the previous base `97096020d64ea00218493a0a99b9b805821eeb28`:
PR #477 / Issue #466 immutable-history context projections. Integration uses a
normal merge in the existing `issue-467-bounded-observation` worktree; no rebase,
force push, unmerged dependency, or canonical-summary compatibility path.

This section supersedes the historical local results below for the repaired
source. Integrated final validation and current-head CI are not yet complete.
Original CI run 38674292393 failed Linux immediate recomposition, macOS fixture
path handling, and two 300-turn browser fixture setups; those are code/test
investigations, not classified as external infrastructure failures.

The Linux failure called semantic `ConversationRuntime::shutdown` through a
composition accessor. That settles execution but does not join the downstream
Projection worker. Its physical Store read can retain `ConversationAccess` and
the `Arc<ProductController>` owning the exclusive filesystem lock. Composition
shutdown now consistently owns semantic settlement followed by projection/read
join; callers retain the composition while awaiting or retrying cancellation.
`drain_projection` awaits the handle in its owning slot and removes it only after
termination. Dropping the composition or the waiting future is not shutdown proof.
Residency retirement separately retains its supervised composition and operation
leases through the same semantic-then-projection drain before writer transfer.

The physical Source handle now remains in a composition-owned slot even if the
Projection worker panics or is aborted. A joined worker outcome remains retained
while shutdown waits for that read, including cancellation of the shutdown caller.
The empty-slot branch releases its mutex before waiting; the deterministic panic
regression exposed and removed an idle lock retention bug. At lifecycle36, both
abnormal-worker drain tests, the source-panic test, immediate recomposition,
atomic compaction-head publication, explicit idle-source recovery and all four
source-failure tests passed. The complete final37 all-target result is recorded
separately below.

Source unavailability is distinct from Replay eviction. Failed native Source
reads reject checkpoint/service admission and emit typed unavailability to the
exact registration, without consuming a cursor or claiming execution failure.
A retained historical checkpoint cannot clear the failure. Only a successfully
validated native read at the current dependency fence restores publication.
Clients preserve Applied and stop automatic resynchronization until explicit
recovery; replacement identity fences late notifications.

Combined version domains: App Server 50, Runtime Client 71, SQLite 63; Runtime
Event 1, Subagent Control 33, private Product Host 4, and Session catalog 16 remain
independent. Main's checkpoint/activation identities, Original/Derived provenance,
and frozen Attempt request input replace obsolete canonical compaction summaries.

Repair regression selectors (execution scope is recorded per run below):

| Test selector | Boundary and synchronization evidence |
| --- | --- |
| `composition_shutdown_retry_joins_physical_read_before_immediate_recomposition` | Actual blocking Store read signals entry and logical cancellation; channel release gates physical termination. Cancelling the first shutdown waiter retains the original join handle. Retry stays pending until release, then weak owners expire and immediate real-lock recomposition succeeds. |
| `explicit_checkpoint_repairs_a_real_source_fault_on_the_original_worker_without_native_work` | Actual SQLite finite occupancy-source fault held through public checkpoint/subscription refusal; exact original BLOB restoration followed by public checkpoint wakes the original worker, publishes one read transition, preserves native cuts and makes zero model requests. |
| `unix_checkpoint_source_repair_keeps_the_current_thread_executor_serviceable` | Real Unix socket and JSONL server on a single-thread executor prove that checkpoint recovery yields the executor while the original Projection worker completes; persistent SQL fault stays unavailable and restoration advances one display transition without semantic work. |
| `projection_drain_joins_residual_read_after_worker_panic_and_waiter_cancellation` and `projection_drain_joins_residual_read_after_worker_abort_and_waiter_cancellation` | An actually blocked Source read retains its physical permit across worker failure and cancellation of the first drain waiter; retry stays pending until channel release, then preserves the worker outcome and proves both handles joined. |
| `committed_compaction_head_is_atomic_with_context_while_domain_reads_are_parked` | Two real native compactions with the read worker parked at the second completion; exact Completed-J, checkpoint cut and full contiguous replay already contain head activation two before background refinement. |
| `source_failure_is_unavailable_until_same_cut_native_read_repair` | Failed native read after a published checkpoint, no intervening semantic event, typed checkpoint/subscriber refusal, and successful same-cut source repair. |
| `read_failure_requires_successful_source_read_at_the_current_dependency_revision` | Installation under the Projection lock rejects obsolete read results; only the matching dependency fence clears source failure. |
| `failed_journal_batch_wakes_subscribers_without_inventing_a_trace_cut` | Native failure wakes observation without fabricating a cursor or execution fact. |
| `t18_child_source_failure_recovers_over_unix_at_the_same_activation_without_native_work` | Real Unix inspection transport preserves Child activation, typed unavailability and original source identity across repair without cold fallback or semantic work. |
| `manual_compaction_lost_commit_ack_publishes_the_original_receipt_after_owner_release` | Actual SQLite post-commit failure and held semantic owner prove original receipt publication ordering, unchanged canonical history and one compaction transition. |
| `giant_original_provenance_retains_scope_digest_and_exact_canonical_metadata_source` | A large original identity retains ancestor conversation scope and digest while its local canonical locator remains authoritative for deferred metadata. |
| `derived_provenance_admits_exact_byte_bounded_prefixes_without_copying_giant_aliases` | Combined encoded-byte admission retains exact provenance prefixes and explicit truncation. |

Generated v50 contracts pass drift checks at protocol13; protocol/Dev/TUI/Web
typechecks, Dev41 tests, i18n and provenance pass at clients17 after the provenance
change. Focused client coverage passes 27 TUI and 131 Web tests, including retained
per-page cuts and late reads after selection replacement. Full native execution
is recorded below; current repaired-source browser and remote CI qualification
remain separate gates.

Repair runs use `CARGO_INCREMENTAL=0`,
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1`, and `RUST_TEST_THREADS=4`.
From native5 onward, Cargo build concurrency is capped with
`CARGO_BUILD_JOBS=4`; test budgets and assertions are unchanged.

| Repair command / selector | Evidence so far |
| --- | --- |
| `cargo test --all-targets --all-features --no-run` (final37) | Passed, 135.848 seconds, including abnormal-worker physical drains and atomic compaction-head publication. |
| `cargo clippy --all-targets --all-features -- -D warnings` (final37) | Passed, 72.427 seconds. |
| `cargo build --bins` (final37) | Passed, 75.083 seconds. |
| `cargo test --all-targets --all-features` (final37) | Passed, 676.260 seconds: 4,684 passed, zero failed, ten existing ignores. Library 4,003 passed / five ignored; nine external targets 661 passed / five ignored; example test targets 17 and three passed. |
| `cargo test --all-features --test subagent definitions::skill_version_identity_is_frozen_across_the_boundary -- --exact --nocapture` (final37) | Original Linux CI failure passed again: one test, 0.560 seconds execution / 0.833 seconds command duration. |
| Nine external targets with `cargo test --all-features --test contracts --test conformance --test durable --test process --test subagent --test provider --test tools --test cfg3_catalog --test cfg3_managed_output` (final37) | 661 passed, zero failed, five existing credential-dependent Provider tests ignored; 314.134 seconds including dispatch. Per target: cfg3_catalog 26, cfg3_managed_output five, conformance 29, contracts 31, durable 147, process 62, provider 179, subagent 47, tools 135. |
| `cargo test --lib --all-features composition_shutdown_retry_joins_physical_read_before_immediate_recomposition -- --nocapture` (native6) | One passed; actual test 1.48 seconds. |
| `cargo test --lib --all-features source_failure -- --nocapture` (full25) | Four passed, including Host, actual App Server registration and real Child Unix public recovery. |
| `cargo test --lib --all-features manual_compaction -- --nocapture` (native7) | Eight passed, including lost-commit-ack publication. |
| `cargo test --all-features --test subagent definitions::skill_version_identity_is_frozen_across_the_boundary -- --exact --nocapture` (linux12) | Original CI failure passed, 0.56 seconds execution. |
| `cargo test --all-features --test subagent` (linux12) | All 47 passed, 41.25 seconds execution, including the original storage-lock regression. Historical request assertions now read the advertised bounded page at its original returned cut. |
| `cargo test --lib --all-features durable::sqlite:: -- --nocapture` (native7) | 251 passed, 19 failed, one existing ignored measurement. Repairs distinguish main's model-admission limits from canonical observation cuts, preserve native receipt evidence, and propagate typed deferred state. This historical failed run is superseded by the complete final37 pass. |
| Normal `pnpm --dir web-console test` (full3) | 2,819 passed, four existing skips; 31.18 seconds. |
| Normal `pnpm --dir tui test` (full4) | 1,158 passed, one existing skip; 37.238 seconds. |
| `pnpm --dir web-console build` (build1) | Passed. |

The initial full Web/TUI runs overlapped all-target Rust linking. They failed
three PTY cleanup cases and two held-A/B-settlement cases respectively; logs are
retained. The unchanged containing PTY suite then passed 19/19, followed by the
normal full Web/TUI passes above with no Cargo overlap. No timeout, worker-count
test configuration, fixture workload, or assertion was changed to obtain those
passes. This does not qualify the still-pending default-filesystem browser run.

The full9 native run discovered 4,002 library tests: 3,982 passed, 15 failed,
five existing ignored measurements. Cargo stopped before external targets. The
failures exposed integrated fixture assumptions about main's 1 MiB model-admission
limit, original-versus-derived request metadata, finite transcript heads, and two
actual bounded-storage defects: the completion expression index needed the exact
same guarded JSON expression as its query, and typed request-message deferred
state was lost when its finite header fit the byte window. Those production
repairs retain the original physical-work assertions and opaque-JSON negative
proof. Fourteen exact selectors passed at regress11; the final Stdio pagination
selector required a further typed-page fixture correction.

All-target linking wrote approximately 6 GiB of dirty build artifacts on this
host. An immediate process-capacity regression timed out at regress10. After
`sync -f target` completed, the unchanged case passed in 1.92 seconds at regress11.
Subsequent complete runs separate compilation, this artifact-writeback barrier,
and test execution. No test timeout, workload, assertion, or lock retry changed.

Final review also identified that a same-cut repair previously demonstrated by
a test-only read installation did not establish an idle production recovery
entry. Explicit checkpoint recovery requests one bounded native read through the
existing Projection worker. Concurrent requests at the same failed fence share
that read; failed reads stop until another explicit request. The original worker
retains physical cancellation/join ownership. Historical checkpoint validity
alone cannot clear source unavailability. The public-entry regression corrupts
and restores an actual SQLite finite source BLOB, verifies unchanged canonical and
Journal cuts and zero model requests, and recovers with the public checkpoint API.
The public native and exact-fence proofs passed at full20; actual Unix transport
proofs passed after the scheduling repair at diagnostic23. Complete final-source
validation remains required.

The full16 diagnostic rerun passed 3,996 library tests and retained five existing
ignored measurements; its single failure was the already-corrected Stdio test
that decoded a native canonical page preview as a presentation descriptor. All
other full9 failures passed in the complete library run. Cargo again stopped
before external targets. This binary predates explicit idle-source recovery;
it is not final-source validation.

The full20 focused run passed explicit public native recovery, coalesced/fenced
retry, Root Host failure and App registration failure/recovery. The real Unix
Child case exposed another production composition boundary: internal endpoint
checkpoint dispatch synchronously blocked the Tokio executor while waiting for
its Source worker, returning the five-second repair deadline instead of the
underlying source failure. Async endpoint checkpoint dispatch now awaits a
blocking call; the existing inspection connection task retains its physical
JoinSet slot until that call completes. A current-thread real Unix regression
makes executor progress a deterministic requirement. The fresh diagnostic23 binary passed the single-thread Unix proof in 0.275
seconds and the actual Child proof in 0.598 seconds; the complete final-source
run remains the next validation gate. The corrected T04 bounded Stdio pagination case passed at
diagnostic22 (2.45 seconds), preserving all sixteen original records and byte
frontiers.

The full25 final-source library run passed all 3,999 tests with five existing
ignored measurements. External cfg3 targets also passed (26 and five). External
conformance then passed 26 and failed three new-main image/coverage assertions:
they counted the bounded display message window as complete canonical history.
Their provider-request, retry-input and compatibility assertions had passed.
The repair changes the history oracle to authoritative native history while
preserving all original counts and semantic assertions. Complete external and
all-target reruns remain required; this partial pass is not full validation.

Conformance26 passed all three repaired selectors and its full 29-test target.
External27 then exposed a documentation-guard prefix bug: matching obsolete v5
also rejected current v50. The guard now requires a version-number boundary;
its tests retain exact v5 rejection and add v50/v51 acceptance. Contracts28
passed all 31 tests. No documentation exception or obsolete-protocol allowance
was added.

External29 found a real publication race in repeated compaction: committed
ContextCompacted generation/count advanced to two while the checkpoint's cached
projection head could remain at activation one. The finite Source selector was
correct; the Projection only installed the new head in a later asynchronous read.
The original Completed-J OccupancyObservation now carries its validated finite
projection head, captured before the witness is built and folded atomically with
the compaction facts. Optional capture failure cannot refuse native commitment;
it preserves typed unavailability. Original uncertain-commit receipts retain the
same head. Deterministic coverage blocks the asynchronous read so it cannot repair
or hide an incorrect intermediate publication.

Remaining31 passed external Process 62, Provider 179 (five existing credentialed
live tests ignored), Subagent 47 and Tools 135. Durable30 passed 146 and failed one
range fixture exceeding main's model-record admission bound; its repair retains
more than 9 MiB across 73 original records, each below 1 MiB, with original selected
range, secret, corruption, and physical-work assertions intact.

The shutdown proof audit also found an abnormal-worker branch: a panic while a
blocking Source read was owned only by the worker's local variable could detach
that read; removing the failed worker handle then let a later shutdown report
success without its physical proof. The repair retains the single original
physical read handle in the drain owner's shared slot, including across worker
failure and cancellation of a shutdown waiter. Both normal worker cleanup and
residual drain await handles in place before removing them. Deterministic gates
cover worker failure/abort with an outstanding read, not only clean cancellation.
The focused lifecycle36 results below execute both abnormal branches.

### Current repair execution: lifecycle36 and final37

The six lifecycle36 commands passed all ten selected tests: the Source-task panic
case (one), abnormal-worker drain cases (two), physical composition/recomposition
case (one), atomic compaction-head case (one), explicit idle-Source recovery (one),
and Source failure/recovery family (four). In particular, a failed blocking Source
task installs a bounded failure at its captured dependency fence; it neither
exposes the panic payload nor leaves the same dirty read spinning. The worker
panic/abort tests cancel the first drain waiter while the actual read still owns
its physical permit, require the retry to remain pending, then verify release,
join and the original worker error classification. Original lost-ack evidence
retains the head and cut captured with the native Completed J.

Exact commands, exit codes and log paths are in
`/tmp/478-validationlifecycle36-results.json`. Each command exited zero; test-only
execution times were 0.61, 0.72, 1.62, 2.53, 0.29 and 1.14 seconds respectively.
These counts refer to focused executions and are not additional distinct tests
on top of the eventual complete-library count.

Final37 passed diff/format checks, all-target/all-feature Clippy with warnings
rejected, default binaries, all-target/all-feature no-run compilation, and all
nine external targets listed above. Evidence is
`/tmp/478-validationfinal37-results.json`, with external summaries in
`/tmp/478-externalfinal37.log`. This also executes the repaired 73-record,
more-than-nine-MiB aggregate request-range fixture under native per-record
admission bounds. The complete `cargo test --all-targets --all-features` command
then passed on the same source in 676.260 seconds: 4,684 passed, zero failed and
ten existing ignored tests. The 4,008 discovered library tests comprise 4,003
passes and five opt-in measurement/generator ignores; external targets contribute
661 passes and five credential-dependent Provider ignores. The test-lane checker
and workload-list example test targets contributed 17 and three passes. The
original Linux CI regression also passed separately without changing its
assertions (0.560 seconds execution, 0.833 seconds command duration).

The separate post-full-run Linux boundary subset passed 235 tests and failed
one during actual managed-environment dependency preparation
(`/tmp/478-boundariesfinal37.log`). `uv sync --frozen --no-install-project
--no-default-groups --no-config` could not download `referencing==0.37.0`: its
request timed out after three retries in 126.1 seconds, before the managed
FastMCP scenario ran. The exact failed wheel URL was
`https://files.pythonhosted.org/packages/2c/58/ca301544e1fa93ed4f80d724bf5b194f6e4b945841c5bfd555878eea9fcb/referencing-0.37.0-py3-none-any.whl`.
A subsequent `curl` HEAD request to that same URL returned HTTP 200. The unchanged
exact selector
`boundary_suites::mcp_tasks_managed::a_real_managed_fastmcp_task_completes_through_one_tool_result`
then passed in 26.89 seconds (`/tmp/478-pypi-exact38.log`). No source, test,
dependency pin, timeout or retry configuration changed. The download failure is
retained as a failed command; the successful probe and exact rerun do not turn
that earlier command into a pass.

The unchanged complete boundary rerun and every remaining remainder38 gate
then passed. Exact commands, exit codes and log paths are recorded in
`/tmp/478-validationremainder38-results.json`:

| Remainder38 command / gate | Actual result |
| --- | --- |
| `cargo test --lib --all-features -- boundary_suites::` | 236 passed, zero failed or ignored; 110.930 seconds execution / 111.220 seconds command duration. Includes the original managed FastMCP test. |
| External boundary targets: durable, process, subagent, tools, conformance, cfg3_catalog and cfg3_managed_output | 451 passed, zero failed or ignored; 177.588 seconds command duration. |
| `cargo run --all-features --example check_test_lanes` | Passed, 15.522 seconds; every runnable Linux test is assigned to a CI lane. |
| `pnpm --dir protocol/app-server check` and `typecheck` | Both passed, 80.574 and 0.882 seconds. Generation/drift check covers v50 schema, TypeScript and fixtures. |
| `pnpm --dir dev typecheck` and `test` | Both passed; 41 tests, zero failed or skipped. Command durations 1.447 and 3.309 seconds. |
| `pnpm --dir tui typecheck` and `pnpm --dir web-console typecheck` | Both passed, 3.780 and 13.162 seconds. |
| Web `check:i18n` and `check:provenance` | Both passed, 0.905 and 1.148 seconds. Provenance verifies 276 source records and notices for 264 production packages. |

These subset reruns overlap the complete Rust test population and must not be
added to its distinct-test total. Full repaired-source Web/TUI execution and
browser/performance qualification remain separate gates. Final commit/push and
repaired-head remote CI are not established by these local passes. The original
CI failures and failed download command remain recorded above; no current-head
CI success is claimed.


## Complete Browser103 repair run

`pnpm --dir web-console test:e2e` completed with 311 passes and six failures
in 1587.828 seconds. Child capabilities remained Stopping during its original
five-second assertion. The commands case exhausted its 120-second test budget.
All four compaction locale/theme cases lacked the exact expected summary in the
Trace inspector. Both original 300-Attempt reading fixtures passed. These are
code/integration failures under investigation, not qualified infrastructure failures.
The frozen build did not contain the subsequent inspection-retirement patch.

## Repair104–107 validation preparation

Repair104 strict Clippy rejected two Option cloning expressions. Repair105 rejected
one test-module doc comment without code formatting. Both were corrected without
lint suppression. Repair106 compiled the combined repairs and found two stale
retry-index call sites after the helper signature changed. The optional display
index now receives the already selected transaction-local retry anchor; its
savepoint isolation remains unchanged. Repair107 targeted tests and strict Clippy
passed after those corrections.

Clients106 passed protocol/dev/TUI/Web type checks and tests, including 2,823 Web
tests. Web build then rejected the modified inspector's stale maintained provenance
hash. The exact local hash and treatment record were refreshed, preserving upstream
pins and license records. Clients107 Web build, i18n and provenance all passed.

## Browser110 interrupted for a final integration correction

The full browser rerun was deliberately interrupted after read-only review found
that live retry selected the last ordinary frozen input while inherited lineage
selected the opening adopted input. It completed 63 cases successfully, interrupted
one active case and did not run 253 cases (exit130,322.411 seconds). All six formerly
failed cases had passed again. This is not a complete-suite pass. A native ownership
correction and final-source validation are required before publication.

### Final opening-adoption repair validation

`repair111` compiled successfully and passed five of six new storage tests. The
sixth incorrectly expected a missing canonical display header to make the entire
RequestStart observation unavailable. Its corrected oracle checks the exact
selected retry-read error, preserved native R/J and C1 locator, and confirms that
the missing header is not silently repaired. All six pass in `final113`; all 27
response tests, including live/Fork opening identity after steering, pass in
`repair112`.

`final113` passed formatting, strict Clippy and binary build. Its complete Rust
command failed with 4,095 passing library tests, six failures and five existing
ignored cases. All six hit the unchanged 30-second liveness guard: the five
`boundary_suites::app_server_file_read` cases and
`children_created_after_selection_inherit_the_parent_and_running_children_allow_selection`.
The exact committed-present case passed separately; the full 237-test native
boundary suite then passed unchanged in `boundary115` (84.103 seconds). These
passes do not erase the full-run failure or prove its cause.

The shared test completion waiter previously ignored exact-target unavailable,
resync and closed notifications. It now fails immediately with the original
native settlement receipt and last observed cursor, instead of masking a typed
observation failure behind a generic timeout. Production behavior, completion
assertions and the existing timeout are unchanged. `final116` reruns full
validation with this diagnostic and backtraces enabled.

`final116` passed strict Clippy; its full library run passed 4,100 tests and
failed only the existing parallel Tool-progress ten-second liveness guard. All
six earlier App Server failures passed. Original-J diagnostics show both Tool
results committed, both retained progress tails at 255, and the next TurnStarted
at J269. The next media preparation unconditionally yields, so this is also the
first point at which the timeout may observe the preceding synchronous progress
commit burst; it is not evidence of a RequestStart deadlock.

`remaining117` passed the exact original lock regression, all 47 Subagent tests,
237 native boundaries, all 451 external Linux boundary tests, lane discovery,
protocol drift, and all 54 fake-provider tests. Separate unchanged exact-case
system-call diagnostics passed in 2.79 and 2.80 seconds. The wall-time profile
recorded 1,136 fsync calls (0.643 seconds), 44,698 pwrite calls (0.233 seconds),
and 51,711 selected calls total (0.924 seconds). Isolated timing does not prove
the cause of the full-suite timeout; no timeout or workload was changed.

Final121 passed strict Clippy, all four Tool-progress cases, and the complete
Rust all-target/all-feature command: 4,782 passed, zero failed, ten existing
ignored. All six earlier App Server failures and parallel Tool-progress pass
under the unchanged budgets. Lane discovery and protocol drift also pass.
Browser120 passed the complete 317 cases (1,499.771 seconds including setup),
with both original 300-Attempt reading scenarios and all six earlier failures.
The final diagnostics report typed observation interruptions immediately and
retain failure-only phase timings; no speculative production optimization was
introduced to chase the liveness result. Earlier failures remain recorded and
are not classified as proven infrastructure faults.

## Pushed merge CI: publication barrier follow-up

Merge HEAD `b8a961eb14760fda2bfa652f032ba493ef1301c4` integrates main
`8cb41624eb2dc2ca87e4409961f42553ceb029cf`. Run
[38800243633](https://github.com/Caismis/rustX/actions/runs/38800243633)
passed Linux boundary suites: 237 native boundary tests and 451 external tests,
including `definitions::skill_version_identity_is_frozen_across_the_boundary`.
Its deterministic lane failed `snapshot_dto_round_trips` (Unavailable instead of
Eligible) and `identical_explicit_selection_is_inert_during_an_attempt` (the
existing 30-second no-op liveness guard). This is not classified as infrastructure
failure; the previously passing local full run does not override the CI result.

The cfg-only `projection_settled` barrier checked an empty pending queue under raw
state alone. The worker can drain activation before installing its transition;
publication can also release raw state during encoding before installing the
candidate. The barrier now joins preparation, publication order, then raw state,
matching the existing worker-park lock order. It releases every lock before
waiting for another worker turn. Deterministic regressions hold each actual
boundary and observe the waiter's real `try_lock` contention before releasing it.
They preserve the original Eligible/model assertions and prove no model work.
The no-op fixture additionally reports its exact internal stage and bounded
projection state on failure; its gates, assertions, and time budget are unchanged.
Validation of this follow-up is recorded separately once executed.

`repair122`: both new settlement regressions, all 14 runtime protocol tests, all
6 no-op tests, formatting and strict Clippy passed. Its full Rust library run
passed 4,097 tests but reproduced the six 30-second boundary-fixture timeouts
previously seen in `final113` (five App Server file-read scenarios and the Child
model-selection scenario). The original CI two failures both passed within this
run. No SourceUnavailable/Resync/Closed diagnostic was emitted; the exact stalled
phase was not observable in that binary. The full command stopped at this library
failure, so its later external targets and planned lane/drift checks did not run.

The shared test helpers now retain cancellation-only phase diagnostics: native
handoff versus display publication, original/current settlement ordinal, last
matching cursor and existing bounded source state; request handling versus schema
initialization/validation versus checkpoint/subscription. They add no RPC, repair,
wait, or deadline change. The host diagnostic reference is weak. `diagnose123`
ran the exact six boundary tests together: all six passed in 3.43 seconds after
compilation. No process-wide environment mutation or earlier global-hook
contamination was found; this does not establish an infrastructure cause.

`diagnose124` full run passed all six App Server boundaries but failed three
`tool_deadlines` cases at their existing 5-second whole-execution guard:
`issue204_admitted_executions_obey_their_own_frozen_policy`,
`issue204_hard_deadline_bounds_a_hung_executor`, and
`issue204_idle_deadline_cancels_a_silent_execution` (4,100 passed, three failed,
five existing ignores). Inspection confirms the absolute deadline is frozen
before the executor's started signal; no missing clock-registration ordering was
found. These fixtures use an inert Runtime Client lifecycle. Failure-only bounded
clock/model/native-tail diagnostics now retain the exact phase evidence without
changing timers or deadline semantics.

At 21:17 UTC on October 15, host `/proc/pressure/io` reported approximately 85%
full I/O stall pressure while no task-owned test/build was running; memory stall
pressure was zero. This is environment context, not proof of the failures' cause.
`diagnose125` ran the complete 19-test Tool deadline module under `strace` at four
libtest threads: 19 passed in 4.41 seconds. The selected read/write/fsync trace had
137,336 calls, 3.093 aggregate syscall seconds, and no observed call above 5.753ms.
It did not reproduce the failure. Final local validation therefore also records a
serial-libtest run to separate cross-test I/O contention from the concurrency
inside each test. Per-test clocks, original guard budgets, workload and assertions
remain unchanged; CI's original concurrency and selectors remain unchanged.

The merge's macOS job `116647081973` ultimately ended cancelled. Its GitHub
annotation states `The job has exceeded the maximum execution time of 1h10m0s`.
It never reached native test execution; the active phase was harness compilation.
Latest main's job `116236434966` had completed that phase in 728.84 seconds
(`/usr/bin/time -l` peak RSS 3,116,613,632 bytes), with overlapping ordinary and
cfg(test) rustX compiler units. The merge's real-binary phase took 427 seconds
versus main's 287.57 seconds, but its harness phase exceeded an hour. Current
compiler logs/timing artifacts were unavailable after cancellation; memory
exhaustion, swapping, and external infrastructure failure are **not** established.

The macOS job now sets only `CARGO_BUILD_JOBS=1` to prevent overlapping compiler
processes, retaining `debug=1`, feature sets, all test selectors, original libtest
concurrency, assertions, overflow checks, and the 70-minute limit. Existing Cargo
HTML timings and `/usr/bin/time -l` remain the measurement authority. This is a
bounded compiler-resource change, not a test exclusion or a claim that the
resource diagnosis has already been proven. The next actual macOS run must
compile and execute the unchanged test suite before the lane can be closed.

`final126` passed diff, format, strict Clippy and binary build. Its serial full run
was deliberately interrupted after locating an existing child-harness handshake
bug in `deletion_cross_process_parent_death_preserves_nested_ownership`; no test
failure was suppressed or counted as a pass. Parent and child were both blocked
in `pipe_read`. Under `RUST_TEST_THREADS=1`, default libtest output prefixes the
child's readiness line with `test <name> ... `, but the parent requires exact
`OWNERSHIP_COMMITTED`. A fresh real-process reproduction confirmed the prefixed
line; the same subprocess with `--format=terse` emits the exact standalone marker
and exits successfully when released. Readiness-handshake child harnesses now
select terse output explicitly, retaining exact markers and every ownership
assertion. This avoids depending on the invoking libtest thread count; it changes
only test-harness presentation, not production storage or runtime behavior.

During `final127`, direct thread-state inspection caught native test threads in
Linux `rq_qos_wait` and `jbd2_log_wait_commit`, followed by normal test completion.
The saved 25-sample `/tmp/478-io-stall-final127.jsonl` includes the latter at
2026-10-15T21:46:49Z with full I/O pressure around 75%; `/proc/.../syscall` access
was denied. This establishes real storage waiting on this host, not the exact
cause of each historical timeout. No fsync, storage exclusion or timeout was
weakened. The original six App Server boundary cases, 19 Tool deadline tests,
new publication contention tests and original storage-lock test all passed in
the serial full run; final totals are recorded after the command completes.

Final127 completed successfully: `cargo test --all-targets --all-features`
passed 4,784 tests (zero failures, ten unchanged ignores), 2,212.844 seconds at
one libtest thread. Library: 4,103; external targets: 661; examples: 20. Full
Subagent: 47 passes, including the original CI storage-lock regression. Strict
Clippy (73.846s), formatting (4.530s), test-lane discovery (16.182s), and protocol
generation/drift (79.945s including compile) passed. The exact readiness-handshake
regression passed alone in 0.58 seconds and again within the full run. Final126's
binary build passed (79.229s); subsequent Rust changes are cfg-only tests.
Main was fetched again and remains `8cb41624eb2dc2ca87e4409961f42553ceb029cf`.
The follow-up retains all original CI test selectors and budgets and awaits its
own remote results; the earlier macOS cancellation is not counted as a pass.

### Review-fix validation 136 (before incremental manifest work)

`git diff --check`, `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo build --bins` passed. `CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features` passed in 2280.218 seconds: 4793 passed, 0 failed, 10 ignored across 20 target reports. The original `definitions::skill_version_identity_is_frozen_across_the_boundary` passed within the complete subagent target. This run compiled the R1/R2 fixes and captured-header reuse, before persistent-map/manifest work. The immediately following `cargo run --all-features --example check_test_lanes` failed to compile because concurrent persistent-map edits preceded the `imbl` dependency addition; it is not final-source lane evidence and must be rerun.

### Reading measurement 137 (captured-header reuse, before persistent manifests)

The same 300 sequential real App Server Attempts, bounded history reads, exact Message landing and Fork completed in 207.070 seconds; physical cleanup finished at 207.451 seconds and confirmed the owned native process and temporary workspace were gone. The unchanged 240-second setup guard was retained. Native CPU was 233.48 seconds; physical writes were 3,209,183,232 bytes and read-system-call bytes were 9,468,600,236. Per-Attempt medians for the first/middle/last 50 were 355.46 / 636.69 / 917.33 ms. This establishes continuing history-dependent growth: captured-header reuse is a constant-factor improvement, not resolution of the incremental-work requirement. These measurements used the binary built in review136 and do not validate subsequent persistent-manifest changes. `uv run --frozen pytest` in `test-support/fake-provider` separately passed all 54 tests (0.12 seconds).

### Incremental integration diagnostics (not final-head validation)

- `cargo check --all-targets --all-features --locked` (144) failed on composed
  module visibility and fixtures still accessing the retired fragment cache.
- `cargo check --lib --all-features --locked` (145) passed; obsolete-path warnings
  remained. This was not a Clippy pass.
- `cargo test --lib --all-features --no-run` (146) failed on six misplaced test
  fixture imports and 21 retired-cache references. The imports were corrected;
  the storage tests are being translated to the new authoritative representation.
- `cargo build --bins --all-features --locked` (147) passed with warnings. The
  resulting binary was used only for diagnostics while tests were still changing.
- The real App Server measurement (148) completed 100 sequential fake-provider
  Attempts, distant history reads, exact Message landing, fork and detach in
  86.940 seconds; physical cleanup completed at 87.015 seconds. Native CPU was
  64.88 seconds and physical writes were 694,910,976 bytes. Early/middle/late
  20-Attempt medians were 506.36/543.68/565.97 ms. Several large outliers remain
  unexplained (largest 17.591 seconds); this is not evidence of an infrastructure
  exemption. The workload guard was measurement-only. The original browser
  fixture still requires 300 Attempts within its unchanged 240-second guard.
- The 300-Attempt diagnostic (149), using the same build-147 binary, completed
  Attempts, history, exact Message landing, fork and detach in 194.968 seconds;
  cleanup completed at 195.246 seconds. Native CPU was 217.34 seconds, physical
  writes 2,245,373,952 bytes, and early/middle/late 50-Attempt medians were
  520.08/585.04/689.16 ms. This diagnostic is not either browser assertion test
  and does not establish final-head conformance. The prior build-136 diagnostic
  (137) took 207.070 seconds and showed medians 355.46/636.69/917.33 ms.
- Compile attempts 150 and 151 each found one stale test adaptation (old tuple
  work counter and unsigned SQL test value respectively). Both were corrected
  before the first focused integrated Source test (153).
- Direct execution of the compiled-153 test binary (155) passed four persistent
  input tests, six native mutation/persistence tests, two actual Store tests
  (including 100/300/1000 appends), three normalized-request/archive tests, three
  metadata-root tests, four cancellation tests and four compaction receipt tests.
  The media filter in that batch discovered zero tests; after correcting it to
  `model::media::tests::selected_tests::`, run 158 discovered and passed all four.
  These are targeted results, not a final full-suite pass.
- The first Source oracle (153) caught a genuine root-preview budget error:
  the composed root reused the scalar prototype's small-node classification even
  after inserting native messages. Coordinates and raw bytes matched, but the
  preview exceeded the old producer's node budget. The implementation was fixed;
  no expected-byte assertion was relaxed. Run 159 then passed the first exact
  byte/field/page oracle and found a sharing fixture still using the explicit
  cold full-input constructor for its second Request. That fixture is being
  changed to exercise the real native producer root for its sharing assertion.
- Diagnostic 152 stopped after 300 Attempts because `web_reading_surface`
  deliberately rejects request 301. This was a measurement-fixture limit, not
  a native hang. A separate `web_reading_scale_1000` scenario was added. Its
  test proves the original browser scenario still has exactly 300 reading
  Attempts plus its gated follow step, and the larger scenario preserves the
  first 300 responses unchanged. `uv run --project test-support/fake-provider
  --frozen pytest test-support/fake-provider/tests/test_scenario.py -q` (154)
  discovered and passed 21 tests.

- Diagnostic 156 completed all 1,000 actual Attempts, transcript/exact-source
  reads, fork and detach in 838.986 seconds; physical process cleanup completed
  at 841.459 seconds and the workspace was removed. It used diagnostic build
  147, not final code. Native CPU was 921.15 seconds and writes were
  7,841,017,856 bytes. This enlarged measurement uses its separate tested fake
  scenario; neither original browser workload nor its timeout was changed.
  Early/middle/late 100-Attempt median durations were 542.55/785.54/980.65
  ms. Remaining whole-request provider materialization/fingerprinting is not
  claimed to be constant work; the native Store tests measure the incremental
  selection and Source maintenance independently.

- Client validation 160 passed: `pnpm --dir protocol/app-server check` and
  `typecheck`; Dev `typecheck` and `test` (41 passed); TUI `typecheck` and
  `test` (1,158 passed, one existing skip); Web `typecheck`, `test` (2,823
  passed, four existing skips), `build`, `check:i18n`, and `check:provenance`.
  Protocol generation compiled before the last association-wrapper edits, so
  final drift validation remains required. The first formatting check in 161
  found a test-helper wrapping difference in `tests/support/context.rs`;
  `cargo fmt --all` corrected it.

- Direct compiled-161 native Store regressions (162) passed all five tests in
  154.83 seconds: 100/300/1000 incremental appends, AFTER INSERT canonical
  mutation rejection/rollback, unchanged compaction checkpoint reuse, completed
  Tool-history warm reuse, and relationship-mutation proof invalidation.
- The first Clippy pass after replacing the old layout (163) failed with 45
  lint diagnostics. These are being corrected in the owning modules; this is
  not a passing final lint result. Web type checking was repeated after making
  the failure-only reading diagnostic also report early setup rejection.

- Source suite 161 discovered 22 tests: 20 passed and two failed in 412.20
  seconds. The two failures were the near-limit fixture calibration and the
  obsolete expectation of Source leaf reuse across a new native compaction
  selection. Exact full bytes/fields/pages, old-read retention, optional rollback,
  tamper rejection and the 300-pair Source coverage test passed. Corrected
  fixtures must be rerun; these failures are not treated as a green suite.

## Full-chain preparation ownership audit (review174 onward, in progress)

Store-only 100/300/1000 results did not prove end-to-end scaling. The subsequent
review found old-input work above that boundary: staged input and contributor
body copies, flat request assembly, observation anchors and token measurement,
and post-commit Store reconstruction. Corrections carry the same immutable
compiled input root through those owners. Canonical contributor input remains a
separate canonical Surface selection, sharing Ledger body allocations; it never
substitutes compiled Derived input for canonical history. The existing Surface
owns its immutable selection and status/count indexes, updated on canonical
append or rebuilt from the exact durable selection at bootstrap.

The controlled pre-change native profile (review174) used the saved all-feature
library binary from review168, with no concurrent task-owned compilation or
other tests. Its 300 real scripted Attempts completed in 201.407s (one passed
test). Inclusive phase totals were: prepare 8.500303s, start 103.906528s, commit
81.603369s, Store reconstruction 19.303716s, and context projection 1.184694s.
Start includes commit and reconstruction; these totals must not be added. This
profile retains every fake adapter request, including its final diagnostic clone,
and does not measure actual provider encoding. It is not a substitute for the
real App Server/browser workload. The saved default-feature binary from review172
is separately running the unchanged real native fixture path at 1000 Attempts
(review175, measurement-only scenario). The original formal browser fixture
remains 300 Attempts with its original 240-second deadline.

Review171 passed the complete 22-test composed Source group, eight snapshot
validation tests, four cancellation tests, four compaction receipt tests and four
header capture tests. Its separate span group passed one test and failed one:
field enumeration returned the original serialized `$key` value rather than the
public `RequestObservationKey` descriptor. The correction derives that bounded
descriptor from the exact current rebased path and original key extent; it does
not alter the original raw key bytes. Verification of that correction and the
new full-chain changes is pending. Review173 strict all-target/all-feature Clippy
passed before these subsequent changes and is not final-tree validation.


Review175 completed the isolated pre-full-chain App Server measurement at 1000
Attempts, including history reads, exact Source landing, Fork and detach, in
864.990 seconds; physical cleanup completed at 867.225 seconds. The first and
last 50-Attempt median durations were 538.731 and 1093.178 milliseconds. No other
task-owned build or test ran concurrently, but recorded host I/O pressure was
high; this is not a claim of an otherwise exclusive machine. The preserved
binary and fixed measurement scenario permit a like-configured comparison.

Full-chain integration checks176–180 exposed compile/lint mismatches in cold
fixtures and owned canonical views. These were corrected without executing live
provider tests. Review181 strict all-target/all-feature Clippy passed. The first
compiled targeted run182 passed raw Source ranges but exposed an unsafe-integer
expectation mismatch in the new descriptor table; its complete independent legacy
header oracle had passed. Review183 exposed two initialization-test authorizer
rejections. Neither run is recorded as passing; their focused corrections require
fresh verification.

Review184 ran the remaining tests against the same compiled binary: 37 passed,
zero failed or ignored. These cover selected request roots (3), exact token
measurement (5), canonical selection (13), real Agent preparation at 100/300/1000
(1), native commit evidence (1), actual-only input refusal (1), same-transaction
Source proof reuse (1), cancellation ownership (4), exact compaction receipts (4),
Source recovery (2), physical shutdown retry (1), and same-cut Source repair (1).
The real Agent test proves bounded old-input preparation visits, zero selected
input flattening, zero post-commit reconstruction, exactly two scripted provider
requests and exactly one terminal event for the measured Attempt. It does not
claim provider wire encoding is constant work. Final full-suite/browser/CI
validation remains outstanding.


Review185 reran the repaired groups: Source spans 2 passed; native initialization
3 passed, zero failures/ignores. The unsafe-integer preview stays null/deferred
under the established JS-safe contract while its exact original 20 bytes remain
readable. The initialization authorizer allows SQLite's empty-column row-presence
callback only for the identity table; explicit attempts to prepare occupancy or
canonical body reads still fail with SQLITE_AUTH. These are test-policy
corrections, not relaxed production read bounds.


Reviews187–189 passed protocol generation/drift, protocol type checking, Dev
41 tests and type checking, formatting and diff checks. Client cancellation audit
also removed projected-Attempt admission in TUI and both Web dispatch layers;
the returned native identity owns the marker, and no refresh follows the receipt.
Focused commands, from their respective package directories:

- TUI `node --test test/cancellation-authority.test.ts test/control-readiness.test.ts test/root-checkpoint.test.ts`: 25 passed.
- TUI and Web `pnpm exec tsc --noEmit`: passed.
- Web `pnpm exec vitest run test/composer-cancellation.test.tsx test/agent.test.tsx test/root-checkpoint.test.ts test/session-product.test.tsx`: 96 passed.
- Web `pnpm exec vitest run test/session-product.test.tsx` after the final status row: 27 passed.

Initial focused Web failures identified its second generic observation-readiness
guard and an obsolete hardcoded fixture receipt; both were corrected before the
listed passes. These focused results do not replace final complete client/browser
validation.


### Controlled full-chain measurements (review190)

The default debug App Server binary completed the same 1000-Attempt measurement
scenario plus exact Source landing, Fork and detach in 600.121 seconds, versus
864.990 seconds for the preserved pre-full-chain review172 binary. Physical
cleanup completed at 602.350 seconds; the original process and temporary storage
were absent. Native CPU fell from 972.04 to 669.45 seconds. Physical writes stayed
approximately 7.85 GB: the change does not claim to eliminate durable writes.
There were no concurrent task-owned builds/tests. Both runs recorded high and
variable host I/O pressure, so these are qualified measurements, not a guaranteed
speedup on other machines. The formal browser fixture remains 300 Attempts/240s.

| Attempt bucket | Preserved baseline median ms | Shared full-chain median ms |
| --- | ---: | ---: |
| 1–50 | 538.731 | 516.556 |
| 251–300 | 715.535 | 554.062 |
| 501–550 | 836.302 | 574.533 |
| 751–800 | 974.549 | 582.782 |
| 951–1000 | 1093.178 | 602.734 |

The separate 300-Attempt scripted native profile passed with 600 preparation
leaf visits, zero old items revisited, zero estimator materialized items and
144,709 incremental hash bytes. Its measured Attempt interval was 222.121s,
compared with 201.407s in review174; overall scripted elapsed time did **not**
improve in this run. Inclusive prepare/start/commit/evidence phases were
4.852/90.565/84.602/0.071 seconds; the former post-commit reconstruction phase
was 19.304 seconds. These phases overlap and must not be added.

Source consumed 73.017 inclusive seconds: current-request scalar metadata capture
52.318s, capture18.975s (including new-leaf15.762s and authentication1.798s),
paths0.859s and length0.786s. Scalar capture omits messages and its per25-Request
cost stayed approximately4.33–4.44s. It is measured per-request work, not evidence
of old-message traversal or justification for a new cross-request cache. Source
query counts include authentication counts and are themselves included in native
index counts. No old canonical bytes, publication checks or storage exclusion
were removed to obtain these results. Formal browser and complete validation
remain distinct required evidence.


## Complete validation191–195: failures retained, corrections in progress

Review191 strict Clippy passed. The unqualified default-parallel
`cargo test --all-targets --all-features` then stopped after its library harness:
4,105 passed,34 failed,four ignored in1130.23s; external targets and lane checks
were not reached. Full diagnostics are preserved in the validation log. Two
assertions still named schema64 storage: the Request range test used the retired
content-segment counter, and old-schema rejection expected64 rather than65.
Their corrections preserve original bytes, fixed cut, cold reopen and actual
bounded physical reads. The unchanged canonical two-row test remains.

The same run exposed a real frozen retry-contribution mismatch and a media
projection work-bound violation; these require owner fixes and deterministic
reruns. Many other failures were original liveness guards (including native
Tool/Child startup); one Child interaction was no longer pending. Their causes
are not waived as infrastructure failures. One exact file-read rerun passed14.05s;
the stale Request two-row assertion reproduced independently. Remaining exact
and containing-suite validation is required. No correctness deadline was raised.
The long final SQL history-cost test passed after completing its original2056
requests; its cold fixture recreates the same nine selected leaves for the final
2048 requests. This is repeated fixed-size fixture setup, not production warm-root
reuse or evidence of quadratic history traversal. Debugger/perf sampling was
unavailable under the local OS policy; system permissions were not changed.

Full TUI193 passed1,163 tests with one existing skip. Web193 passed2,825,
failed three and skipped four: two tests awaited the removed automatic compaction
refresh; one Settings case repeatedly reopened the same unchanged detail for
each status assertion. The corrected tests explicitly invoke repair when testing
repair and read each unchanged detail once while preserving every assertion.
Their focused49-test run passed. Full Web194 then passed2,827,failed one and
skipped four: the real Office converter exceeded its unchanged15s deadline.
This run overlapped the heavy native harness, so a separate exact and complete
Web run remains required; it is not recorded as a passing suite. Web typechecking
passed. Fake-provider `uv run --frozen pytest` passed 55 tests.

### Review199–200 isolated client validation and final audit

With the prior native stress run fully terminated, the canonical Web command
`pnpm --dir web-console test` passed 2,828 tests, zero failed, four existing skips
in 34.902s. Production build, i18n and provenance checks also passed. The exact
Office integration file had first passed all four cases in 4.95s in isolation.
This resolves the local overlapping-work validation failure; it does not replace
the original browser reading fixtures or final-head CI.

The final ownership audit confirmed that `current_input` resets at every logical
turn, and corrective restaging retains only that step's committed contribution
receipts. Native start still validates the original Context bodies, selected input
membership and committed emission. Compaction protects those logical Context IDs;
approved media transformations retain their explicit Derived provenance.

Review200 strict all-target/all-feature Clippy passed. Its complete Rust test run
uses `RUST_TEST_THREADS=4`, with no concurrent task-owned heavy test/build workload.
It preserves every original timeout and assertion. Results remain pending here.
A fresh fetch confirmed main `8cb41624eb2dc2ca87e4409961f42553ceb029cf`, already
contained in the published branch; no additional merge was needed.

Review200 library result: 4,137 passed, three failed, four existing ignores,
983.99s. Both the corrected retry-contribution and production media-bound tests
passed, as did all eight R1/R2 native regressions. The failures were the first
three `boundary_suites::app_server_file_read` scenarios, each expiring the outer
30s guard during initial `session/attach` (call10), before tested file operations.
The short call durations (139us–300ms) do not measure prior fixture/schema setup.
A cold shared schema initialization under concurrent native stress is a hypothesis,
not proven by these logs. The entire five-case group then passed in 7.93s using
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1 cargo test --lib --all-features boundary_suites::app_server_file_read:: -- --nocapture`.
No guard, workload, assertion, or CI selection was changed. The complete serial
all-target/all-feature run (review202) is required to validate the integrated
source without the preceding local concurrency qualification.

Review202 completed: all-target/all-feature Rust passed 4,821 tests, zero failed,
nine existing ignores with RUST_TEST_THREADS=1; total 3,272.496s. Library 4,140
passed/four ignored in 2,761.96s; subagent 47 passed, including the original skill
identity/recomposition regression; Tool 135 passed; provider 179 passed/five ignored.
The lane checker passed in 15.989s with actual Linux discovery. No source changed
during this run, and no other task-owned heavy tests/builds ran concurrently.
Host I/O pressure remained substantial and is recorded separately.

Review203 passed the original skill-identity test alone (one discovered test),
default `cargo build --bins`, App Server generation/drift and TypeScript,
`cargo fmt --all -- --check`, and `git diff --check`.
Review201's first original reading browser selector passed with its unchanged
300-Attempt workload and 240s preparation budget on the default filesystem.
Playwright reported one passed in 3.5m and 4.1s for page assertions; the complete
wrapper (including native/web setup) took 260.172s, which is not fixture duration.
The independent Fork selector and complete suite remain pending.

The independent Fork-reading selector also passed: one test in 3.2m, page
assertions 6.2s, complete wrapper 193.437s. The same 300 Attempts/240s limit and
default filesystem were used; the full 317-case `pnpm --dir web-console test:e2e`
run began only after both independent selectors passed.

Review201 complete browser result: `pnpm --dir web-console test:e2e` passed
all 317 cases, zero failures/skips, Playwright 23.2m / wrapper 1,401.974s. Both
original 300-Attempt reading fixtures passed again inside this full run. The
default filesystem, pinned browser, one worker, 240s fixture deadline, original
assertions and screenshot baselines were retained. There were no concurrent
task-owned heavy builds/tests. Final-head GitHub Actions remains required.
