# Issue #533 delivery evidence

Source implementation and independent review are complete. This report records
local evidence and limitations. The PR description records the final committed
head and live FULL CI verdict/run; local timeouts below are not passing claims.

Base: `57447d2e825f914e80bbfecc68d373fcab487446`, the merge of prerequisite
[PR #540](https://github.com/Caismis/rustX/pull/540) for #532. Fixed Codex reference:
`rust-v0.166.0` / `6ff1e1ca3b30fc8b914720681fcf5fe62e95f856`.

## Whole-plan delivery matrix

| Package | Actual APIs / native owner | Implementation / tests / documentation |
| --- | --- | --- |
| AS-01 | v75 initialization; Thread/Turn identity, lifecycle, pagination, exact numeric codec; inherited connection authentication | #532 / #540 implementation and regression suites; `app_server::{protocol,wire,transport::auth}`, `session::threads`, `thread_status`; [shared-control contract](../app-server/shared-control.md). v75 extends the same generated contract. |
| AS-02 | Shared writable attachments, exact Turn start/steer/interrupt, mailbox and source CAS/application | Inherited runtime manager/Conversation writer and admission owners; shared-control, configuration and transport suites retained. |
| AS-03 | Bidirectional RPC, broadcast approvals/questionnaires/reviews, first-response/error claim and resolution | Inherited `app_server::callbacks`, Interaction Coordinator and bounded existing client handlers; arbitration/replay/retirement tests retained. |
| AS-04 | Dynamic Host Tool definitions, namespace identity, normal native Tool Runtime dispatch and settlement | Inherited Host definitions/callback adapter, provider and recovery suites; no new claiming/election semantics. |
| AS-05 | `command/exec`, write/resize/terminate and outputDelta; connection resources over the existing Bash supervision kernel | `app_server::command`, `runtime::process_runner::spawn_program`, native supervisor direct argv/I/O. Real process/PTY/disconnect/connection-identity boundary scenarios under `tests/boundary/app_server_environment.rs`. |
| AS-06 | Full fs method family, native changes/Diff, existing preview/upload/delivery/Download | `local_runtime::environment`, shared descriptor-relative `package_io`, `app_server::filesystem` watchers. Atomic fault, conflict, parent/root replacement and symlink unit tests plus real protocol/file/watch scenarios. |
| AS-07 | Native Model/Provider/Profile, source/effective/admitted configuration, Skills and MCP management | Existing source/catalog/application/Capability Coordinator owners; added shared Skill discovery/content reads and bounded complete MCP inventories through the existing probe cleanup owner. Existing source/revision/credential/MCP suites and new management scenarios. |
| AS-08 | Rust-derived schema/TypeScript/fixtures, existing clients, minimal connectivity example, whole-plan verification | v75 artifacts replace v74; existing client imports/transport version synchronize without UI redesign. [Environment wire/security/resource contract](../app-server/environment-services.md); current-head checks and measurements recorded below. |

Approved exclusions remain those in #532/#533: no standalone SDK or extra demo
products, client roles/scopes/Thread ACLs, exclusive Host Provider/claim protocol,
review/start product, generic Agent outputSchema, detached persistent commands,
parallel process service, billing/quota service, new OAuth issuer, plugin
marketplace, multi-tenant/distributed runtime or unrelated engine/UI redesign.
These exclusions do not waive required in-scope behavior.

## Validation ledger

| Check | Current result |
| --- | --- |
| Provider emulator locked Python 3.12 environment / pytest | 55 passed |
| Real command/PTY/filesystem/Skills/MCP protocol boundaries | 21 passed, including literal/reference HTTP/stdio credential echo, ready-gated PTY signals and missing-status settlement, atomic replacement deferred identity checks, unread-retirement cancellation, post-cap output termination, descriptor-bound watch ABA, canonical PTY EOF, PTY descriptor isolation, Host drain, native cleanup failure and instance-capacity cases; macOS adds its native child/capacity boundary in FULL CI |
| Native file fault/race/CAS tests | 6 passed in the full run |
| Rust formatting and all-target/all-feature Clippy (`-D warnings`) | Passed |
| Complete local Rust all-target/all-feature regression | Typed-boundary source `cd9a792f4`: 6061 passed / zero failed / 20 existing ignored across 22 targets. Subsequent Proxy-Authorization repair: 153 MCP tests and all 21 environment boundaries passed. Earlier 6036-test and focused 207-test results below remain historical. The final-head FULL run must validate the platform/target matrix. |
| CI lane coverage | Passed; Linux FULL and FAST selectors cover their required tests |
| Generated protocol/schema/fixtures and existing client typechecks | Generated-artifact drift check and all four package typechecks passed |
| TUI, dev and Web deterministic suites | dev: 43 passed; TUI: 1349 passed / 1 native skip; Web: 4204 passed / 4 existing skips |
| Browser acceptance | Full local pinned run: 660 passed / 7 expectation timeouts on the shared host. Focused diagnostics reproduced intermittent timing failures; native 34-Turn history preparation passed on untouched base and candidate, and the original browser assertions passed with temporary logging (removed). No assertion, timeout or screenshot baseline was relaxed. Final-head FULL browser acceptance is required in the PR. |
| Minimal external-client smoke | Real authenticated v75 initialize/server-info passed from outside the repository |
| Independent code review | Initial/shutdown/watch/EOF/descriptor findings repaired. Subsequent literal-credential, PTY-signal and busy-watch findings, redaction expansion/diagnostic caps, unread-retirement cancellation and actor fairness were independently re-reviewed: no remaining P1/P2 or weakened assertions. |
| Final-head FULL CI, including macOS and both browser shards | Pending at capture time; required on the final PR head. The PR description tracks its exact SHA, run and verdict. |
| Performance/resource measurements | Six source-identified baseline/candidate pairs and six native environment captures are retained below. The latest captures identify typed-boundary source `cd9a792f4`; every source/binary digest and raw sample count was independently verified. |

Native commit points, physical settlement and resource limits are specified in the
service contract above. Lost responses never authorize command/mutation replay.


## Native safety and review regression map

| Boundary | Regression evidence |
| --- | --- |
| Direct argv, no implicit credentials or model work | Buffered argv/binary/env boundary and zero-resident diagnostics; authenticated opaque/digest/JWT transport scenarios |
| Physical process tree settlement | Connection-local identity, terminate/disconnect/timeout boundaries and native supervisor settlement suites |
| Host shutdown ordering | Host drain cancels independent processes before waiting for their counted request owners; native process absence proves cleanup |
| Canonical PTY EOF | Current EOF settings, nonnewline combined/split writes, pending literal-next, and raw/disabled/conflicting-setting rejection before batch effects |
| PTY endpoint isolation | Both raw endpoints are atomically close-on-exec; the PTY program and another live ordinary command cannot inherit them, and the PTY completes while that unrelated command remains alive |
| Simultaneous PTY input/output | Raw PTY 64 KiB stdin against 128 KiB output, plus incomplete-prefix write returning unknown outcome |
| Watch authority / quiet idle / cleanup | Native descriptor registration under symlink/directory restoration and capture barriers, regular-file identity assertions; Linux Access filtering and capacity reclamation; macOS immediate child writes/metadata, bounded reconciliation, refusal and native teardown; real watch/unwatch/root retirement |
| Atomic file commit / path source identity | Native file faults, mode preservation, concurrent CAS, parent/root replacement, revocation while staging and post-commit sync uncertainty |
| Skills bounded I/O and ownership | Disconnect barrier retains admission; 32 MiB sparse-body range, large metadata chunk reconstruction and encoded-byte catalog pagination |
| MCP connection-scoped pagination | Public native management route walks empty continuation on one connection; cycles, oversized inventories and excess counts fail without publishing partial success or calling business resources |
| MCP native cleanup ownership | Both public management methods retain admission after a dropped waiter; held/failing native close gates prove pending/failed host drain, and four-slot capacity is isolated per instance |
| Secret readback | Native MCP credential-value/key redaction and existing source/transport credential-isolation suites |

The independent reviewer works read-only in this worktree. Findings are repaired
and reviewed before opening the primary PR; final status and the exact verified
head / FULL run are recorded in the PR description.

## Retained limits

Environment grants are instance User/Host policy, shared equally by authenticated
clients; they do not introduce client roles. Command cwd authorization does not
claim an additional OS process sandbox. Existing OS permissions and native
process policy remain authoritative. Native file CAS serializes this service's
writes, while unrelated commands/OS writers remain concurrent; there is no
cross-file/command transaction. Directory/tree operations are bounded and can
report unknown partial outcomes. Watch events are hints requiring rereads after
gaps. PTY EOF is canonical EOF input, not a raw-terminal half-close. No live paid
Provider, commercial CAD integration or external OAuth issuer was exercised.

The protocol intentionally replaces v74 with v75. All current clients, fixtures,
version negotiation, source inventory and the one standalone connectivity example
upgrade together. Their presentation and UI/UX remain unchanged.

## Performance and physical resource evidence

The untouched baseline is the merged prerequisite at `57447d2e825f914e80bbfecc68d373fcab487446`,
built freshly with `cargo build --bins --all-features`. The initial candidate measurements
use source commit `2e7c58bbaf9c8495211bdb7e09dfca03d1365aa0`, before the subsequent
watch-registration and canonical-EOF review fixes, and the same debug/all-features
profile. They remain historical evidence for that source, not measurements of
the follow-up implementation. The
existing Turn harness uses twenty Turns per mode: twenty provider requests with
zero Host Tools, forty with one Host Tool/continuation. Session names suppress
unrelated Title requests. The new environment harness uses five warmups and
twenty cycles per mode, then separately measures four live commands/eight watches.
It invokes zero Host Tools and zero models in every mode.

Both Turn pairs ran baseline then candidate. The first pair ran after Rust tests
and compilation ended while the long browser suite remained active; the repeat
pair ran after that browser suite also ended. This is still a shared developer
host, with twenty samples per mode, fixed ordering and no statistical or release
performance guarantee. Kernel write counters are not physical-device I/O. The
paused idle peer can fit in kernel buffers; saturation/slow-consumer correctness
belongs to the native bounded-queue and physical-settlement regression tests.

Raw evidence: [first baseline](baseline-measurements.json),
[first candidate](candidate-measurements.json),
[quiet repeat baseline](baseline-repeat-measurements.json),
[quiet repeat candidate](candidate-repeat-measurements.json),
[native environment cycles](environment-measurements.json).
Every file retains individual samples and source/binary identities. Candidate
JSON records the pre-commit HEAD with `workingTree: true`; use its measured-source
SHA-256 and binary SHA-256 to identify the changed implementation, rather than
mistaking that base revision for the built source. Both candidate Turn captures
have the same measured-source digest and binary; the environment capture uses
that same binary and records its own measured-path digest.
Both candidate source digests and the environment source digest were verified
against source commit `2e7c58bbaf9c8495211bdb7e09dfca03d1365aa0`; the measured binary
hash was also verified against its corresponding pre-follow-up local build.

The following quiet repeat values show **baseline / candidate**:

| Mode | CPU ms | RSS after MiB | Write calls | Kernel written MiB | Latency p50 ms | Latency p95 ms |
| --- | --- | --- | --- | --- | --- | --- |
| no-host-tools | 4870 / 5200 | 320.0 / 307.0 | 30733 / 30569 | 62.0 / 62.0 | 151.1 / 177.7 | 171.4 / 410.9 |
| host-one-client | 16380 / 15870 | 333.6 / 325.7 | 70604 / 70709 | 142.9 / 143.2 | 537.1 / 687.6 | 606.1 / 945.2 |
| host-three-clients | 24490 / 21530 | 345.1 / 330.0 | 70785 / 70784 | 143.3 / 143.1 | 576.0 / 515.5 | 647.7 / 582.6 |
| host-three-clients-one-paused | 21860 / 19550 | 345.9 / 331.7 | 70927 / 70644 | 143.2 / 143.1 | 563.6 / 523.8 | 602.0 / 557.5 |
| host-three-resident-sessions | 23150 / 21230 | 357.2 / 345.2 | 60217 / 60090 | 123.2 / 122.9 | 532.1 / 498.9 | 584.0 / 551.9 |

Observed regressions remain visible: quiet no-Host CPU increased 6.8%; its p95
rose from 171.4 to 410.9 ms, and one-Host p95 rose from 606.1 to 945.2 ms.
The first pair instead had higher three-client/paused-client p95 (987.6/969.5 ms
versus 650.0/648.3 ms baseline); those tails decreased in the quiet repeat.
Other quiet CPU modes and sampled RSS decreased. These samples demonstrate
variable timing, including actual regressions; they do not establish causation,
zero overhead or a supported throughput/capacity target.

| Environment mode | Parent / reaped-child CPU ms | RSS after MiB | Cycle latency p50 / p95 ms | Write calls | Kernel written MiB | Descriptors before / after / peak / cleanup |
| --- | --- | --- | --- | --- | --- | --- |
| environment-one-client | 280 / 60 | 144.0 | 32.9 / 42.3 | 608 | 0.08 | 19 / 19 / 67 / 19 |
| environment-three-clients | 320 / 60 | 145.2 | 35.3 / 40.5 | 620 | 0.08 | 21 / 21 / 69 / 21 |
| environment-three-sessions | 290 / 60 | 152.4 | 33.2 / 34.9 | 530 | 0.08 | 21 / 21 / 69 / 21 |
| environment-one-idle-paused-client | 280 / 70 | 147.0 | 33.6 / 37.9 | 630 | 0.08 | 21 / 21 / 69 / 21 |

All eighty measured environment cycles retained their descriptor baseline and
had only the App Server process after command completion. The separate peak
contained four admitted commands, eight watches and thirteen OS processes
(the server plus four supervised process trees); cleanup returned to one process
and the original descriptor count. Process-tree RSS is a sum that counts shared
pages repeatedly, and peak/setup work is excluded from cycle CPU/write deltas.
These are observed cleanup/cardinality facts, not a promise of constant RSS or
arbitrary concurrent workload support. Native caps and unknown-outcome semantics
remain authoritative.


## Watch/EOF review follow-up (before PTY descriptor isolation)

External review found a pathname-registration ABA race and incomplete canonical
PTY EOF. Both were repaired with retained descriptor identity, platform-native
observers and current-termios EOF handling, then independently re-reviewed with
no remaining P1/P2. The local 16-case environment boundary run for that source, strict
all-target/all-feature Clippy, production build and external connectivity smoke
passed. The actual native watch module and EOF function also passed isolated
Apple Silicon target Clippy with the repository's pinned native dependencies;
the macOS runtime boundary remains part of final-head FULL CI.

The follow-up source is `a5dd0534850b25ac15fc2619c6fb71216baf6073`.
Its binary SHA-256 is
`1c59b23fc2b116b4e45f445afab42291eb3796207cc78d30d12ccb1a2c0a4264`.
Two fresh environment captures use that binary:
[first follow-up](environment-followup-measurements.json) and
[repeat follow-up](environment-followup-repeat-measurements.json).
Their measured-source SHA-256 is
`d4aeba44c4f4ebc5951449fb7bedfacdd99304b15ecd25e362cf01acbc4a1e97`,
verified against that source commit. The harness records `workingTree: true`;
the source digest, source revision and binary digest identify the measured code.

The following values show **first follow-up / repeat follow-up**, using the same
shared-host, five-warmup/twenty-cycle method and four modes described above:

| Mode | Parent CPU ms | Reaped-child CPU ms | RSS after MiB | Cycle p50 ms | Cycle p95 ms |
| --- | --- | --- | --- | --- | --- |
| one client | 540 / 260 | 110 / 50 | 156.7 / 156.3 | 113.5 / 32.9 | 1494.5 / 33.8 |
| three clients | 430 / 230 | 90 / 60 | 157.6 / 157.9 | 109.3 / 32.3 | 140.1 / 33.3 |
| three Sessions | 570 / 520 | 110 / 80 | 160.7 / 162.0 | 47.1 / 41.1 | 49.4 / 46.5 |
| idle paused client | 370 / 470 | 80 / 90 | 155.9 / 157.2 | 86.6 / 42.8 | 109.4 / 44.4 |

Every one of these 160 cycles retained the descriptor baseline (19 for one client,
21 for the other modes), invoked zero models and left one server process after
command completion. Peak descriptors increased from the earlier source's 67/69
to 75/77: each of the eight active watches now retains one additional authorized
source descriptor. Cleanup returned to 19/21 descriptors and one process; the
four-command peak still contained thirteen processes.

Timing and CPU regressions remain visible, including the first capture's
1494.5 ms p95 and the repeat's three-Session/paused-client CPU of 520/470 ms
versus the earlier source's 290/280 ms. The first and repeated captures vary
substantially on the shared host. Neither the improvements nor regressions
establish causation or a production performance guarantee. macOS descriptor
budgets and runtime correctness are tested natively in CI; these Linux samples
are not macOS measurements.


The watch/EOF-source Turn comparison uses a freshly measured untouched base followed
by source `a5dd0534850b25ac15fc2619c6fb71216baf6073`:
[baseline follow-up](baseline-followup-measurements.json),
[candidate follow-up](candidate-followup-measurements.json).
The candidate's Turn measured-source SHA-256 is
`60a85e30215c20dbbff4fdc7ce88e641bed0228b034ba74a03a737259454f429`,
verified against that source commit; its binary matches the environment captures
above. Both processes completed all five modes with exactly twenty native Turns
each, twenty provider requests without Host Tools or forty with Host Tools.

These values show **baseline / watch/EOF-source candidate**:

| Mode | CPU ms | RSS after MiB | Write calls | Kernel written MiB | Latency p50 ms | Latency p95 ms |
| --- | --- | --- | --- | --- | --- | --- |
| no-host-tools | 4670 / 4720 | 318.5 / 318.3 | 30636 / 30601 | 61.9 / 61.9 | 150.2 / 187.6 | 166.5 / 225.8 |
| host-one-client | 17770 / 18010 | 339.8 / 340.0 | 70762 / 70879 | 143.1 / 143.1 | 559.9 / 636.8 | 594.0 / 724.1 |
| host-three-clients | 23550 / 22980 | 347.0 / 344.1 | 70675 / 70741 | 143.0 / 143.2 | 568.9 / 618.7 | 621.9 / 690.4 |
| host-three-clients-one-paused | 21620 / 21900 | 347.5 / 344.7 | 70588 / 70659 | 142.9 / 143.1 | 554.3 / 615.8 | 620.4 / 719.2 |
| host-three-resident-sessions | 22740 / 23440 | 359.3 / 361.0 | 60253 / 60118 | 122.9 / 123.1 | 526.5 / 610.9 | 794.2 / 689.8 |

This pair retains higher p50 in every candidate mode and higher p95 in four
of five modes; only the three-Session p95 decreased. CPU ranges from a 2.4%
decrease to a 3.1% increase, with small sampled RSS/write differences. Fixed-order
shared-host debug samples cannot attribute those movements to the change, and
no zero-overhead or production-throughput claim follows from them. Earlier pairs
and both watch/EOF environment captures remain visible rather than selecting only
the better measurements.

## PTY descriptor-isolation follow-up

A real App Server probe found that `openpty` left its master and slave inheritable:
an unrelated command received both endpoints, allowing cross-command terminal
access and delaying terminal EOF while its inherited slave remained open. Source
`7433431710164e3b6f3889e5019337ffd1c2c327` replaces this allocation with the existing
rustix PTY APIs and atomically close-on-exec opens on both Linux and macOS. It
preserves native default terminal attributes and the current-termios EOF contract.

The independent reviewer found no remaining P1/P2. All 17 native environment
boundaries passed, including inspection of each program's inherited descriptors
and complete PTY settlement while an unrelated ordinary command stays alive.
Strict all-target/all-feature Clippy, production build and the authenticated
external Node example passed. A real App Server `/proc/self/fd` probe now shows
only standard I/O and its own directory iterator; no other command's PTY ends.
The actual complete Terminal implementation and native watch module also passed
isolated Apple Silicon target strict Clippy with the pinned dependencies. The
preceding FULL run on `a24637339b7c9121e94f2414c53f4d0474d4e04a` passed all selected
jobs, including all 667 browser cases and native macOS; it predates this fix and
is not final-head validation. The PR records a new complete FULL verdict.

Fresh final-source captures retain the same method and all earlier observations:
[baseline](baseline-final-measurements.json),
[candidate](candidate-final-measurements.json) and
[environment](environment-final-measurements.json).
The candidate binary SHA-256 is
`94a369ccaff3828e5ed3474daae9fb699a91241cfb1e69e54a06711c3bc469e3`.
Turn/environment measured-source SHA-256 values are respectively
`0732535b862d3c633febe4cd120a064b1f712bc55224ee98b0f107fa3dd7cdf7` and
`e652861076f8844f7a5d22cdd66c737f4a463b86360c38fca36ff9122dd951f8`.
Both were independently recomputed from the committed source above; the baseline
source and binary were also checked against the unchanged prerequisite revision.
Each Turn mode completed twenty native Turns with exactly twenty model requests
without Host Tools or forty with Host Tools.

Values below show **baseline / final candidate**:

| Mode | CPU ms | RSS after MiB | Write calls | Kernel written MiB | Latency p50 ms | Latency p95 ms |
| --- | --- | --- | --- | --- | --- | --- |
| no-host-tools | 7220 / 4520 | 325.5 / 319.1 | 30919 / 30573 | 62.0 / 61.8 | 646.3 / 146.4 | 5829.7 / 160.3 |
| host-one-client | 18280 / 17610 | 342.3 / 340.4 | 70706 / 70777 | 143.1 / 143.2 | 782.7 / 561.5 | 949.1 / 593.5 |
| host-three-clients | 23670 / 23330 | 345.6 / 348.0 | 70824 / 70496 | 143.3 / 142.8 | 743.3 / 548.4 | 948.9 / 618.9 |
| host-three-clients-one-paused | 21740 / 20280 | 345.1 / 344.1 | 70796 / 70799 | 143.3 / 143.1 | 555.7 / 555.4 | 599.9 / 609.7 |
| host-three-resident-sessions | 23600 / 23220 | 362.6 / 361.6 | 60039 / 60150 | 123.1 / 123.0 | 540.5 / 530.6 | 596.1 / 581.9 |

All four environment modes completed twenty cycles with zero model requests.
All eighty descriptor samples retained 19 descriptors for one client or 21 for
other modes; cleanup returned to the same count and one server process. The
four-command/eight-watch peak remained 75/77 descriptors and thirteen processes.

| Environment mode | Parent CPU ms | Reaped-child CPU ms | RSS after MiB | Cycle p50 ms | Cycle p95 ms |
| --- | --- | --- | --- | --- | --- |
| environment-one-client | 500 | 100 | 156.5 | 43.3 | 47.2 |
| environment-three-clients | 390 | 80 | 156.1 | 34.4 | 47.2 |
| environment-three-sessions | 500 | 90 | 162.0 | 41.9 | 44.6 |
| environment-one-idle-paused-client | 430 | 90 | 156.9 | 41.9 | 45.7 |

These shared-host debug samples preserve all observed movements and do not
establish causation, zero overhead or production throughput. Setup and the
separate resource peak are excluded from cycle deltas; paused idle peers do not
prove saturated-output behavior. Native boundary tests own that assertion.

In this last pair, sampled parent CPU is lower by 1.4%–37.4%, while the paused
client p95 increases from 599.9 to 609.7 ms. The baseline's no-Host p95 of 5829.7 ms
is a large shared-host outlier relative to earlier pairs. It remains in the raw
samples and table; neither it nor the opposite-direction earlier regressions
justify attributing a performance gain or loss to this PTY allocation change.

## Browser screenshot checkpoint follow-up

FULL attempts 1 and 2 on `357796f0fad4ed5454880b38ace3b94ff4d7837f`
passed all Linux and native macOS checks. Linux completed 6,042 tests with 20
existing ignored cases; macOS included the new PTY endpoint-isolation and native
watch child/capacity/teardown boundaries. Browser shard 2 passed all 344 cases.

Attempt 1 passed 322/323 cases in shard 1; the Workflow Editor reference had
sixteen changed Bezier-edge pixels, with four above their existing individual
bounds. Independent reconstruction found all six graph screenshot checkpoints,
SVG paths/order/transforms, ancestor geometry and all five CSS resources identical
to retained reference captures. The complete original test passed in the pinned
local browser, then also passed in the second complete shard attempt. No raster
allowance or reference was changed, and the browser's internal cause is unknown.

Attempt 2 passed 322/323 cases; the sole failure moved to the narrow Light
Providers reference. Its 2,458 changed pixels were confined to the Provider strip
at x25..308, y307..346. Trace `22843` proves Playwright's click auto-scroll changed
the grid's `scrollLeft` from zero to nine before dispatching the click. The row
was then correctly selected and focused; all three stable screenshot checkpoints
retained the nine-pixel offset. The remaining page matched its frozen reference.
This was a real viewport difference, not raster noise or a product-source change.

The screenshot test now verifies the actual selected/focused row, sets the
Provider strip to its explicit origin once, asserts `scrollLeft === 0`, and then
uses the original exact screenshot comparison. Other repository screenshots use
the same explicit scroll-position setup. There is no conditional retry based on
pixels, extra wait, timeout increase, baseline update or tolerance change. Product
WebUI/TUI components, CSS and UX are unchanged. Web typecheck and all four original
Light/Dark x 1440/390 complete editor scenarios passed in the pinned browser.
A separate local diagnosis forced and asserted a persistent nine-pixel offset
immediately before the new precondition; the complete Light 390 flow passed
with selection and focus retained. That temporary injection was removed before
commit, and the independent reviewer verified the diagnostic trace.
The final committed head requires a new FULL run after this test fix; its exact
run and verdict belong in the PR. The two preceding attempts are not final
all-green validation. Product source and all performance captures above are
unchanged by this test-only follow-up.

## Subsequent native-owner review and CI follow-up

FULL [run #1776](https://github.com/Caismis/rustX/actions/runs/42420431724)
on `fd73e29878f9452590a034bc9032c803b78f4bb9` passed Linux (6,042 tests,
20 existing ignored), both browser shards and coverage (all 667 cases, zero
skips), client/protocol checks and macOS Desktop/Host. Native macOS FULL failed
the existing raw duplex PTY boundary: a pending stdin batch reported unknown
partial effects and the command reported connection closure. It completed
4,194 other in-crate tests with eight existing ignored; later external targets
did not run. This is a failed run, not final-head validation.

A subsequent external review confirmed three independent native boundaries:

- Literal MCP transport environment/header values were absent from catalog
  redaction. The native binding/connect owner now includes every accepted
  literal and resolved reference in its private credential projection. Real
  HTTP and stdio peers echo received credentials into all three catalogs,
  including nested schema keys and values. An isolated child test process
  supplies synthetic host references without mutating global test environment.
- PTY foreground signals also reached the lifecycle supervisor. Caught
  TERM/INT/QUIT/HUP and terminal job-control handlers now preserve that owner
  while exec restores business-program defaults. Real ready-gated programs
  handle INT/QUIT and continue, or terminate with their defaults. Physical
  containment without a business exit status now settles as a control failure
  before the command deadline, while retaining actual reaping requirements.
- A watch callback's busy identity check could leave it bound to a replaced
  inode forever. The existing owner now performs a serial deferred check with
  one coalesced pending recheck wake-up. A barrier after rename and before parent
  fsync proves both file/directory callbacks encounter the held I/O gate; the
  file watch then retires and releases capacity while the unchanged directory
  continues observing another retained child. A second held-gate barrier proves
  pending native checks retain capacity through unwatch/disconnect and Host
  teardown, then return all slots after actual completion.

Independent follow-up review also found and repaired two adjacent issues.
Redaction scans original text once, so credential values matching substrings
of `<redacted>` cannot recursively expand inserted masks; a regression includes
every nonempty mask substring and unmatched UTF-8. Final catalog size is checked
after redaction as well. Tool/resource final failure statuses retain their
existing 1 KiB diagnostic budget after redaction, including short values matching
mask text; real HTTP error responses exercise both final executor exits. A peer
marker and resource-read count prove those errors originate from the actual
HTTP requests. Unwatch cancels a withdrawn retirement-notification receipt wait.
A real queued-retirement barrier verifies all 32 observer slots return while
the connection remains alive and never reads that notification.

The macOS duplex failure exposed a continuously ready PTY producer's ability to
fill its 32-slot observation queue within one task poll. The command actor now
cooperates after completed select turns, including after reaching its capture
cap. It never yields inside a read future after consuming bytes and does not
wait for individual output receipts while servicing stdin. Queue bounds,
saturated-consumer retirement, final transport-acceptance fencing and original
duplex assertions remain unchanged. Both real pipe and PTY infinite-output
regressions observe the cap notification before requesting prompt termination.

All of these repairs remain inside the existing native owners. WebUI/TUI
presentation, screenshot references, retry policies and timeouts are unchanged.
Independent read-only review of the implementation and incremental lifecycle
changes found no remaining P1/P2 issues or weakened assertions. All 207 relevant
native regression tests passed, as did the strengthened HTTP peer-origin test,
strict all-target/all-feature Clippy, formatting, production binary build and
authenticated standalone Node example. Earlier captures above remain historical
evidence for their explicitly identified sources. At capture time the final-head
FULL run remains pending; after pushing the final head its exact SHA, run and
verdict must be tracked in the PR, independently of these local checks.

### Measurements after the native-owner repairs

Product source is `4903a216b93625967b9b7962160ce4e53cb51cb6`. After all local
compilation and regression work ended, the unchanged harnesses ran baseline,
candidate, then environment modes sequentially on the same shared host:
[baseline](baseline-review-measurements.json),
[candidate](candidate-review-measurements.json) and
[environment](environment-review-measurements.json). Earlier samples were retained.

The baseline remains prerequisite `57447d2e825f914e80bbfecc68d373fcab487446`
with binary SHA-256
`c2532fa4c9560388e4b67190f8da2555d40dcf0dd15a257746ce3c7ed8f0ac19`.
The candidate binary SHA-256 is
`9ebcb1ffb4b9ca650bd12c7882ac79cadc51faa2264834e163afa4d348ae976d`;
Turn/environment measured-source SHA-256 values are respectively
`f5d8c3f195d91516a8ec7afaa21cecad93cd50115917ba2918dcd3d7c95f8e5c` and
`a2ac4337104083c68c7aed2d181388851c616645c1fcf2c79a400c0b182077d8`.
The recorded revisions, source digests and actual binaries were checked against
the unchanged committed source in each checkout. `workingTree: true` remains the
harness's fixed capture marker; it does not replace those identities.

Every Turn mode completed twenty actual settled native Turns with twenty model
requests without Host Tools or forty with Host Tools. Values show
**baseline / repaired candidate**:

| Mode | CPU ms | RSS after MiB | Write calls | Kernel written MiB | Latency p50 ms | Latency p95 ms |
| --- | --- | --- | --- | --- | --- | --- |
| no-host-tools | 6950 / 4610 | 326.7 / 318.5 | 30908 / 30542 | 62.14 / 61.92 | 333.5 / 146.5 | 5751.2 / 165.4 |
| host-one-client | 18290 / 16610 | 340.3 / 337.8 | 70721 / 70623 | 142.95 / 143.08 | 762.5 / 529.3 | 1063.8 / 615.2 |
| host-three-clients | 23580 / 23490 | 347.5 / 349.8 | 70739 / 70672 | 143.20 / 142.91 | 746.7 / 560.8 | 967.1 / 594.0 |
| host-three-clients-one-paused | 21650 / 21810 | 346.6 / 347.5 | 70684 / 70776 | 143.06 / 143.05 | 562.8 / 555.5 | 587.5 / 602.0 |
| host-three-resident-sessions | 23140 / 22730 | 361.3 / 363.2 | 60216 / 60203 | 123.11 / 123.09 | 534.9 / 535.5 | 574.2 / 570.0 |

Regressions remain visible: paused-client CPU increased from 21,650 to 21,810 ms
and p95 from 587.5 to 602.0 ms. RSS increased in the three-client, paused-client
and resident-session modes, while other CPU/timing observations moved in both
directions. The baseline no-Host p95 of 5751.2 ms is another large shared-host
outlier, retained in full. These debug samples do not establish causation,
statistical significance, zero overhead or production throughput.

All four environment modes completed twenty cycles, with zero Host Tools/model
requests. Every one of the eighty descriptor samples retained its original
19/21 count. Four admitted commands/eight watches reached 75/77 descriptors and
thirteen OS processes; cleanup returned to 19/21 descriptors and one process.

| Environment mode | Parent / reaped-child CPU ms | RSS after MiB | Cycle p50 / p95 ms | Write calls | Kernel written MiB | Descriptors before / peak / cleanup |
| --- | --- | --- | --- | --- | --- | --- |
| environment-one-client | 260 / 50 | 156.1 | 33.0 / 34.7 | 706 | 0.08 | 19 / 75 / 19 |
| environment-three-clients | 420 / 80 | 157.3 | 41.2 / 43.5 | 678 | 0.08 | 21 / 77 / 21 |
| environment-three-sessions | 530 / 90 | 161.1 | 42.1 / 44.7 | 583 | 0.08 | 21 / 77 / 21 |
| environment-one-idle-paused-client | 440 / 90 | 157.3 | 42.0 / 46.6 | 611 | 0.08 | 21 / 77 / 21 |

Setup/peak work is excluded from cycle CPU/write deltas. Kernel counters are not
physical-device I/O; summed process-tree RSS repeats shared pages. The paused
idle peer does not prove saturation behavior, which remains covered by native
bounded-output and physical-settlement regressions. Resource return is an
observed cardinality fact, not a general workload/capacity guarantee.

### Typed credential boundary follow-up

The external follow-up review of `1ff65c8fbaaa5d95c546890ef04338d2fd701e63`
confirmed the PTY signal/settlement and watch retirement repairs, then found two
remaining credential defects. An Authorization header's complete value was
masked while its Bearer token could still be echoed. Blanket redaction of the
finished catalog/status also altered native DTO keys and connection-generation
facts when an ordinary literal value was `name`, `tools`, `e` or `1`.

The connection owner now collects full Authorization values and Bearer tokens
through the same rule for literal and referenced headers. Catalog projection
redacts explicit peer fields and arbitrary schema JSON before composing native
DTOs. ErrorData messages/data, protocol evidence, task IDs and MRTR diagnostics
are sanitized before escaping, truncation or native framing. Native error codes,
server identity, generations, counts and fixed protocol fields remain accurate.
Opaque transport errors retain category, transport type and operation phase
without replaying potentially escaped peer text. Stderr capture must already be
complete and unclipped before it can enter a diagnostic; optional capture never
delays physical settlement. The original 64 KiB catalog and 1 KiB diagnostic
limits remain enforced.

Real HTTP catalog tests obtain the actual Authorization header at the peer and
return both the complete value and independently extracted token in all three
catalog kinds and nested schema keys/values. Literal/reference configurations
also retain ordinary `name`, `tools`, `e` and `1` headers. Real HTTP transport
failure echoes quoted/backslash credentials through a response Content-Type;
real stdio failures cover escaped JSON and a credential clipped at the capture
limit. The original recovery fixture's literal `1` and both strict generation
assertions remain unchanged. An attempted uncommitted fixture-only workaround
was withdrawn before this product repair and supplies no validation evidence.

[FULL #1778](https://github.com/Caismis/rustX/actions/runs/42428879869) on the
reviewed head failed those two recovery assertions. It also exposed an existing
Host lifetime test that waited only for projection-worker exit while the
independent admission worker could still own a temporary runtime upgrade. The
test now gates that exact interleaving and waits for both existing native exit
receipts before asserting weak runtime release; its original assertions and
30-second bound remain. A browser chrome test sampled a sidebar's running
opening transform and observed a fractional width. It now waits for the existing
animation to finish before the unchanged exact geometry assertions. Product
WebUI/TUI code, screenshot references, retries and timeouts are unchanged.

The earlier local results and measurements above apply only to their identified
historical sources. This follow-up requires its own local checks, independent
review, fresh source-identified captures and final-head FULL CI. The exact final
SHA, run and verdict are tracked in the PR; this evidence does not claim that
#1778 or an earlier source is green.

The same #1778 macOS full lane still failed the unchanged raw-PTY duplex test:
actor-turn `yield_now` does not guarantee consumer scheduling. Native short PTY
reads could therefore fill the 32-event queue before an active client resumed.
The command actor now retains at most one 8 KiB pending batch per output stream.
First bytes and acknowledged short tails flush promptly; unacknowledged small
reads become full batches. EOF and reaching the retention cap force tails out,
including the original empty truncation marker when required. Receipt polling
shares the actor select with stdin, controls, output and physical settlement;
no output receipt blocks input. The original 32-event `try_send` overload and
final transport-acceptance fence remain. Four deterministic real-Events tests
cover 131,072 one-byte reads with no consumer progress, exact reconstruction,
32-slot overload, exact-cap tails and cap-triggered overload without EOF or an
acknowledgement. The original macOS duplex, pipe/PTY post-cap termination and
slow-consumer assertions remain unchanged.

A subsequent full local run exposed an existing archive-test completion race:
its deliberate Unavailable observation wakes a status-recovery reader, which
can contend with the archive's conservative native `try_lock` claim after the
main cross-Thread assertions have passed. Test-only gates now park that reader
and the empty admission pass outside Coordinator, then observe the existing
shutdown boundary to prove the first archive claim succeeded before releasing
them. Production claim semantics and the original success, cross-Thread,
metadata, zero-model and timeout assertions remain unchanged. Independent review
found no remaining P1/P2 or weakened assertions in these synchronization repairs.

The external protocol-negotiation target also exposed fixture inputs that used
the peer's expected revision as a literal credential value. Peer-advertised and
legacy-negotiated revision strings remain sanitized; the locally offered SDK
revision set remains native. Negotiation fixtures now receive explicit peer
revision parameters independently of credential inputs, with descriptive
presence-only mode flags. The obsolete legacy revision environment override is
removed. Both original exact-version and compatibility-classification assertions
remain. A real stdio regression separately covers modern/legacy peers returning
quoted/backslash credentials as their revision, through literal/reference
configuration, and long nonsecret revisions. It checks native client revisions,
secret masking, the final 1 KiB diagnostic budget and physical peer reaping.
Final compatibility errors and appended complete stderr share that same budget;
no final blanket redaction is reintroduced. Recovery's original literal `1` and
its strict generation assertions remain unchanged.

The final local all-target/all-feature Rust run completed 22 targets with
6,061 passed, zero failed and 20 existing ignored tests (library: 5,350 passed,
ten ignored). Strict all-target/all-feature Clippy, formatting, whitespace,
the all-feature executable build and the authenticated standalone Node example
also passed. The four original browser chrome geometry cases passed with the
existing exact assertions after animation completion. Independent read-only
review found no remaining P1/P2 or assertion weakening. Native macOS and the
complete browser matrix still require a new exact-head FULL CI run; these local
results do not change the failed verdict of historical run #1778.

### Final typed-boundary source and measurements

The sixth sequential capture pair follows product commit
`cd9a792f4f44608c8a763b37710d888df9df01d7`, with no concurrent test or compile
work from this task. The unchanged baseline remains
`57447d2e825f914e80bbfecc68d373fcab487446`. All executables use the debug
all-feature build. The raw files retain every sample and the harness's
`workingTree: true` marker; the relevant working files were independently
compared with their identified committed Git objects.

| Capture | Measured-source SHA-256 | Binary SHA-256 |
| --- | --- | --- |
| [Baseline Turn](baseline-typed-redaction-measurements.json) | `9698a4826d8c1f7f65e9d729b41aaa7ff5ade3b8c5179646c2fe416caa53fc22` | `c2532fa4c9560388e4b67190f8da2555d40dcf0dd15a257746ce3c7ed8f0ac19` |
| [Candidate Turn](candidate-typed-redaction-measurements.json) | `68b0e0a60c0c5ab12f88d450de239afab9ffa963b03f0e50f9f2d809918b6b83` | `f3a4a50c844e9ff7b73704b55829025657d2eb62fcf4c2ec2552edb0eace5c39` |
| [Candidate environment](environment-typed-redaction-measurements.json) | `2d7fb8b70c2eb073fbfcdd118ab67317dfb3809d7296f1022f92f3f2b873fc21` | `f3a4a50c844e9ff7b73704b55829025657d2eb62fcf4c2ec2552edb0eace5c39` |

Each Turn mode completed twenty actual settled native Turns, with twenty
provider requests without Host Tools and forty with Host Tools. Values below
are baseline / candidate; p50/p95 use the original twenty raw samples.

| Mode | CPU ms | RSS after MiB | Write calls | Kernel written MiB | Latency p50 ms | Latency p95 ms |
| --- | --- | --- | --- | --- | --- | --- |
| no-host-tools | 7180 / 4870 | 324.4 / 321.1 | 30855 / 30697 | 62.03 / 61.97 | 1438.4 / 150.6 | 4908.2 / 165.7 |
| host-one-client | 18260 / 17360 | 341.9 / 340.8 | 70647 / 70378 | 142.99 / 142.59 | 725.8 / 552.4 | 999.0 / 590.7 |
| host-three-clients | 20240 / 23290 | 341.2 / 348.3 | 70925 / 70767 | 143.21 / 143.01 | 672.0 / 557.0 | 1607.8 / 605.3 |
| host-three-clients-one-paused | 22610 / 21720 | 345.6 / 348.1 | 70693 / 70802 | 143.05 / 143.10 | 655.4 / 559.1 | 717.5 / 796.4 |
| host-three-resident-sessions | 23630 / 23600 | 359.5 / 358.8 | 60169 / 60120 | 123.12 / 123.19 | 601.1 / 662.0 | 708.6 / 978.9 |

The three-client candidate used more CPU and RSS, while its latency decreased.
Paused-client p95 and resident-session p50/p95 increased. Baseline no-Host raw
latency includes an 8834.2 ms maximum and 4908.2 ms p95; both remain in the
capture. These fixed-order shared-host debug observations do not establish
causation, statistical significance, zero overhead or production throughput.

All four environment modes completed twenty cycles, with zero Host Tools/model
requests. All eighty descriptor samples retained their original 19/21 count.
Four admitted commands and eight watches reached 75/77 descriptors and thirteen
processes; cleanup returned to 19/21 descriptors and one server process.

| Environment mode | Parent / reaped-child CPU ms | RSS after MiB | Cycle p50 / p95 ms | Write calls | Kernel written MiB | Descriptors before / peak / cleanup |
| --- | --- | --- | --- | --- | --- | --- |
| environment-one-client | 490 / 90 | 153.3 | 43.6 / 45.3 | 548 | 0.08 | 19 / 75 / 19 |
| environment-three-clients | 440 / 90 | 156.7 | 43.9 / 46.3 | 595 | 0.08 | 21 / 77 / 21 |
| environment-three-sessions | 520 / 80 | 161.5 | 42.3 / 45.8 | 522 | 0.08 | 21 / 77 / 21 |
| environment-one-idle-paused-client | 480 / 90 | 157.0 | 43.3 / 46.2 | 604 | 0.08 | 21 / 77 / 21 |

Setup/peak work is excluded from cycle CPU/write deltas. Kernel counters do not
measure physical-device I/O; summed process-tree RSS repeats shared pages. The
paused idle peer does not prove saturation behavior, which remains covered by
native bounded-output and physical-settlement tests. Observed resource return
is a cardinality fact, not a general workload/capacity guarantee. These captures
supplement the local checks above; final-head FULL CI is recorded separately in
the PR so historical evidence is not relabeled as a later CI verdict.

### Proxy authentication credential follow-up

External review of `93dae00f6` accepted the Authorization, typed catalog,
generation attribution, diagnostic and command-output repairs, then identified
one further accepted standard header: Proxy-Authorization retained its complete
value but omitted the Bearer token component. The same native collector now
handles both standard authentication fields, case-insensitively, for literal
and resolved reference inputs. Full values and tokens both enter the existing
private redaction set.

The real HTTP peer obtains either header from the actual request and independently
extracts its Bearer token. The existing matrix now performs 72 catalog calls
(66 HTTP, six stdio), including 30 Proxy-Authorization calls, literal/reference
inputs, mixed-case field/scheme spelling, full-value/token echo in all three
catalog kinds and nested schema keys/values, and ordinary `name`, `tools`, `e`
and `1` transport values. Native structure, zero loaded sessions and final host
drain assertions remain. Independent read-only review found no remaining P1/P2
or assertion weakening. Earlier captures and #1779 identify their own historical
sources; this repair requires new source-identified evidence and exact-head FULL
CI before completion.

Local follow-up verification passed all 153 MCP unit tests and all 21 real
environment boundaries, including the expanded 72-call matrix, with zero
failures or ignored tests. Strict all-target/all-feature Clippy, formatting,
whitespace, all-feature binary build and authenticated standalone Node example
checks passed. The earlier 6,061-test complete local run identifies
the preceding typed-boundary source; the new final-head FULL run must validate
the complete target/platform matrix after this small collector extension.
