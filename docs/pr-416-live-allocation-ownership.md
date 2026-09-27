# PR #416: live pre-Reserved physical ownership

Starting head: `9b2c65d2b7cbd9f76f4579917c66097da961f54a`.
Fetched main/merge base: `ad863a24cf48fbb0d5182746e1046d4b64e8c167`.
This repair keeps Jobs finite, Agents durable, and Workflow children finite.

## Physical allocation ownership

The resume admission owner previously guarded rollback cleanup with the journal
`reserved` boolean. Recovered workspace verification can already have published
physical authority and supervised Git continuations before that boolean becomes
true. Cancellation or another pre-Reserved error could therefore clear `resuming`
and expose Inactive without exact positive physical settlement.

A consumed allocation is now transferred to `recovery_unreserved` and
`recovery_pending`, with its exact child Conversation and activation identity,
before admission can release its reservation. The transfer fences the workspace
under the registry mutex. The admission owner then clears only its own reservation,
drops its executable authority outside the mutex, and invokes the existing recovery
owner. Handle drop is not proof. Availability, Goal idle and drain remain excluded
until that owner acquires exact native proof for the activation and all continuations.
Proof locks remain held through the in-memory settlement cut. No Reserved,
RolledBack, Agent row or activation-history event is invented for this allocation.
The durable namespace retains ordinal consumption across reopen.

The same transfer helper handles consumed initialization errors. Once Reserved has
actually committed, the existing durable rollback path remains in charge; the new
unreserved path does not replace it. Existing admission completion classifies a
still-unproven transfer as Failed, so a captured interrupt cannot report successful
settlement while native proof remains unavailable. Before classification the
admission owner joins the existing bounded recovery owner. A temporarily unavailable
probe therefore does not prematurely fail a captured wait while recovery is still
in progress. Starts share one completion owner: the registry mutex excludes a
second reconciler from publishing completion for the first. The existing receipt
regression parks that owner at its exact claim and verifies that a second start
does not change the completion watch.

`DeletionExclusion::acquire` now obtains and retains native proof for every consumed
allocation in the Session, in addition to conversation-writer exclusions. The
snapshot's existing ownership freeze excludes new allocation while the destructive
owner validates these proofs. A logical journal that lacks Reserved is no longer
sufficient to grant destructive authority over a live physical namespace. Missing,
corrupt or locked published evidence still fails closed.

## Deterministic regressions

`recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop`
continues to run real recovered-workspace Git with a FIFO-held fsmonitor helper.
The non-crash branch now retains a duplicate of the exact parent lease at the
post-verification cleanup boundary. While that duplicate exists, cancellation
must leave Unavailable/pending recovery, reject another message admission, block
Goal idle, and make runtime physical drain/shutdown fail rather than report
quiescence. No admission event or second logical activation may exist. Releasing
the duplicate and joining the existing recovery owner must restore Inactive and
idle, permit a new runtime to drain/shut down, and survive a separate SQLite reopen
with next ordinal 3 and only the original committed activation.

Disabling just the new transfer reproduced the defect deterministically: the
captured interrupt returned successful rollback despite the retained lease. The
repaired source was restored immediately after this diagnostic run.

`pre_reserved_allocation_blocks_destructive_deletion_until_exact_proof` publishes
an actual allocation without Reserved. A retained duplicate excludes destructive
Session authority. Explicitly unlocking that test-owned open-file description
releases the kernel exclusion even if another concurrent spawn transiently
inherited a duplicate; destructive acquisition must then itself obtain exact
proof. No journal admission is fabricated. This separately tests the destructive
owner; the registry regression above tests runtime shutdown.

The full focused subagent suite exposed the same immediate-reacquisition assumption
in `initial_uncommitted_physical_authority_consumes_activation_identity_on_restart`
that earlier repair rounds found in resume tests. It now joins existing recovery
reconciliation rather than assuming drop implies the next Try probe succeeds.
All negative physical-ownership and consumed-ordinal assertions remain.

## macOS Bash cancellation: unresolved original failure

