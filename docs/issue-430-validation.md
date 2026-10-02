# Issue #430 repair validation

The normative contract is [conversation-reading.md](conversation-reading.md).
App Server v30, Runtime Client v55 and subagent IPC v29 retain separate version
ownership. SQLite schema 48 adds turn-reading bootstrap provenance and a semantic
read-mutation epoch; old stores are refused without migration.

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

Separate `TurnReadingProvenance` belongs to Session lineage/bootstrap, rather than
finalized-response/Retry provenance. Clone, fork and tree branch share the same
retention/remapping seam. It keeps native origin/order, retained member IDs and
terminal-only predecessor/outcome/clocks, without copying Journal events or
execution state. Genuinely unowned content creates no turn. Destination-local
Attempts follow inherited origins.

The rail merges one bounded outline page with the live native Attempt. Current
identity, historical locate capability and loaded anchor are distinct. First
visible native process location causes one outline refresh before settlement;
subsequent same-Attempt progress causes none. ChatViewport alone owns automatic
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

The focused Web command discovers 46 tests in four files: reading navigation,
rail, scroll and width. Controlled replies prove both navigation reply orders,
seven authority/user replacements, exact installed cut/window and destination,
512-entry bounds, two in-flight reads, ordinary detached reading and Return to
latest. New exact regressions include:

- `an existing outline survives same-Attempt durable streaming progress and installs only its original cut`;
- `a loaded historical rail mark remains usable after same-Attempt append progress without refreshing its cut`;
- `current native identity precedes location; the first durable process location enables navigation before settlement`;
- `follow publishes the live native turn without a locate cursor; historical anchors and Return to latest own active reading`.

ResizeObserver/animation-frame controls prove user scrolling wins before a reply
and before the navigation frame, and detached positions survive width/sidebar/
right-panel, Tool disclosure and image growth. All 16 existing width tests retain
pointer capture, frame coalescing, two-pixel deliberate commit, press-only/cancel/
lost-capture behavior, storage failures, measured temporary clamps, keyboard
commits and reduced motion. No Chat `scrollIntoView` or second scroll coordinator
was introduced.

## Browser reference review

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

All 70 retained reference updates were individually inspected. The final normal
acceptance run passed all 146 tests in 9.2 minutes with strict pinned comparison,
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


## CI investigation

