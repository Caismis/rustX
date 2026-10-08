# Issue #454 unified `present` delivery: architecture record and validation

Validated on Linux x86_64 on 2026-10-08 in the independent worktree
`../rustX-issue-454`, on branch `issue-454-unified-present`. It is based on fetched
`origin/main` `26def795775884d07e013dc51939028a52cb56b2`. The primary checkout was
not modified.

## Ownership after this change

| Concern | Owner | Evidence |
| --- | --- | --- |
| Execution, cancellation, terminal settlement and commit | Native `present` Tool, ordinary Tool Plane, `AgentLoopExecution::commit_tool_result_batch` (unchanged) | `present.rs` order/duplicate, malformed-tail and post-validation cancellation tests. The boundary scenarios show exactly one committed Tool message. |
| Delivery truth | `deliveries` on a successfully committed canonical Tool-result message | Web `presentedDeliveries`, TUI `pageDeliveries`/`resultCommitted`. Failed results, cancelled results, tool JSON, prose and foreground settlements yield nothing (unit tests in both clients). |
| Delivery bytes and native location | `app_server::delivery_access`, one native core with every fence | Product Host scenario (unchanged behavior) and new delivery-access scenario share it. |
| Who may enter | Transport authentication only: the Product Host secret lane, stdio-owner delegation (`--stdio-delivery-access`), or the separate WebSocket credential (`--delivery-access-token-file`) | Process, transport and scripted tests below. |
| Presentation | Web: Harness-derived `PresentRow`/`PresentedFileCard` via `bindings/present.ts`. TUI: pure `tool-present` renderer and `/files` selector | Web/TUI unit tests and e2e. |
| Client-local effects | TUI `app-server/delivery-files.ts` only | Renderer purity contract test; selector dispatches intents only. |

No delivery database, registry, DSH Session event, Agent Loop branch, provider
behavior, generic filesystem RPC or Artifact Store change was added.

## Authorization threat boundary

- An ordinary authenticated connection can supply exact coordinates and a
  trusted-looking client name (`rustx-tui`, `rustx-product-host`), but `delivery/read`
  and `delivery/locate` still fail `session_file_read/unauthorized` before lookup.
  The Product Host secret offered on the ordinary lane, the transport token reused
  as a delivery credential, an empty or wrong credential, or a delivery credential
  without the transport token: each is refused at the handshake.
