# Issue #430 repair validation

The normative contract is [conversation-reading.md](conversation-reading.md).
App Server v30, Runtime Client v55 and subagent IPC v29 retain separate version
ownership. SQLite schema 49 retains turn-reading bootstrap provenance and a semantic
read-mutation epoch, and rejects inherited live execution outcomes; old stores are refused without migration.

## Final contracts and deterministic evidence

One native Attempt is one user-facing turn. Model retries, logical steps and Tool
calls stay inside that Attempt. Actual Goal continuation admits another Attempt.
Failed/interrupted/cancelled/timed-out/limited Attempts keep their origins.

A read cut contains Conversation identity, inclusive Journal/transcript upper
bounds and a native mutation revision. Later appends and Surface compaction leave
old cuts usable. Entries, response/process facts and Tool results are bounded to
that cut. Foreign/future/mutated cuts, absent-at-cut targets and locations created
after the cut fail explicitly. Attachment/runtime replacement has independent
control and browser-response fences. Older/newer paging stays in the same frozen
prefix; Return to latest selects fresh bounds. No outline refresh is tied to each
streamed event.

Separate `TurnReadingProvenance` belongs to native lineage/bootstrap, rather than
finalized-response/Retry provenance. Clone, fork and tree branch share the same
retention/remapping seam. It keeps native origin/order, retained member IDs and
terminal-only predecessor/outcome/clocks, without copying Journal events or
execution state. Genuinely unowned content creates no turn. Destination-local
Attempts follow inherited origins.

`ConversationStore::read_lineage_cut` is the sole native lineage-read authority.
The first SELECT in one SQLite read transaction freezes C. Selected Surface
history, its Ledger identity closure, completed responses and turn-reading facts
all come from that transaction. Surface revision selects structure; Journal C
selects time. A terminal-only predecessor controls placement only after the
terminal has passed the Journal bound. Publication after C cannot enter a parked
copy. Live retained output is `InheritedTurnOutcome::IncompleteAtCut`; that enum
cannot represent Running. Projection has no live timer, invented end timestamp,
request, Journal event or destination settlement ownership.

The rail merges one bounded outline page with the live native Attempt. Current
identity, historical locate capability and loaded anchor are distinct. First
visible native process location causes one outline refresh before settlement;
subsequent same-Attempt progress causes none. Explicit latest/page(offset) intent
survives automatic refresh; newest-page selection restores latest, including
64→65 and 128→129. One demand during a pending reply is retained, and resync
retires old authority demand. ChatViewport alone owns automatic
Chat scroll writes and publishes the current identity while following. Historical
navigation detaches; Return to latest restores the latest window and follow mode.
The rail occupies ChatViewport's existing overlay surface; the redundant outer
flex surface was removed after it caused inconsistent rounded composer focus
corners. Three fresh pinned captures of the affected dark 390px state then matched
the unchanged reference. No second navigation/scroll coordinator was added.
Width implementation was audited and retained unchanged in this repair.

| Contract | Discovered native regression |
| --- | --- |
| >512-position direct jump, exact destination, 64-entry window, 240-character preview, adjacency and no work/writes | `durable::sqlite::reading_tests::distant_turn_windows_are_exact_bounded_and_read_only` |
| Native terminal/running identity, exact order and locations | `durable::sqlite::reading_tests::paging_terminal_empty_and_running_attempts_keep_native_identity_order` |
| Copied native origin maps to destination; unowned content stays unowned | `durable::sqlite::reading_tests::copied_turns_preserve_origin_and_use_destination_locations` |
| Original outline/window survives later appends; foreign/future cuts fail | `durable::sqlite::reading_tests::append_stable_cuts_reject_future_and_foreign_targets_without_fallback` |
| New targets and first locations cannot leak backward into an old cut | `durable::sqlite::reading_tests::frozen_cut_rejects_targets_and_locations_created_later` |
| Successful pending edits/removals retire unreconstructible cuts; failed CAS does not | `durable::sqlite::reading_tests::pending_edits_and_removals_retire_cuts_without_moving_turns` |
| Two old turns plus live Attempt; later Tool result/Assistant/native fact; full decorated old window equality and frozen newer paging; no execution | `runtime_client::response::tests::historical_window_cut_survives_live_appends_and_bounds_tool_and_response_decorations` |
| Compaction preserves response origins and the original frozen window | `runtime_client::response::tests::real_compaction_preserves_response_identity_cut_and_cumulative_usage` |
| Interrupted content keeps native cancelled origin/outcome and exact destination | `runtime_client::response::tests::lineage::interrupted_process_content_retains_native_origin_outcome_and_destination_location` |
| Success/cancelled/failed/timed-out/limited exact IDs, ordinals and cursors [2,3,4,5,6], three generations/reopen, bootstrap snapshot, no copied Journal and local ordinal 6 | `runtime_client::response::tests::lineage::lineage_preserves_success_failed_and_terminal_only_turns_in_native_order` |
| All three real Session lineage operations retain interrupted and terminal-only origins at cursors 2/3; first-input fork is empty | `local_runtime::session::tests::clone_fork_and_branch_share_native_unsuccessful_turn_provenance` |
| Real Goal commit gate, ordinary settlement/admission handoff and parked provider: three requests, distinct Human/Goal Attempts, Completed/Cancelled terminals, copied origin order and cursors [2,6] | `scripted_suites::extensions::goal351_model_create_goal_starts_no_nested_attempt_and_continues_after_settlement` |

