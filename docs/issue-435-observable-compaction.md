# Issue #435: observable native context maintenance

## Scope and baseline

Implementation starts at `06dc8b101acf5a880466548e9e9b3214e41ac7e8`, fetched
`origin/main`, in `rustX-issue-435` on `issue-435-observable-compaction`.
The primary checkout remains on that commit with its existing untracked
`.playwright-mcp/` untouched. Main was fetched again before final validation;
PRs #445 and #446 remained open, and main had not moved. Neither PR is a base.

## Ownership and ordering

`AgentComposer` command invocation → `App.invokeCommand` →
`AppServerClient.compact` → App Server `context/compact` → Runtime Client →
`ConversationRuntime::compact_context` is the sole manual execution path.
The client synchronously claims its Session/attachment/generation request slot
before consuming the exact command draft and before RPC dispatch. The generic
compact modal, second confirmation action, compact-specific modal copy, and
`CommandSession.compact` path are removed. Other commands retain their architecture.

The Context engine owns summary construction and the durable summary/Surface
commit. The maintenance coordinator admits, excludes competitors, restores the
Conversation, and releases ownership. Runtime Client owns projection; Web owns
intent, transport evidence, scope fences, and presentation only.

1. Local request evidence exists before dispatch. It is not native admission.
2. Native admission checks out ConversationState and publishes manual start.
3. The owned task commits summary and Surface. Its settlement gate can still hold
   coordinator ownership at this point; the RPC is unfinished and no terminal
   manual observation has been published.
4. `finish_manual_compaction` restores ConversationState and clears the slot under
   the coordinator lock, then publishes committed history and terminal manual
   observation. The waiter completes afterward. This is the release boundary used
   by the resident surface. Automatic context completion does not settle its Attempt.

The new UI adds no canonical history. Existing native summary identity and transcript
paging remain responsible for checkpoints. No fake command message, synthetic
Attempt, execution queue, polling, retry, or cancellation mechanism is introduced.

## Repair and applicability

Old projection was insufficient: a false progress bit or a newer checkpoint cannot
identify a lost request. App Server v33 requires `request_id`; Runtime Client v57
retains one live `manual_compaction` record with that ID, `released`, and the native
pre-commit error. Start/failure events now carry the complete context projection.
The correlation travels with the owned maintenance task, independent of its waiter.
All generated/current consumers move atomically; v32 is removed, not supported.

Reconnect reads native state without resending the mutation. Matching released
correlation resolves the pending request. No record, an overwritten record, another
client's ID, or a newer unrelated checkpoint leaves the request uncertain. Automatic
compaction does not overwrite manual correlation. This is deliberately not durable
operation recovery across runtime incarnation replacement. Authority, Conversation,
incarnation, attachment and connection-generation fences retire obsolete evidence
and callbacks. A confirmed successful RPC remains successful after a read failure.

Occupancy required no new native field. The native reader already pairs the latest
prepared request's measured input with its frozen model/capacity. A newer unmeasured
request and compaction invalidate it; later measured usage restores it. Zero is
measured zero. Missing input or invalid capacity is unavailable, not zero. Desired
model changes, transcript paging, and unsent draft text cannot supply measurements.
The resident Composer disclosure names the historical model and input/capacity.

PR #409 (`f268175bb8d31010706e7070aae80d2b46b7aced`) removed occupancy from
`ConversationStats` while rebuilding resident conversation/Turn presentation. Its
diff and PR description establish that placement change, not a separate rationale
for removing occupancy as a product capability. This implementation retains the
resident Turn design and adds the measurement to the current Composer context stack.

## Deterministic acceptance evidence

