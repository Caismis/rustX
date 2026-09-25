# PR #404: native model catalog authority and Settings-observation teardown

Starting HEAD: `98e8425adb8f7aa5f891d649f7540d9d6fe87432`.
Base: `5df40f29c72ee5179864bf2f4671c3f8d32dbc18`.

## New Conversation model catalog

New Conversation built its model menu from `SourceSettings.resolved.models`, a
source-resolution document. That made the browser a second model-selection
authority beside `session/models`.

Native owns the answer now. `ApplicationState::creation_capture` is the one path
from a Workspace to the capture a new Session binds. It returns the published
`available` capture, or the capture creation would validate on first use.
`initial_binding` (`session/create`) publishes that capture.
`session_creation_models` only reads it: it resolves the capture's catalog with the
process credentials, builds the `ModelBindingRegistry` and returns its
`catalog_view()`. `SessionRuntimeManager::source_settings` projects the result as
`SourceSettings.session_models` for Workspace targets, under the same application
lock that already projects `application`:

- `available { catalog }`: the Session catalog in native order, with protocol,
  declared and effective capabilities, reasoning profiles, default profile and
  redacted credential source.
- `unavailable { diagnostic }`: Session creation would fail here.

The Product Host Workspace read already carries this projection, so no new Host
operation or pre-Session RPC exists and no Session is created early.

`bindings/model-catalog.ts` is the single `ModelCatalogView` to selector mapping
and admission check. The Session selector and New Conversation share it. New
Conversation offers only `session_models`, and a selection stays draft Session
intent. Native unavailability, or a draft model that a later authoritative read no
longer publishes, blocks Send. The first-submit contract is unchanged: create is the
commit point, explicit model intent is applied after the Session exists, and the
first upload and turn wait for its authoritative observation.

## Settings-observation teardown failure

CI run `36086252243` failed `settings-observation.spec.ts:61` with `fetch failed`
(`other side closed`), then `Target page, context or browser has been closed`.
The trace shows every assertion passed and `After Hooks` started. So `f.stop()`
succeeded, and the error was an unhandled rejection during context teardown.
All six traced Product Host requests had completed. A proxied request whose
`fetch` fails records no `Fulfill request`, so the trace cannot show that one.

Reproduction: keep the page open for three seconds after `f.stop()`. About 6 ms
after the App Server exits, the page sends `POST /product-host/list`. The
`routeWorkspaceHost` proxy then fetches the Workspace Host that `stop()` just
closed. The result is `ECONNREFUSED` locally and `other side closed` in CI, where
undici reuses a keep-alive socket that `server.close()` is tearing down. The
failure depends on whether the context closes before the page reacts.

Cause: this PR keyed `<NewConversation>` on `endpoint`, `connection`,
`authorityRevision` and `generation`. Every transport transition remounted the
route, which re-listed Workspaces and destroyed the first-submit actor. A transport
drop right after `session/create` could therefore lose the committed Session from
the presentation. The route is now keyed only on its navigation epoch:

- Each `SUBMIT` carries the port captured at its gesture, so the authority fence
  belongs to the submission, not to the mount.
- Workspaces are re-listed only when endpoint or authority revision changes.
- Enablement reads the live transport.

After the change, the same three-second probe records no page request in three
repetitions. The unchanged spec and the complete suite pass.

The earlier note in [review-corrections.md](review-corrections.md) treated the
first occurrence as a fixture accident. That was wrong; it had the same cause.

## Committed Session recovery ownership

The navigation-lifetime correction above deliberately did not make a
submission's transport port durable. `FirstSubmitPort.current()` is still the
authority for native continuation only: attach, model observation, upload and
send must stop when its endpoint, generation, authority or target fence is
replaced.

That is not the authority for presentation of an acknowledged `session/create`.
Successful create stores the exact `CreatedSession` in `firstSubmitMachine`
before the next continuation fence is evaluated. From that commit point, a
transport replacement may stop the old continuation, but it cannot erase the
native Session identity. `NewConversation` therefore transfers a committed
Session through its current navigation-epoch callback, rather than through the
obsolete submission port:

```text
submission continuation authority != committed Session presentation authority
FirstSubmitPort.current()           != current New Conversation navigation epoch
```

If the route is still current, recovery opens that exact native Session after a
confirmed create even when attach is fenced by a dropped connection. The App's
normal Session route can then attach or reread under the new generation; no
create or other native mutation is replayed. If a newer navigation invalidates
the old epoch first, the old route cannot open or steal focus, while the native
Session remains discoverable from authoritative Session state.

## Deterministic proof

- `issue402_workspace_session_models_are_the_created_session_catalog`
  (Rust): the Workspace read catalog equals the created Session's
  `session/models`. It excludes a model authored after the Workspace binding was
  published. It is absent for User. A Workspace where creation fails reports
  `unavailable` and creates no Session.
- `new-conversation.test.tsx`:
  - Choices are exactly the native catalog in native order, and a
    configuration-only model is absent.
  - Native reasoning profiles and default are shown as published.
  - A selection writes nothing and creates nothing. After Send, `session/setModel`
    carries the exact intent while the turn waits.
  - Read failure and native unavailability offer no choice and never reach
    `session/create`.
  - An obsolete Workspace read cannot replace the current Workspace's catalog.
  - A model the catalog stops publishing blocks Send.
  - A transport drop keeps the draft with no Product Host traffic, and the next
    Send binds the reconnected generation.
- `first-submit.test.ts`: a replaced authority's SUBMIT is refused, and a later
  SUBMIT on the current authority proceeds.
- `new-conversation.test.tsx`: a create acknowledgement followed by a transport
  replacement opens the exact committed ID while attach/model/upload/send stay
  fenced and create remains single-shot; reconnect does not replay create. A
  replacement navigation epoch prevents the older route from opening its
  committed Session.

## Validation commands

| Command | Final result |
| --- | --- |
| `cargo fmt --all -- --check`; `git diff --check` | Pass |
| `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | Pass: 3,078 tests, two pre-existing ignores |
| `cargo test --lib --all-features -- boundary_suites::` | Pass: 225 tests |
| `cargo test --test contracts --test provider --all-features` | Pass: 28 and 166 tests |
| `pnpm --dir protocol/app-server check` / `typecheck` | Pass: generated `v21` includes `SessionModelsView` |
| `pnpm --dir tui typecheck` / `test` | Pass: 852 tests |
| `pnpm --dir web-console typecheck` / `test` | Pass: 53 files, 942 tests |
| `pnpm --dir web-console check:provenance` / `build` | Pass; `NewConversation.tsx` rehashed with a #402 note |
| Three-second post-`stop()` probe on `settings-observation.spec.ts:61` | Before: `/product-host/list` fires and the test fails 2/2. After: no request, 3/3 pass |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | Pass: 89 scenarios, 5.6 minutes; no reference image changed |