- A granted connection reaches only its own attachments (`stale_attachment` for
  another connection's target). Invented coordinates and non-delivering indexes are
  `unavailable`; index ≥ 8 is `invalid_params`.
- Native credential removal revokes live grants. Connection close cancels the
  connection's authority. With a gate held after leaf open and before bytes, close
  publishes no bytes, the permit stays owned until the blocking read physically
  settles, and then both permits return.
- Fork reads resolve the original Conversation and root. A same-named file in an
  unrelated Session root is `unavailable`. Mutable replacement, deletion
  (`missing`), oversize (`too_large`) and capacity (`capacity`) are explicit.
- Locations are verified leaves. The TUI opens locally only for its own spawned
  child, after its own `lstat` device/inode matches. A remote TUI never requests a
  location. Startup rejects reused secrets, group/world-readable credential files and
  cross-transport flags.
- The Product Host credential and lane are unchanged and never given to the TUI or
  browser. The browser never offers the delivery credential.

## Deterministic regressions added

Rust:
- `transport_granted_delivery_access_reads_and_locates_only_through_own_attachment`
  (boundary suite): capability reporting, forged names, ordinary rejection,
  own-attachment routing, bytes/location equality, fork original identity,
  unrelated roots, mutable/missing/oversize, capacity, gated close-before-bytes
  revocation and physical permit settlement, zero model requests.
- `websocket_delivery_access_is_a_separate_additive_revocable_credential`: real
  WebSocket handshakes and live revocation.
- `app_server_delivery_access_is_explicit_transport_composition`: real binary flag
  validation, stdio delegation and WebSocket credential.
- `location_is_the_verified_leaf_identity_without_a_size_bound`: descriptor walk,
  replacement, symlink, unrelated root and post-open revocation fence.

TUI:
- `deliveries.test.ts`: committed-only records, renderer/forgery cases, pre-commit
  card suppression, bounded decoding, no-clobber byte-exact save and cancellation,
  shared-host/leaf checks for Open, selector intents, stale-outcome fencing and
  explicit paging.
- `delivery-integration.test.ts` (real child and real socket, provider emulator):
  owned stdio save with Unicode/spaces/CRLF bytes, refusal to overwrite, mutable
  reopen, oversize and missing failures with no output, cancellation before write,
  verified local open. Remote without the credential shows metadata only and
  refuses bytes; a wrong credential is refused; with the credential, Save works
  client-side after reconnect and Open is never attempted. Both prove zero
  additional provider requests.
- `app.test.ts`: `/files` through real input routing, with no read without access
  and an exact-record read with access.

Web:
- `present.test.tsx`: lifecycle-only phases, call row never renders cards,
  keyboard disclosure, four-card summary, canonical order, separate Preview and
  Download intents, en/zh.
- `file-delivery.spec.ts` (e2e): updated for the Harness card DOM and the
  collapsed summary. It checks call-row phases, and that the browser's ordinary
  connection gets `delivery/read` refused as unauthorized.

## Review repairs

The independent review of `d35b929f` found four P1 findings and one P2 finding.
The first two had one cause: an ordinary-lane delivery request had no owner
between admission and transmission.

| Finding | Correction | Regression |
| --- | --- | --- |
| P1: Escape in `/files` abandoned only the local Promise; the native read kept running | Each `delivery/read`/`delivery/locate` is a `delivery_access::Operation` registered under its exact id before admission. New `delivery/cancel` cancels only that connection's own id and fails the request's native fences. The TUI client sends it when the action's signal aborts and keeps the correlation until the one terminal response. | `delivery_cancellation_is_request_scoped_at_every_native_interleaving` (before admission, after admission before open, after open before bytes, after settlement before publication, after publication, concurrent sibling, other/ordinary connections, reuse); TUI client correlation tests; real stdio cancel in `delivery-integration.test.ts` |
| P1: a produced response could be written after revocation | The response is queued with its `Publication`; `transport::Outgoing` commits it immediately before the physical write, rechecking cancellation, delivery authority and the attachment, and substitutes the same id's typed failure | `delivery_publication_commits_at_the_transport_writer` (real stdio writer parked after native settlement, before commit: credential, detach, close, locate; complementary unrevoked case; unrelated `server/info`; permits restored); `delivery_revocation_before_publication_commit_suppresses_sensitive_responses` |
| P1: Save prefilled the editable Input with the raw delivered name | `DestinationInput` holds only renderable text (refuses unrenderable `setValue` and whole pastes); the name prefills only when it renders as itself; outcome text is sanitized; `deliveryDestination` has no name fallback | `deliveries.test.ts`: ESC/CSI, OSC, C1, LF, CR, bidi override/isolate, ALM through the real Input render; Unicode/space names prefilled exactly and saved byte-exactly |
| P1: the location test assumed unlink + recreate yields a new inode | The original stays allocated (renamed aside) while the replacement is created; adds a swap after the owned open, which fails `Replaced` | `location_is_the_verified_leaf_identity_without_a_size_bound`, run 30 times consecutively |
| P2: retiring `/files` left the operation running | One abort scope per `/files` overlay, retired by `#closeOverlay`, which every ending passes through. Save commits after `sync` + `close` and removes only the device/inode it created. Open commits at the spawn. | `app.test.ts` retirement test (Escape, overlay replacement, snapshot replacement, disconnect, quit; the native read is cancelled and no file appears even when the bytes arrive late); `deliveries.test.ts` Save/Open commit-point orderings |

Negative controls: with the publication decision forced to `Ok`, the three Rust
publication/cancellation tests fail. With cancel not cancelling the request token,
the cancellation test fails ("never admitted"). With the interaction abort removed,
the app retirement test fails on overlay replacement. With the raw `Input` and raw
prefill, the hostile-name test fails on ESC/CSI.

The protocol stays v38: v38 is introduced by this unmerged PR, and main is v37.

## Validation

See the pull request for the final command list and results; the PR description
records the exact pass/fail counts of the final head.
