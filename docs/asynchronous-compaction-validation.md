# Independent compaction verification

Independent generation and current installation authority share the sequential
Context kernel. A Session-scoped operation retains its captured source and
Auxiliary invocation across Attempts. Current native authority admits its result
at a Safe Boundary, idle maintenance boundary or mandatory recovery boundary.
Ready alone grants no installation authority.

## Invariant coverage

| Contract | Owning evidence |
| --- | --- |
| Detached lifetime, exact cancellation, frozen staged input, useful waiting and background/blocking competition | [Runtime tests](../src/runtime/conversation_runtime/asynchronous_compaction/tests.rs) |
| Active generation prevents idle eviction; joined Ready permits ordinary residency; idle-claimed shutdown records cancellation before quiescence | `idle_claim_retires_deferred_ready_and_supervises_generation` in the Runtime tests |
| Asynchronous work has finite starts/recovery and retains its Auxiliary scope without a second operation timer or Ready TTL | [Budget tests](../src/context/compaction_budget.rs) and `accepted_ready_result_can_install_after_original_scope_deadline` |
| Complete ordered source consumption and sealed source identity | [Chunk tests](../src/context/chunks.rs), [coverage proof](../src/context/coverage.rs) |
| Compatible strict coverage advancement, equal/smaller/incomparable results, actual generation provenance and atomic current-head admission | [Store coverage tests](../src/durable/sqlite/projection/coverage/tests.rs) |
| Atomic initial admission and exact durable restart reconciliation | [Operation observation tests](../src/runtime/conversation_runtime/compaction_observation.rs), [live bootstrap tests](../src/runtime_client/host/tests/compaction_head_tests.rs) |
| Actual process death while generating and Ready; no intermediate-job replay or false commit after cold recovery | [SIGKILL conformance](../tests/conformance/agent_loop/asynchronous_process_death.rs) |
| Real HTTP generation across completed/cancelled Attempts, frozen model/Profile, more than twelve accepted fragments, one format downgrade, retained tail and one idle checkpoint | [HTTP conformance](../tests/conformance/agent_loop/asynchronous_compaction.rs) |
| Exact per-operation Trace ownership, typed outcomes, bounded physical requests and historical invocation identities | [Trace tests](../src/runtime_client/trace/tests/compaction_tests.rs), [Auxiliary tests](../src/durable/sqlite/auxiliary/tests/retry_title.rs) |
| Delayed Attempt settlement, manual receipt identity and typed Inspector status | Web `incremental-vocabulary`, `compaction-receipt`, `context-seat` and `trajectory` suites |

The real process test holds the primary request through provider gates, binds
the exact native Summary operation, verifies Ready through actual protocol
notifications and kills the isolated process group with SIGKILL. Both ordinary
incremental context facts and bounded `CheckpointAdvanced` notifications are
authoritative representations. Cold recovery must preserve the original Call,
request and usage facts while recording interruption without dispatching work.

## Controlled measurements

HTTP conformance emits `RUSTX_529_CONTROLLED_METRICS` from the original Journal
and Auxiliary audit cut. It records admission-to-next-primary-start,
admission-to-Ready and Ready-to-installation intervals, accepted fragments,
physical requests, blocking replacements and the latest native primary occupancy.
The first interval includes capture and ordinary request preparation; it does
not claim an isolated causal overhead or a paired blocking-baseline speedup.

Usage reports known component sums together with counts of requests lacking
each measurement. The rejected Native request's usage remains separately
identified; missing input, output or reasoning usage never becomes a reported
zero. Window headroom is distinct from the native hard input admission limit.
These deterministic fixtures establish ordering, latency hiding and bounded
accounting. They do not calibrate a production trigger optimum or billed cost.

The current stack must rerun this scenario against the shared Auxiliary model
policy and native permission-reminder activation. Measurements from the earlier
implementation are not evidence for this protocol generation.

## Verification gates

Use Rust 1.98.1, the locked provider emulator and the repository FULL profile.
Build all native targets before running the uninterrupted executable suites.
Keep Cargo artifact discovery and rebuilds outside real process-test execution.
Use one active source workspace per Cargo target directory so type metadata
cannot be confused with another branch's contract.

Required gates include strict Clippy, formatting, all applicable Linux/macOS
native suites, test-lane coverage, generated App Server v80 protocol/editor
schemas, all TypeScript projects, provider tests, complete clients, i18n,
source provenance/notices, production bundles and real-browser acceptance.
Unfinished or unexecuted gates are not passing results.

Local verification uses the WSL Linux toolchain and the repository's locked
provider emulator. Supported-host document isolation, macOS native boundaries
and the complete fixed-image browser suite are validated by the FULL CI jobs.
Their results must be attached to the exact pull-request head.

No paid live-model summary-quality, latency or billed-cost evaluation has been
run. The opt-in [semantic corpus](../tests/fixtures/compaction-quality/README.md)
remains separate from deterministic execution and persistence verification.
