# Session operation latency investigation

Measured on 2026-10-21, starting from `fe83a992`, on the local Linux host.
This investigation separates native mutation, runtime preparation, browser
navigation, and model generation. It does not change production behavior.

## Findings

The dominant delay reproduced with the user's configuration is **cold runtime
composition waiting for Exa MCP preparation**, rather than copying conversation
history. Fork and Retry then expose that wait because their Web navigation
sequence awaits an attachment before completing.

A warm switch between already attached Sessions is a separate, fast path. It
changes browser focus without rebuilding the runtime. Closing a browser view
and reopening it is also different from a cold runtime load: detach does not
unload the App Server's resident runtime.

## Actual-history measurements

The live runtime contained three Sessions, five Conversation databases and
approximately 18 MiB of runtime data. Each database was copied using SQLite's
read-only backup API into a temporary, isolated runtime, together with a copy
of its catalog and configuration. All native mutations and attachments below
ran against that copy. The live runtime and the user's configuration were not
modified. No original prompts were sent to a model.

The first comparison used the same source Conversation and latest completed
response cut with Exa enabled and with only `[agent.tools.sources]` removed in
the isolated configuration. The global Exa definition remained present; an
unselected source is inert. Source labels below are anonymized.

| Source | Fork copy, Exa selected | Attach, Exa selected | Fork copy, Exa unselected | Attach, Exa unselected |
| --- | ---: | ---: | ---: | ---: |
| A | 198 ms | 11,811 ms | 242 ms | 614 ms |
| B | 140 ms | 8,873 ms | 151 ms | 195 ms |
| C | 149 ms | 15,551 ms | 153 ms | 195 ms |

Checkpoints after these attachments took 31–54 ms. The measured attachment
phase accounts for approximately 98% of copy-plus-attach time with Exa
selected. Disabling Exa here is a diagnostic intervention, not a proposed
product fix. Remote timings vary; these samples do not establish a universal
network latency or percentile.

A later native Retry-preparation sample, with Exa selected, measured:

| Phase | Duration |
| --- | ---: |
| `session/branch`: copy the exact pre-input lineage | 145 ms |
| `session/switchNode`: unload the previous runtime and select the new node | 3,146 ms |
| `session/attach`: compose the successor runtime | 6,673 ms |
| `session/checkpoint` | 37 ms |

Thus approximately **10 seconds elapse before a replacement model request can
be admitted**. This sample deliberately stops before `turn/start`; it measures
preparation without provider inference or re-sending the user's prompt.
An Exa-unselected sample measured 13 ms for switching and 183 ms for successor
attachment. Retry samples are individual observations, not a paired benchmark:
initial composition can publish new system/configuration evidence in the
copied source, changing a later latest-history cut.

## Browser measurements

A production Web build, the debug native binary, and the real isolated
`web_fork_feedback` provider fixture measured the following first sample:

- Fork click to selected child route: **381.5 ms**.
- Switch to an already attached Session: **12.7 ms**.
- Reopen a closed browser view whose runtime remains resident: **120.9 ms**.

In that short-history Fork sample, the browser dispatched 17 native RPCs and
nine Product Host HTTP requests between the click and route publication.
They are partly concurrent, so their durations must not be added together.
There were four `session/settings` requests and two `session/list` requests in
that interval. This is secondary latency and request-budget pressure, especially
with a remote Product Host, but does not explain the reproduced 9–16 second
native attachment wait.

Reproduce the portable browser phase measurements from `web-console/`:

```sh
pnpm build
RUSTX_E2E_PREVIEW_PORT=5180 RUSTX_E2E_FIXTURE_PORT=5181 \
  bash scripts/browser-tests.sh session-latency-measurement.spec.ts \
  --output=/tmp/rustx-session-latency-results
```

The tests attach `session-latency.json` and emit `SESSION_LATENCY`, recording
only browser-clock timestamps, RPC method names, Product Host paths and response
sizes. They cover Fork, warm switching, resident reattachment, and Retry up to
provider admission. They introduce no RPCs, socket interception or fixed sleeps.
The deterministic fixture has no external MCP; its timings isolate the basic
client pipeline and must not be presented as an Exa benchmark. Assertions check
single mutation dispatch, reuse of warm attachments, protocol ordering and
absence of native errors, not machine-speed thresholds.

## Owners of the wait

1. `web-console/src/app/commands/native.ts`, `CommandSession.transition`:
   Fork awaits source admission, the real `session/fork` receipt, child
   attachment/checkpoint/subscription, and any native editor restoration.
   Retry additionally branches, awaits `switchNode`, then submits restored
   input exactly once. The child route is committed after these gates.