The focused Web command discovers 65 tests in five files: reading navigation,
rail, scroll, process presentation and width. Controlled replies prove both navigation reply orders,
seven authority/user replacements, exact installed cut/window and destination,
512-entry bounds, two in-flight reads, ordinary detached reading and Return to
latest. New exact regressions include:

- `an existing outline survives same-Attempt durable streaming progress and installs only its original cut`;
- `a loaded historical rail mark remains usable after same-Attempt append progress without refreshing its cut`;
- `current native identity precedes location; the first durable process location enables navigation before settlement`;
- `follow publishes the live native turn without a locate cursor; historical anchors and Return to latest own active reading`.

New follow-up regressions:

- `local_runtime::session::tests::running_lineage_cut_retains_incomplete_origin_without_execution`;
- `local_runtime::session::tests::terminal_only_publication_loses_frozen_lineage_copy_race`;
- `local_runtime::session::tests::terminal_only_publication_wins_frozen_lineage_copy_race`.

All three use channels to park copy after the native transaction returns C. They
assert exact origin, ordinal 1, destination cursor 2, incomplete/cancelled outcome,
clone/fork/tree agreement, zero destination events/requests, unchanged source cut,
reopen equality and repeated-copy mapping. The terminal loser has no visible
member and is excluded despite its retained predecessor. The running member
is copied while the source remains running, and stays incomplete after later
source settlement.

Web regressions:

- `latest outline follows 64 → 65 through start, location and settlement`;
- `latest outline follows 128 → 129 through start, location and settlement`;
- `explicit historical page survives live growth; reaching newest page restores native latest intent`;
- `settlement during a gated outline reply preserves one latest refresh demand`;
- `resync retires a queued outline refresh; the old reply cannot retire new authority demand`;
- `incomplete inherited process is historical, has no running timer and is not reported failed`.

ResizeObserver/animation-frame controls prove user scrolling wins before a reply
and before the navigation frame, and detached positions survive width/sidebar/
right-panel, Tool disclosure and image growth. All 16 existing width tests retain
pointer capture, frame coalescing, two-pixel deliberate commit, press-only/cancel/
lost-capture behavior, storage failures, measured temporary clamps, keyboard
commits and reduced motion. No Chat `scrollIntoView` or second scroll coordinator
was introduced.

## Previously approved browser reference review