The reviewed failing run is [36879484371](https://github.com/Caismis/rustX/actions/runs/36879484371).
The exact base `93915c2265746c7c94d07146a51829470f9c4084` passed in
[36854600526](https://github.com/Caismis/rustX/actions/runs/36854600526).

- **Web:** large geometry mismatches are caused by #430's intentional shared width
  and rail. An initial outline reply could also be discarded by unrelated window
  selection, leaving loading chrome stuck; outline authority now survives window
  intent changes while retaining attachment/runtime/resync fences. Reviewed
  references are updated; strict comparison policy is retained.
- **Clippy:** successful base used rustc 1.98.1 (`48a229cea`, 2026-09-01), paired
  Clippy 0.1.98 (`48a229ceae`). PR logs explicitly record stable advancing to
  rustc 1.99.0 (`b940084d7`, 2026-09-28), paired Clippy 0.1.99 (`b940084d7e`).
  Exact-main archive and repaired PR both fail under 1.99 with five library errors
  and 455 test errors. The diagnostic class/file multisets match exactly:
  420 `assert_is_empty`, 26 non-empty assertion diagnostics, five deprecated
  atomic `fetch_update`, two redundant-else, one double-must-use and one needless
  borrow. Repair-owned diagnostics were fixed. Repository-wide mechanical cleanup
  belongs in independent main maintenance; hundreds of unrelated tests were not
  changed here. `-D warnings` is retained and no old toolchain pin was added.
  Local rustc/Clippy 1.95 validation passes; it does not substitute for current
  stable CI.
- **macOS Bash:** the unchanged
  `tools::native::bash::tests::stopped_anchor_supervisor_is_contained_by_the_outer`
  timed out waiting for settlement after native stop, successful unwedge kill,
  TERM, terminal observation, group reaping and ControlFailure evidence. The exact
  base passed it on macOS and Linux; six other recent successful main runs contain
  the same source history. No #430 diff touches Bash ownership. Evidence points
  to an existing platform settlement failure, not a reading regression; the exact
  root cause is unproven without macOS reproduction. No Bash changes, retries,
  skips or timeout increases were made. The full Linux test passes this case;
  native macOS is unavailable locally.

## Executed validation

All commands run in the independent Issue #430 worktree. Node is 24.21.0 and pnpm
11.13.1. Rust uses `CARGO_PROFILE_DEV_DEBUG=0` / `CARGO_PROFILE_TEST_DEBUG=0` to
reduce generated debug metadata, without changing assertions/optimization.
Provider-bearing commands require the existing emulator. The full native run uses
`TMPDIR=/home/caismis/.cache/rustx-443-tests-tmp`: pre-existing `/tmp/.git`,
`/tmp/.agents` and `/tmp/rustx.toml` otherwise make unrelated launch fixtures share
an unintended workspace. Those ambient files and the primary checkout were not
modified. An initial full run exposed that contamination and two obsolete schema
47 assertions; the assertions now require schema 48 and the complete isolated
run passes. An early binary build exhausted a tmpfs quota; build artifacts were
moved to this independent worktree's ignored disk-backed target directory.

| Command | Final result |
| --- | --- |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | Passed: 71 files, 1,317 tests |
| `pnpm --dir web-console check:i18n` | Passed |
| `pnpm --dir web-console check:provenance` | Passed: 147 source records, 131 package notices |
| `pnpm --dir web-console exec vitest run test/reading-navigation.test.ts test/conversation-width.test.tsx test/scroll.test.tsx test/turn-navigator.test.tsx` | Passed: 4 files, 46 tests |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | Passed: all 146 tests, strict pinned references, 9.2 minutes |
| `pnpm --dir protocol/app-server generate` | Passed: v30 schema/types/fixtures |
| `pnpm --dir protocol/app-server check` | Passed: regeneration has no unstaged artifact diff |
| `pnpm --dir protocol/app-server typecheck` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Passed with local 1.95 |
| `cargo +1.99.0 clippy --all-targets --all-features --locked -- -D warnings` | Failed identically to exact main: five library / 455 test diagnostics |
| Exact main archive: same Clippy 1.99 command | Failed with matching diagnostic class/file multiset |
| `cargo build --bins --all-features --locked` | Passed |
| `cargo build --bins --locked` | Passed: actual default-feature CI prerequisite |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked` | Passed: 4,067 tests, 8 ignored, no failures; lib 3,432 + external targets 615 + examples 20 |
| `cargo test --lib --all-features --locked durable::sqlite::reading_tests` | Passed: all 6 discovered tests |
| `cargo test --lib --all-features --locked runtime_client::response::tests` | Passed: all 20 discovered tests |
| `cargo test --lib --all-features --locked clone_fork_and_branch_share_native_unsuccessful_turn_provenance` | Passed: 1 discovered test; also full suite |
| `cargo test --lib --all-features --locked goal351_model_create_goal_starts_no_nested_attempt` | Passed: 1 real continuation test; also full suite |
| `cargo run --all-features --example check_test_lanes -- --job rust-contracts` | Passed: actual workflow/Cargo discovery |
| `cargo run --all-features --example check_test_lanes -- --job rust-boundaries` | Passed: actual workflow/Cargo discovery |
| `pnpm --dir tui typecheck` | Passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | Passed: 895 tests, 96 suites |
| `pnpm --dir dev typecheck` / `pnpm --dir dev test` | Passed: 38 tests; no source changes or wrapper required |
| `uv sync --frozen` (fake-provider) / `uv run --frozen pytest` | Passed: 51 tests |
| Frozen pnpm installs (Web, protocol, TUI, dev) | Passed; lockfiles unchanged |
| `git diff --check` / `git diff --cached --check` | Passed |

Linux results do not establish native macOS behavior. The hosted state is a
snapshot; after the existing branch is pushed, CI is not continuously monitored.
