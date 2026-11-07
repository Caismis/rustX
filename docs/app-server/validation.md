# Issue #532 implementation evidence

Implementation base: `4ecd28b8834d91566404ccc24a601ba7cf2e9b7d`.
Pinned Codex release: `rust-v0.166.0`, commit
`6ff1e1ca3b30fc8b914720681fcf5fe62e95f856`.
The [method and native-owner matrix](shared-control.md) describes the public
contract, identity mapping, admission and settlement boundaries, deployment
responsibilities, and intentional exclusions.

## Invariants and executable evidence

| Invariant or boundary | Executable evidence |
| --- | --- |
| Capability file/hash and controlled-clock HS256 verification fail closed | `src/app_server/transport/auth.rs` tests; `tests/scripted/app_server/authentication.rs`; private credential and listener argument tests |
| Browser and native credentials use the same validator and ordinary API eligibility; JWT expiry affects new upgrades | `native_and_browser_share_validation_and_jwt_expiry_only_affects_new_admission`; standalone example uses a credential containing `/`, `+` and `=` |
| Three writers share one native Attempt; start-or-steer differs from queued input; stale controls cannot hit a successor | `shared_start_steer_sparse_queue_stale_controls_and_disconnect_use_one_native_writer` in `tests/conformance/app_server_host.rs` |
| One native mailbox retains sparse queue/steer identities and acceptance correlation | Native durable adoption tests; the shared-control provider conformance; Web and TUI lost-receipt tests |
| First correlatable result or RPC error claims once, including invalid domain results | `three_clients_broadcast_first_response_error_and_invalid_payload_commit_once`; `src/app_server/callbacks.rs` arbitration tests |
| Broadcast may execute two external effects but commits one canonical result and continuation | Real HTTP provider conformance records two application effects before either response and verifies one native result |
| A canceled or retired actor cannot publish a queued callback or claim a reply | Callback carrier-acceptance and cancellation tests; `retired_native_incarnation_is_fenced_before_closed_delivery`; actual catalog archive/unarchive regression |
| Every live request at the 128-request bound can replay before a reader runs | `reconnect_replays_the_entire_pending_bound_before_the_reader_runs`; bounded client queue and slow-consumer tests |
| Exact Thread history and catalog identity survive rename, archive, unarchive and restart | `thread_identity_tools_names_archive_and_unarchive_remain_native_metadata`; existing node-switch, deletion and residency suites |
| Host declarations validate namespaces, schemas, reserved names and frozen capabilities | `src/tools/host.rs` tests, root capability resolution, manifest serialization and real provider conformance |
| Process death never automatically repeats an uncertain external effect | `host_effect_is_unknown_after_process_death_and_is_never_automatically_replayed` in `tests/process/app_server_host.rs`: actual App Server, fsynced effect, SIGKILL, same-root reopen |
| Existing clients preserve prompts, drafts, exact targets, queued-message operations and lost-receipt repair | Web deterministic and browser suites; TUI state/controller and actual stdio/WebSocket integration suites |
| An independent external client needs no rustX SDK | `tui/test/external-example.test.ts` launches the real binary and runs `examples/app-server/connect.mjs` |

Composed conformance uses `test-support/fake-provider` through the actual catalog,
adapter, HTTP client and parser. Gates and native validity seams establish order;
timeouts bound failure. No paid model credentials are required.

## Local checks