Pinned rendering authority:
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`.
Podman ran that same immutable image; host browser/font caches were not mounted.
The Browser plugin was unavailable, so the repository Playwright workflow was
used. Initial full acceptance passed 121 tests and found 25 screenshot mismatches.
Candidate generation exercised the later states those mismatches prevented from
being reached. Original references were restored while candidates were reviewed;
only inspected pinned stable captures were then retained.

Each affected reference was reviewed as reference/actual/diff. Desktop transcript,
composer and context cards now share the measured width (for example, 742.4px
transcript on a 1160px column instead of the old 680px cap). Rail controls/current
marks are native reading chrome. Narrow widths clamp rather than overflow. Sidebar,
header, Inspector and Settings control geometry remain unchanged; modal backdrop
and rounded-edge pixels reflect the changed conversation underneath. Existing
four New Conversation button noise-region pixel crops are byte-identical before
and after. No noise regions, thresholds, pixel budgets or deadlines were changed.
The exact reference list and review conclusions follow below.

All 70 retained reference updates were individually inspected. The previous repair
normal acceptance run passed all 146 tests in 9.2 minutes with strict pinned comparison,
including the unchanged dark 390px composer focus-corner reference.

| Updated reference | Review conclusion |
| --- | --- |
| [agent-approval-light-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/agent-approval-light-linux.png) | Shared transcript width and native rail/current marks; Agent/Tool state remains unchanged. |
| [agent-dark-desktop-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/agent-dark-desktop-linux.png) | Shared transcript width and native rail/current marks; Agent/Tool state remains unchanged. |
| [agent-dark-narrow-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/agent-dark-narrow-linux.png) | Shared transcript width and native rail/current marks; Agent/Tool state remains unchanged. |
| [agent-error-light-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/agent-error-light-linux.png) | Shared transcript width and native rail/current marks; Agent/Tool state remains unchanged. |
| [agent-questionnaire-light-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/agent-questionnaire-light-linux.png) | Shared transcript width and native rail/current marks; Agent/Tool state remains unchanged. |
| [agent-selectors-light-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/agent-selectors-light-linux.png) | Shared transcript width and native rail/current marks; Agent/Tool state remains unchanged. |
| [agent-settled-light-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/agent-settled-light-linux.png) | Shared transcript width and native rail/current marks; Agent/Tool state remains unchanged. |
| [agent-streaming-light-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/agent-streaming-light-linux.png) | Shared transcript width and native rail/current marks; Agent/Tool state remains unchanged. |
| [agent-tools-light-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/agent-tools-light-linux.png) | Shared transcript width and native rail/current marks; Agent/Tool state remains unchanged. |
| [composer-attachment-dark-1440-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-attachment-dark-1440-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-attachment-light-1440-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-attachment-light-1440-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-context-dark-1440-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-context-dark-1440-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-context-dark-390-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-context-dark-390-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-context-light-1440-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-context-light-1440-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-context-light-390-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-context-light-390-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-idle-draft-dark-1440-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-idle-draft-dark-1440-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-idle-draft-dark-390-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-idle-draft-dark-390-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-idle-draft-light-1440-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-idle-draft-light-1440-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-idle-draft-light-390-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-idle-draft-light-390-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-idle-empty-dark-1440-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-idle-empty-dark-1440-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-idle-empty-dark-390-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-idle-empty-dark-390-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-idle-empty-light-1440-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-idle-empty-light-1440-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-idle-empty-light-390-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-idle-empty-light-390-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-running-draft-dark-1440-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-running-draft-dark-1440-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-running-draft-light-1440-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-running-draft-light-1440-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-running-empty-dark-1440-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-running-empty-dark-1440-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-running-empty-dark-390-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-running-empty-dark-390-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-running-empty-light-1440-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-running-empty-light-1440-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [composer-running-empty-light-390-linux.png](../web-console/test/e2e/agent.spec.ts-snapshots/composer-running-empty-light-390-linux.png) | Shared composer/context width axis; narrow clamp and empty-rail cleanup; controls remain usable. |
| [locale-general-zh-linux.png](../web-console/test/e2e/locale.spec.ts-snapshots/locale-general-zh-linux.png) | Chinese copy/Settings preserved; conversation backdrop follows the shared width axis. |
| [settings-advanced-dark-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-advanced-dark-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-agent-light-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-agent-light-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-agent-narrow-dark-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-agent-narrow-dark-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-conflict-light-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-conflict-light-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-delete-confirm-light-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-delete-confirm-light-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-extensions-light-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-extensions-light-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-general-light-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-general-light-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-loading-light-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-loading-light-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-mobile-dark-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-mobile-dark-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-mobile-menu-dark-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-mobile-menu-dark-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-models-light-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-models-light-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-provider-detail-light-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-provider-detail-light-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-provider-detail-mobile-dark-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-provider-detail-mobile-dark-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-read-error-light-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-read-error-light-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-tools-light-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-tools-light-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-workspace-inherited-light-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-workspace-inherited-light-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [settings-workspace-override-light-linux.png](../web-console/test/e2e/settings-presentation.spec.ts-snapshots/settings-workspace-override-light-linux.png) | Conversation backdrop/rounded-edge pixels only; Settings control bounds unchanged. |
| [desktop-collapsed-rail-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/desktop-collapsed-rail-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [desktop-expanded-dark-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/desktop-expanded-dark-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [desktop-expanded-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/desktop-expanded-light-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [desktop-right-panel-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/desktop-right-panel-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [mobile-expanded-dark-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/mobile-expanded-dark-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [mobile-rail-dark-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/mobile-rail-dark-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [mobile-settings-dark-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/mobile-settings-dark-linux.png) | Settings shell preserved; visible conversation underneath uses the intended width axis. |
| [session-idle-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/session-idle-light-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [session-queued-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/session-queued-light-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [session-reconnect-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/session-reconnect-light-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [session-stopping-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/session-stopping-light-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [session-uncertain-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/session-uncertain-light-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [session-uncertain-mobile-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/session-uncertain-mobile-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [settings-shell-dark-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/settings-shell-dark-linux.png) | Settings shell preserved; visible conversation underneath uses the intended width axis. |
| [settings-shell-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/settings-shell-light-linux.png) | Settings shell preserved; visible conversation underneath uses the intended width axis. |
| [sidebar-background-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/sidebar-background-light-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [sidebar-delete-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/sidebar-delete-light-linux.png) | Conversation empty-state width/chrome beneath unchanged sidebar controls. |
| [sidebar-empty-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/sidebar-empty-light-linux.png) | Conversation empty-state width/chrome beneath unchanged sidebar controls. |
| [sidebar-named-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/sidebar-named-light-linux.png) | Conversation empty-state width/chrome beneath unchanged sidebar controls. |
| [sidebar-other-uncertain-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/sidebar-other-uncertain-light-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [sidebar-preview-light-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/sidebar-preview-light-linux.png) | Conversation empty-state width/chrome beneath unchanged sidebar controls. |
| [sidebar-scoped-inspector-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/sidebar-scoped-inspector-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |
| [workspace-session-browser-linux.png](../web-console/test/e2e/shell.spec.ts-snapshots/workspace-session-browser-linux.png) | Conversation width/rail/current state; sidebar, header and Inspector bounds unchanged. |


## Current CI and pinned compiler

The starting head `12eedc911c978035d3295085c671ec1584627331` completed
[36947507874](https://github.com/Caismis/rustX/actions/runs/36947507874): quality
failed; Linux contracts/boundaries, macOS boundaries, protocol, TUI and full Web
passed. The macOS run includes the previously failing unchanged Bash test. This
is evidence of that run passing, not proof that compiler pinning fixed Bash.
Native macOS is unavailable locally; the final pinned commit has not run there yet.

Quality resolved floating stable to rustc 1.99.0 / Clippy 0.1.99. Exact main had
passed under 1.98.1 and had reproduced the same new diagnostic classes under 1.99.
The repository now selects 1.98.1 in rust-toolchain.toml and explicitly in all seven
Cargo-bearing CI jobs, including TUI. Local 1.98.1 Clippy passes with -D warnings.
No global allow, skipped lane, Rust-1.99 assertion cleanup or warning relaxation
was added. Contributor policy makes upgrades intentional repository changes.
Existing rust-cache keys already include resolved rustc, toolchain file and Cargo
inputs; shared keys and save policy are unchanged.

Resolved versions: rustc 1.98.1 (48a229cea, 2026-09-01), Cargo 1.98.1
(797e8a9bc, 2026-08-05), Clippy 0.1.98 (48a229ceae, 2026-09-01), rustfmt
1.9.0-stable (48a229ceae, 2026-09-01).

## Follow-up acceptance observations

No references, tolerances, noise regions or screenshot policy changed in this
follow-up. Strict normal acceptance found 15 light / 16 dark changed pixels at the
390px running-draft composer's upper-left rounded focus corner, with unchanged
334×174 dimensions. Both actual PNGs reproduced byte-for-byte on an exported
copy of the reviewed head under the same pinned image and preceding Agent states.
The isolated two composer tests on that reviewed head passed, showing the existing
state-sequence rendering variation. This is not an intentional product change to
approve as a new baseline. The additional archive-only accessibility failures
were missing native prerequisites in that comparison export and are not claimed
as product results. The primary checkout was untouched.

Full TUI initially passed 894 of 895 tests; the WebSocket child-transcript case
never reached inspection-request-1 (one request parked at inspection-request-0).
The unchanged test passed when isolated against the same binary (1 discovered
test), and the subsequent sequential full TUI run passed all 895. No source
cut/copy operation is used in that path. A concurrent full Web run also timed out
the unchanged four-cycle remount test at its existing 5-second budget; sequential
full Web then passed all 1,323. The earlier runs overlapped native compilation
and tests. Resource contention is a plausible explanation, not a proved provider
contract defect; no gate, timeout, assertion or runtime behavior was changed.

## Executed follow-up validation

Commands use the independent Issue #430 worktree, Rust 1.98.1, Node 24.21.0 and
pnpm 11.13.1. CARGO_PROFILE_DEV_DEBUG=0 / CARGO_PROFILE_TEST_DEBUG=0 reduce debug
metadata only. Native/provider/TUI tests require the emulator and use isolated
TMPDIR=/home/caismis/.cache/rustx-443-tests-tmp to avoid pre-existing ancestor
workspace files under /tmp. Ambient files and the primary checkout stay untouched.

The strengthened live-copy gate was checked again after the full native run.
Every focused selector discovered at least one test; none was a zero-test pass.

| Command | Result (exit) |
| --- | --- |
| `rustc --version` / `cargo --version` / `cargo clippy --version` / `rustfmt --version` | Versions above; all 0 |
| `cargo fmt --all -- --check` | Pass (0) |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Pass (0), 1.98.1 |
| `cargo build --bins --all-features --locked` | Pass (0) |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked` | Pass (0): 4,070 passed, 8 ignored |
| `pnpm --dir protocol/app-server generate` | Pass (0), v30 schema/types/fixtures |
| `pnpm --dir protocol/app-server check` | Pass (0), no artifact drift |
| `pnpm --dir protocol/app-server typecheck` | Pass (0) |
| `pnpm --dir web-console typecheck` | Pass (0) |
| `pnpm --dir web-console test` | Final pass (0): 71 files, 1,323 tests; earlier concurrent run had 1 existing timeout |
| `pnpm --dir web-console check:i18n` | Pass (0) |
| `pnpm --dir web-console check:provenance` | Pass (0): 147 source records, 131 package notices |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | Fail (1): 144 passed, 2 strict existing focus-corner mismatches, 10.7 minutes; byte-identical reviewed-head reproduction above |
| `pnpm --dir tui typecheck` | Pass (0) |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | Final pass (0): 895 tests, 96 suites; earlier concurrent run 894 pass / 1 gate failure |
| `git diff --check` / `git diff --cached --check` | Pass (0) |