| Contract | Evidence |
| --- | --- |
| Direct command, one request, exact draft consumption, no modal or User turn | `commands.test.tsx`: compact invocation with held RPC, repeated Enter, subsequent draft, request count, no `turn/start` |
| Submission, start, commit versus release, remount | `context-seat.test.tsx`: held response; native progress with newer committed count remains running; remount; matching release |
| Actual native commit/release boundary, no synthetic Attempt | `manual_compaction_commits_and_restores_the_idle_conversation`: post-commit settlement gate, durable generation, owned slot, unfinished waiter, correlated observations; projection regression |
| Native-only automatic/other-client observation and useful errors | `context-seat.test.tsx`: uninitiated progress/error and other-client progress during own uncertainty; native pre-commit maintenance regressions |
| Lost response before sufficient evidence, after release, failed release, unrelated/newer checkpoint | Four correlation cases in `context-seat.test.tsx`; exactly one request after reconnect, exact uncertainty removal only |
| Obsolete authority/Conversation/incarnation/attachment/generation/later request | Held old replies and old-socket events in `context-seat.test.tsx`; replacement attachment starts a newer request before old response is released |
| Pending inbound admission | Command registry/client pending-inbound regression and existing native maintenance admission contract; no empty-queue rule added |
| History overlap/paging | Existing `transcript.test.ts` overlap, refresh, resync and reconnect fences; no second historical representation added |
| Occupancy zero/absent/invalid capacity and remeasurement | Four Web measurement cases; native `context_measurement_is_native_and_is_invalidated_by_compaction` adds newer unmeasured request and genuine zero to existing frozen-model, compaction and remeasurement coverage |
| Browser languages, themes, keyboard details, narrow layout and non-modal interaction | Four native `compaction.spec.ts` cases: en/zh, light/dark, 1440/390; real provider gate, exact measurement, diagnostic disclosure, retained editable draft, invalidation; en/light reconnects while held |
| Composer residency, reading/streaming stability | Existing full browser suite, including Agent, accessibility, transcript/streaming and navigation coverage, retained with the new Context stack seat |

Tests use deferred RPCs, native settlement gates and provider emulator gates. Timeouts
remain liveness guards; no sleeps prove ordering. Screenshot references reflect the
new resident row. The obsolete live Composer corner noise allowance is removed;
its original image and all comparator assertions remain frozen regression evidence.
The new crop is compared exactly. No screenshot tolerance or timeout was increased.

## Reference audit

The approved read-only DeepSeek Harness pin is
`639ed015397290b3745d163aafe02ffee4aa3f84`. The eight actually inspected paths and
hashes are listed in `web-console/PROVENANCE.md` and `source-inventory.json`.
They cover CompactionCommandCard, CompactionItem, GenericCommandCard, compaction and
command node projection, the chat snapshot contract, ContextMeter, and occupancy.
The local reference checkout was neither switched nor modified. Behavioral ideas
inform the non-modal disclosure; native execution/storage remain rustX-owned.

## Validation environment

Validation uses Linux x86_64, the repository toolchains, provider emulator, and pinned
Podman Playwright image. Native tests use
`TMPDIR=/var/tmp/rustx-issue-435-validation`: the ambient `/tmp` already contains an
unrelated `.git` and `rustx.toml`, which interfere with parent-discovery tests.
Those files were left untouched. The isolated temporary root changes no assertions.
macOS and Windows execution were not available in this environment.

The CI-equivalent commands used for this change are:

| Command | Result |
| --- | --- |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | 74 files, 1,373 tests passed |
| `pnpm --dir web-console check:i18n` | Passed |
| `pnpm --dir web-console check:provenance` | 147 source records and 132 production package notices verified |
| `pnpm --dir protocol/app-server check` | Generated contract matches |
| `pnpm --dir protocol/app-server typecheck` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Passed |
| `cargo build --bins --all-features --locked` | Passed |
| `TMPDIR=/var/tmp/rustx-issue-435-validation RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked` | 4,089 passed, 0 failed, 8 existing ignored, across 19 test binaries |
| `pnpm --dir tui typecheck` | Passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | 895 passed, 0 failed, 0 skipped |
| `pnpm --dir dev typecheck` | Passed |
| `pnpm --dir dev test` | 38 passed, 0 failed |
| `uv run --frozen pytest` in `test-support/fake-provider` | 51 passed |
| `git diff --check` | Passed |

Focused discovery was also verified: 111 command/Composer/context-seat tests;
6 native manual-compaction tests; 18 Agent/compaction browser cases; and the native
Composer dock case after its stack assertion was updated. Initial browser failures
identified the expected changed screenshots and obsolete stack cardinality, which
were corrected without changing product admission behavior or weakening assertions.

The eight existing ignored native tests are two opt-in startup/stage measurements,
one fixture-corpus regeneration writer, and five live paid-provider tests requiring
API credentials. They were not executed; deterministic provider-emulator coverage
was required and executed. Hosted CI is left to run after PR creation and is not
continuously monitored by this task.

Final browser validation:
`CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` — **152 passed, 0 failed
(9.4 minutes)**, with screenshot updates disabled. Main was fetched once more before
delivery and remained `06dc8b101acf5a880466548e9e9b3214e41ac7e8`; #445 and #446
were still open. No main integration was necessary.
