# PR #301 attachment lifecycle correction

Validated 2026-09-14 against reviewed head
`0a8ff55c62a1f619128f908561bf796a40975091` and unchanged `origin/main`
`cd5b9a04a1f0cb1403059c01b723be4f90e1811f`.

## Ownership and native contract

React's `tabs`/selection own visibility. The client separately records
`attachmentIntent: wanted | released`, its observed `attachment`/target, and
uncertain operations. Explicit Open/Attach records wanted intent. Detach, unload
and Close record released intent at admission, before awaiting any RPC. Neither
acknowledgements nor errors rewrite intent. Reconnect reads current intent after
each await; it never manufactures new intent.

Close removes the tab immediately and releases only the external relationship
with `session/detach`. Per-Session serialization handles close during attach and
reopen during detach using the exact target. The queue is bounded and generation
fenced; it does not retry. A failed/lost detach remains diagnostic without
reopening presentation. Existing reload navigation hints drop released Sessions;
no new persistent state is introduced.

This follows the inspected native implementation:

- `src/app_server/connection.rs`: `AttachReservation` enforces 32 active/reserved
  attachments per connection and rejects duplicate Session claims. `release_route`
  removes only the exact route; `SessionDetach` calls it without a runtime lease.
- `src/runtime_client/attachment.rs`: `RuntimeAttachment::detach` relinquishes the
  external claim and subscription. It does not cancel execution, settle an
  interaction, unload or delete.
- Native unload is a distinct manager-owned operation whose terminal result retires
  its initiating route. The browser cannot infer that result from transport loss.
- `docs/app-server-protocol.md` documents those same ownership boundaries.

No Rust, wire DTO, generated artifact, Harness presentation, dependency or CI
configuration changes were needed for this correction.

## Deterministic regressions

The fixture now tracks per-socket claims/reservations, native duplicate/capacity
rejection, exact-target validation, loaded runtimes and cold loads. State commitment
and response delivery are separate explicit operations; no timing sleeps are used.

| Regression | Invariant |
| --- | --- |
| A → B → A switch and unmount | No RPC; both open Sessions remain attached |
| Close attached A with B open | Exactly A's detach; no unload/cancel/interaction/delete; B unchanged and A's running/pending facts retained |
| A close acknowledged, reconnect, explicit reopen | Only B automatically attaches; explicit Open A acquires one fresh snapshot restoring Approval |
| 40 distinct historical open/close owners | At most two live claims with B retained; loaded runtimes survive; reconnect attaches only B |
| Lost detach acknowledgement | Fixture commits detach, drops transport before reply; intent stays released and result uncertain; no replay/reattach |
| Lost unload acknowledgement | Fixture commits unload before transport loss; reconnect leaves A unloaded with no extra cold load; only explicit Attach reloads it |
| Close during held attach | Waits for the exact acquired target and releases it once |
| Reopen during held detach | Wanted intent changes immediately; duplicate Open coalesces into one fresh attach and restores Questionnaire |
| Release while reconnect awaits another Session | Reconnect re-reads intent instead of using a captured wanted flag |
| Rejected detach | Released intent remains separate from still-attached observation |
| Lost close-triggered detach reply | Closed tab stays closed, uncertainty remains globally visible, reconnect acquires only B |
| Released but visible tab, fresh page | Existing resume hint is removed before unload acknowledgement; reload cannot reacquire A |

Existing interaction settlement/loss, stale-generation, routing, raw log and
source-derived shell regressions remain green. The original close/unmount test
initially failed because it required retaining a hidden attachment; that obsolete
assertion was replaced. Final deterministic total: **43 passing tests**.

## Real-server browser evidence

The production-built console ran in Playwright Chromium against the actual rustX
App Server and controlled external HTTP/SSE provider emulator, with no Harness
backend. A remained active while B completed. After socket reconnect and page
reload, the test closed A and awaited detach acknowledgement before releasing the
provider's completion gate. It awaited provider completion with no A controller,
then reopened A and recovered its committed answer in the same runtime incarnation.

On that connection, 34 alternating A/B close/reopen cycles each awaited detached
and attached observations. The existing Approval/Questionnaire disconnect/reload
recovery, interaction published while detached, raw JSON-RPC inspection, explicit
unload/cold configuration resolution, history/cwd preservation and deletion also
passed. All provider steps and process exit remained checked. Lost-unload reply
delivery is tested deterministically, not with a timing-sensitive browser cut.

This is automated real-server browser coverage, not a claim of manual dogfooding.

## Executed validation

| Directory | Command | Result |
| --- | --- | --- |
| `web-console` | `pnpm install --frozen-lockfile` | Pass, lockfile unchanged |
| `web-console` | `pnpm typecheck` | Pass |
| `web-console` | `pnpm test` | Pass, 43 tests in 3 files |
| `web-console` | `pnpm build` | Pass |
| `web-console` | `pnpm test:e2e` | Pass, 1 full real-server browser scenario |
| `protocol/app-server` | `pnpm install --frozen-lockfile` | Pass, lockfile unchanged |
| `protocol/app-server` | `pnpm check` | Pass, generated artifacts unchanged |
| `protocol/app-server` | `pnpm typecheck` | Pass |
| repository | `cargo fmt --all -- --check` | Pass |
| repository | `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| repository | `cargo test --lib --all-features app_server` | Pass, 10 tests |
| repository | `cargo test --test process --all-features app_server` | Pass, 8 tests |
| repository | `git diff --check` | Pass |

No lint configuration exists in the console, so no lint command is claimed.