2. `src/local_runtime/session_controller.rs`, `copy_lineage` and
   `copy_admitted_lineage`: copy and durably publish the selected history.
   They do not compose a runtime or connect to Exa. This explains why the
   native Fork receipt is fast even while the overall UI action is slow.
3. `src/local_runtime/session_runtime_manager/lifecycle.rs`, `load` and
   `compose`: a warm runtime is reused. A new Conversation receives its own
   runtime and its own capability coordinator.
4. `src/local_runtime/composition.rs`, `compose_from_config`: awaits
   `prepare_captured_candidate` before activating the runtime.
5. `src/capabilities/coordinator.rs`, `prepare_candidate_from_inputs` and
   `prepare_mcp_server`: eagerly connect selected MCP sources and await
   `list_tools` before publishing the initial capability generation. The
   selected sources are prepared sequentially. Multiple slow sources can
   therefore add their startup durations.
6. `src/app_server/connection/catalog.rs`, `SessionSwitchNode`: unloads the
   old incarnation before committing the successor selection. Unload includes
   settling conversation-owned external resources; Retry pays this cost before
   a second cold composition.
7. `web-console/src/client/cold-reads.ts`, `readMetadata`: reads settings for
   the cold preview. `app-server.ts` reads them again after subscription, while
   `workspaces/session-admission.ts` also reads settings for fresh operation
   admission. Admission checks at the dispatch boundary prove current workspace
   authority and must not simply be removed or replaced with cached UI data.

The Exa intervention isolates selected external-source preparation as the main
cause. It does not distinguish DNS, TLS, remote handshake and remote tools-list
latencies inside that phase. In particular, rmcp's documented ten-second
`Auto` probe window is **not evidence** that every measured delay is that
fallback timeout; the observed durations vary and some are below ten seconds.

## DSH source comparison

The reference checkout was `deepseek-harness` at
`687f282e8625c6b8d11ff58211a90524be2e4317`.

- `packages/api/session-controller/src/commands.ts`, `fork`, builds the selected
  seed and creates the child Agent before returning its real identity.
- `packages/api/session-controller/src/client/sessions/service.ts`, `fork`,
  projects the real child into the list and calls `onCreated` immediately after
  the Host receipt, before the optional rename completes.
- `packages/client/ui-chat/src/client/apply.ts` opens the returned child after
  the service finishes. DSH also awaits native work; it does not fabricate an
  executable child before the Host has created it.
- `packages/mcp/mcp-client/src/index.ts` and `connection.ts` retain a supervised
  connection per MCP plugin instance, registering its tools until disposal.
  Whether creation starts a new MCP connection depends on plugin placement
  and the Agent preset. Source inspection alone does not establish the user's
  DSH configuration or its end-to-end timing.

Consequently the reliable comparison is ownership and publication order, not
an unsupported claim that DSH never initializes tools when forking.

## Optimization boundaries

The highest-impact UI change would separate **opening the durable child for
reading** from **making its runtime executable**. Native child identity and
exact cold history can be shown while external tools prepare, with a distinct
runtime/tool preparation status. Sending, Retry replay and live controls must
still require the exact acknowledged attachment and observation. This requires
an explicit read-only navigation state; bypassing today's attachment-proof
guard would grant the route semantics it has not proved.

Reducing actual Retry-to-provider time requires work on the MCP/runtime
lifecycle, including measured handshake stages and the old-runtime close
barrier. Parallel independent source preparation would help multiple selected
sources, but not the single Exa case measured here. Sharing MCP generations
across Conversations would change isolation, credentials, working directory,
notification and physical-shutdown ownership and needs a deliberate design.

After that, deduplicate supplementary settings/list reads without weakening
fresh dispatch admission. Loading indicators alone, shorter artificial timers,
turning off search, fabricated child identities, or retrying dispatched native
mutations do not solve this underlying delay.

## Validation

The final portable run passed both native browser measurements. It measured
381.6 ms from Fork click to selected child, 11.9 ms for the warm switch, and
392.0 ms from Retry click to replacement `turn/start` dispatch before the
provider's generation gates. TypeScript checking and provenance/notices checks
also passed. Production source and runtime behavior remain unchanged.

A subsequent change implements bounded HTTP transport reuse, parallel root
MCP-source preparation and Pi-style best-effort remote cleanup; see
[MCP startup optimization](mcp-startup-optimization.md). The measurements above
describe the original implementation.