Environment: Linux x86_64, Rust 1.98.1, Node 24.19.0, locked pnpm 11.13.1 and the
repository's Python/uv provider emulator. Browser references use the repository's
immutable Playwright 1.63.0 container. Results below refer to the integrated base
above and the completed implementation, including the independent review fixes.
The full-suite snapshot precedes the final interaction-reply numeric-codec
correction. Its follow-up App Server unit suite passed all 58 tests; final-head
FULL CI below owns the complete result after that correction.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| `cargo build --bins --all-features` | Pass |
| Provider emulator `uv run --frozen pytest` | 55 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --no-fail-fast` | Pass; Cargo reports 5999 passed, 20 ignored; includes actual process death and required-emulator conformance |
| `cargo run --all-features --example check_test_lanes` | Pass; Linux FULL discovery has no missing, empty or stale lane selections |
| `cargo test --all-features app_server:: --lib` after the reply-codec correction | 58 passed; exact Workflow u64 identities round-trip, noncanonical/overflow values fail |
| Protocol generation drift, editor schema generation and TypeScript checks | Pass; generated editor schemas have no drift |
| Development launcher typecheck and tests | Pass; 43 tests |
| TUI typecheck and required-emulator full suite | Pass; 1326 passed, 1 skipped |
| Web typecheck and full deterministic suite | Pass; 4181 passed, 4 skipped |
| Web production build, i18n, provenance and notices | Pass; 347 source records, 349 production package notices |
| Pinned-container browser acceptance | Full local traversal reported 633 passed and 34 failed. Focused repairs passed delivery 2/2, presentation 3/3 and all 60 Shell/navigation cases across two runs. The remaining review-codec repair's browser recheck awaited container startup at this snapshot. Its result and the complete final-head browser-shard/coverage verdict are recorded on [the primary PR](https://github.com/Caismis/rustX/pull/540) |
| Five-mode real-process performance measurement | Pass; 100 native Turns, exact 20/40 model requests per mode; [raw data and method](../../benchmarks/app-server/README.md) |
| `git diff --check` | Pass |

TUI's one skip is the native macOS ACL case; Web's four skips are native macOS
Office/Seatbelt cases. Required emulator cases ran. Native ignored live-provider
tests, measurements and fixture generators are not correctness coverage.
Fixture child entry points in Cargo's totals are not separate boundary proofs;
their parent tests own those claims.
Earlier partial or failed runs are not counted as
successful full-suite evidence. Final-head Linux, macOS and full browser CI must
all pass under `full-verification`; the [primary PR](https://github.com/Caismis/rustX/pull/540)
records the exact final head, run URL and full-suite verdict. This committed local
snapshot makes no claim about a later CI result. Local Linux checks do not
substitute for macOS execution.

## Independent review

A separate read-only review agent inspected the complete implementation and
re-reviewed it after integration with the latest main. It found seven P2 issues,
all fixed and re-reviewed: private credential bounds/truncation; encoded browser
bearer carriers; full-bound callback replay; resolved TUI pagination entries;
physical close after a wrong Thread receipt; retired native-incarnation fencing;
and ordinary display/edit/remove treatment for `queued_message`.

Follow-up source review also checked the browser wire probe's native Thread-to-
Session mapping and physical receipt barriers, the corrected ordinary-delivery
assertions, and the canonical integration documentation. The browser's real
history/reconnect test and the 21-test probe suite pass after the probe repair.
It also verified finite fake-server capabilities, current mutation guards and
the exact Attempt ID in interrupt assertions; 242 affected unit tests pass.
The final browser traversal also found a claimed review result bypassing the
existing exact-u64 wire decoder. App Server now reuses that decoder after claim;
an independent review verified that invalid results still settle once and that
free-form Host JSON is unchanged. Workflow completion assertions now check the
run header rather than accepting a completed child node.

The final independent source review found no remaining blocking issues. It also
checked native lifecycle/cancellation/broker lock order and the main branch's
submission-activity sampling. No WebUI/TUI controls, layouts, styles or screenshot
references were added, removed or redesigned. Product changes adapt protocol,
identity and state synchronization behind the existing UI.
Static presentation fixtures declare only their implemented API subset. Their
unchanged references do not assert unchanged feature visibility against Native:
common ordinary API eligibility can make the existing side-question control
available on connections that previously lacked the extra delivery credential.

## Deliberate limits

This remains one user's trusted App Server, with admission-time JWT validation
and operator-owned credential distribution, TLS and browser Origin/CSP policy.
Broadcast does not guarantee one external effect. A remote error, disconnect,
cancellation after dispatch or process crash cannot prove that external work
stopped. The native Tool Runtime owns canonical result and terminal commitment.

The existing browser Delivery Access input remains visible to respect the UI
constraint. Ordinary API eligibility now comes from the connection credential;
the former extra delivery credential has no effect and is never sent. The
private Product Host root-proof channel retains its separate native ownership.

Command execution, new filesystem/management APIs and the other Issue B work
remain outside this change. The standalone connectivity example and its run
instructions are in [shared-control.md](shared-control.md#existing-clients-and-standalone-example).