Exact current Linux CI selectors and prerequisites (same pinned compiler and
provider-required environment where the workflow specifies it):

| Command | Result (exit) |
| --- | --- |
| `cargo build --bins` | Pass (0), default-feature executable prerequisite |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | Pass (0): 3,261 passed, 3 ignored |
| `cargo test --test contracts --test provider --all-features` | Pass (0): 196 passed, 5 ignored |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- boundary_suites::` | Pass (0): 194 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | Pass (0): 419 passed |
| `cargo run --example check_test_lanes -- --job rust-contracts` | Pass (0), actual workflow/Cargo discovery |
| `cargo run --example check_test_lanes -- --job rust-boundaries` | Pass (0), actual workflow/Cargo discovery |
| `cargo build --bin rustx` | Pass (0), actual TUI prerequisite |
| `uv sync --frozen` / `uv run --frozen pytest` in `test-support/fake-provider` | Pass (0): 51 tests |
| `pnpm --dir dev typecheck` / `pnpm --dir dev test` | Pass (0): 38 tests |

Native macOS selectors cannot execute as macOS on this Linux host. The existing
reviewed-head macOS lane passed all of its exact selectors and its discovery
audit; final pinned-head macOS results are not claimed.

Focused discovery and execution:

| Command | Result (exit) |
| --- | --- |
| `cargo test --lib --all-features --locked local_runtime::session::tests` | Pass (0): 137 passed, 1 ignored |
| `cargo test --lib --all-features --locked running_lineage_cut_retains_incomplete_origin_without_execution` | Pass (0): 1 discovered test, strengthened pre-settlement copy gate |
| `cargo test --lib --all-features --locked terminal_only_publication_loses_frozen_lineage_copy_race` | Pass (0): 1 discovered test |
| `cargo test --lib --all-features --locked terminal_only_publication_wins_frozen_lineage_copy_race` | Pass (0): 1 discovered test |
| `cargo test --lib --all-features --locked durable::sqlite::reading_tests` | Pass (0): 6 discovered tests |
| `cargo test --lib --all-features --locked runtime_client::response::tests` | Pass (0): 20 discovered tests |
| `cargo test --lib --all-features --locked goal351_model_create_goal_starts_no_nested_attempt_and_continues_after_settlement` | Pass (0): 1 discovered real continuation test |
| `pnpm --dir web-console exec vitest run test/reading-navigation.test.ts test/turn-navigator.test.tsx test/scroll.test.tsx test/turn-process.test.tsx test/conversation-width.test.tsx` | Pass (0): 65 tests, 5 files |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui exec node --test --test-name-pattern='native child transcript websocket' test/integration.test.ts` | Pass (0): 1 discovered test |

The export-only screenshot comparison used the exact reviewed source and the
same image, with alternate fixture/preview ports. The isolated two composer tests
passed; the preceding-Agent-state comparison reproduced both PNGs byte-for-byte.
Its four accessibility failures were missing archive-native binary prerequisites,
not executions used to certify this worktree. No screenshot baseline was written.