[Run 36317341024](https://github.com/Caismis/rustX/actions/runs/36317341024) failed
only the macOS platform-sensitive job. Its external tools suite had 129 passes
and one failure: the background cancellation result was Cancelled but the shell's
TERM marker was missing. The hosted log contains no signal trace or shell exit
status for that failure. Linux cannot reconstruct those missing observations.

The audited path is: Job cancellation commits intent and cancels the executor's
signal; `ProcessSupervisor::settle` sends one `MSG_TERMINATE`; the inner's
`handle_frames` calls `killpg(SIGTERM)` and starts the existing two-second grace;
the inner's sole `waitpid` owner publishes shell exit, while `observe_inner_group`
uses WNOWAIT. On Darwin, after no inner children remain, outer fallback containment
signals the retained group before canonical terminal publication. The runtime
requires terminal supervision and direct-child reaping before returning an outcome.
A Cancelled intent by itself is not terminal proof, and is not proof that the
shell ran a trap. Signal-attempt reporting is written before the native call;
the old log cannot establish its actual syscall result or per-process delivery.

Apple's [group signal implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sig.c)
and [group iteration](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_proc.c)
signal a captured set of members individually. Its
[wait implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_exit.c)
counts live matching children before returning WNOHANG, so no evidence was found
that ordinary live-child WNOHANG should be interpreted as ECHILD. The
[Bash manual](https://www.gnu.org/software/bash/manual/html_node/Signals.html)
states that a foreground command can defer a pending trap. These facts identify
possible signal-order questions; they do not establish which occurred in this run.

The original marker assertion and command are preserved, with the terminal result
added to its failure diagnostics. A portable real-supervisor regression,
`cancellation_waits_for_pipe_gated_term_trap_before_physical_terminal`, handshakes
trap readiness and trap entry through FIFOs, holds the TERM trap until explicit
release, asserts the result remains pending, and requires emitted TERM first and
Cancelled only after release/physical settlement. No production supervision change,
longer grace, weaker marker assertion or platform skip is justified by the available
evidence. The original macOS failure is **not claimed resolved**. Emission, delivery,
early escalation and the exact missing-marker cause require a macOS trace; the
added positive regression does not substitute for that diagnosis.

A broader Linux run separately exposed an empty PID-file read in
`anchor_observed_alive_is_never_consumed_before_its_terminal_observation`.
The fixture treated file creation as publication of its complete PID payload.
Both possible producers (the existing anchor PID test seam and shell descendant
PID fixture) now write privately and atomically rename the completed file. This
repairs fixture readiness without a new delay or retry. It does not explain the
macOS TERM marker failure and does not change ordinary supervisor signaling.

## Identity and generated protocol

`AgentId` now documents durable Agent identity across activations; `SubagentId`
documents one finite Agent activation or finite Workflow child execution. Native
resumes retain AgentId and consume distinct SubagentIds. The canonical generator
regenerates v27 schema/TypeScript documentation; `generate_schemas` also updates
the configuration schema that embeds AgentId documentation. Wire shapes, version and fixtures
remain unchanged. `docs/jobs-and-agents.md` now names canonical App Server v27.

## Validation

Validation used Linux, Rust 1.98.1 and `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`.
Test concurrency remains the repository default. macOS is unavailable locally;
its required hosted validation remains unverified. No CI configuration changed.

The final source passed:

| Command (repository root unless noted) | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| `cargo build --bins` | Pass |
| `cargo test --lib --all-features subagent` | 337 passed |
| `cargo test --lib --all-features deletion_tests` | 62 passed |
| `cargo test --lib --all-features tools::native::bash::tests` | 37 passed |
| `cargo test --lib --all-features anchor_reaping_tests` | 4 passed |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,192 passed; 3 existing ignores; 194 boundaries run separately |
| `cargo test --test contracts --test provider --all-features` | 28 contracts and 166 provider tests passed; 5 existing credential-dependent ignores |
| `cargo test --lib --all-features -- boundary_suites::` | 194 passed |
| `cargo test --lib --bins --all-features -- --skip scripted_suites:: --skip local_runtime::session_runtime_manager::tests::` | Exact hosted unit command on Linux: 2,638 passed, 2 existing ignores, 749 filtered |
| `cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | Full rerun passed: catalog 26, managed-output 5, conformance 22, durable 129, process 62, subagent 45, tools 130 |
| `git diff --check`, `git diff --cached --check` | Pass |
| `pnpm generate`, `pnpm check`, `pnpm typecheck` in `protocol/app-server` | Pass |
| `cargo run --example generate_schemas` | Pass |
| `pnpm typecheck`, `pnpm test` in `tui` | Pass; 893 tests |
| `pnpm typecheck`, `pnpm test` in `web-console` | Pass; 1,094 tests |
| `pnpm check:i18n`, `pnpm check:provenance`, `pnpm build` in `web-console` | Pass |
| `pnpm typecheck`, `pnpm test` in `dev` | Pass; 37 tests |
| `uv run --frozen pytest` in `test-support/fake-provider` | 51 passed |
| `CONTAINER_ENGINE=podman pnpm test:e2e` in `web-console` | 126 passed against final built binaries |

The three existing broad-unit ignores are startup/stage instrumentation and the
fixture-regeneration test. No new skips/ignores were added. Generated JSON after
removing descriptions and TypeScript after removing comments match their prior
semantic shapes. The obsolete one-shot identity description is absent.

Focused commands also passed during development (and their tests passed again in
the final full suites):

```sh
cargo test --lib --all-features runtime::subagent::registry::tests::recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop -- --exact --nocapture
cargo test --lib --all-features pre_reserved_allocation_blocks_destructive_deletion_until_exact_proof -- --nocapture
cargo test --lib --all-features cancellation_waits_for_pipe_gated_term_trap_before_physical_terminal -- --nocapture
cargo test --all-features --test tools bash::bash_background_cancellation_uses_the_same_process_group_path -- --exact --nocapture
cargo test --lib --all-features agent411_resume_reservation_recovery_requires_rollback_containment_proof -- --nocapture
```

### Failed and superseded development runs

- An initial short filter combined with `--exact` selected zero tests; the full
  qualified name was rerun. The zero-test run is not counted as evidence.
- A binary build exposed a missing `#[cfg(test)]` on the test-hook initializer;
  corrected before the final build.
- The initial subagent group had 336 passes/one failure from the old immediate
  proof-reacquisition assumption; changed to join recovery ownership as above.
- Deliberately disabling the transfer made the retained-lock regression fail at
  its captured interrupt assertion. Source was immediately restored.
- The first broad unit run had 3,190 passes/two failures: configuration schema
  drift and the empty PID-file publication race. Canonical regeneration and atomic
  fixture publication repaired them.
- The next broad run had 3,191 passes/one failure: captured admission completion
  was classified before transient recovery could converge. Joining the shared
  recovery owner fixed the production cut without changing the test's assertion.
- A subsequent strict Clippy run rejected a 103-line test. Extracting its semantic
  completion-watch assertion fixed the warning without suppression.
- The first final external run passed catalog 26, managed-output 5, conformance
  22, durable 129, process 62 and subagent 45. Tools had 127 passes/three failures:
  two explicit PyPI DNS download failures during dependency preparation and one
  source-preparation-unavailable error whose nested cause was not printed. DNS
  resolution was then verified and the entire external command rerun successfully.
  The third failure remains an opaque source-preparation error in the first log;
  its exact nested cause is not claimed to have been recovered.
- An earlier client/browser run passed (including 126 browser cases), but source
  changed afterwards. Final client and browser runs replace that evidence.

No arbitrary timeout increase, marker-assertion weakening, test serialization,
CI modification or retry-until-green argument was used. Existing bounded runtime
reconciliation policy is unchanged; exact native proof, not its deadline, permits
settlement. The new TERM test is positive coverage, not a reproduction or repair
of the original macOS failure.
