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
| Real command/PTY/filesystem/Skills/MCP protocol boundaries | 17 passed, including descriptor-bound watch ABA, canonical PTY EOF, PTY descriptor isolation, Host drain, native cleanup failure and instance-capacity cases; macOS adds its native child/capacity boundary in FULL CI |
| Native file fault/race/CAS tests | 6 passed in the full run |
| Rust formatting and all-target/all-feature Clippy (`-D warnings`) | Passed |
| Complete local Rust all-target/all-feature regression | 6036 passed / 20 existing ignored; subsequent shutdown/MCP refinements passed the 14 boundary and 7 native probe tests; watch/PTY follow-up boundaries are included in the 17-case run; final-head FULL CI is required below |
| CI lane coverage | Passed; Linux FULL and FAST selectors cover their required tests |
| Generated protocol/schema/fixtures and existing client typechecks | Generated-artifact drift check and all four package typechecks passed |
| TUI, dev and Web deterministic suites | dev: 43 passed; TUI: 1349 passed / 1 native skip; Web: 4204 passed / 4 existing skips |
| Browser acceptance | Full local pinned run: 660 passed / 7 expectation timeouts on the shared host. Focused diagnostics reproduced intermittent timing failures; native 34-Turn history preparation passed on untouched base and candidate, and the original browser assertions passed with temporary logging (removed). No assertion, timeout or screenshot baseline was relaxed. Final-head FULL browser acceptance is required in the PR. |
| Minimal external-client smoke | Real authenticated v75 initialize/server-info passed from outside the repository |
| Independent code review | Initial and shutdown findings repaired; external watch/PTY findings and follow-up source-identity/child-directory cases are repaired and independently re-reviewed; the later PTY descriptor leak is also repaired and independently re-reviewed before the final FULL run |
| Final-head FULL CI, including macOS and both browser shards | Required on the final PR head; exact SHA, run and verdict are recorded in the PR description |
| Performance/resource measurements | Two initial baseline/candidate pairs plus a watch/EOF-source pair and repeated native environment modes completed; raw samples and observed regressions below |

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
