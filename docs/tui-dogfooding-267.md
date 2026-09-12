# TUI dogfooding presentation contract (#267)

Rust remains the semantic owner. This change needs no protocol or native edits.
The native implementation and tests from #238 still govern admitted-attempt
freezing, effective/desired approval reconciliation, model selection, and resync.

## Tool cards

The transcript selects `background.tool` independently of lifecycle. The common
card renders native status/glyph/duration/exit/certainty, while specialized
adapters receive only `{ content }` for result formatting. Arguments, output,
client disclosure, and runtime truncation keep their existing bounds and identity.
Folded cards and split continuations use the same neutral surface.

Removed: `cardBackground`, `toolPendingBg`, `toolSuccessBg`, `toolErrorBg`, and
state-specific background roles. Added: neutral `toolBg` / `background.tool`.
Content accents now use `toolSubject`, `diffAdded`, and `diffRemoved` roles rather
than raw styles in adapters. Output uses primary text; supporting text and the
shared red accent have sufficient contrast on the neutral surface. The contrast
regression decodes the actual emitted truecolor and 256-color SGR values and
requires at least 4.5:1 for title, output, metadata, and lifecycle accents.

## Approval interaction

| Input/state | Local presentation | Native mutations |
| --- | --- | --- |
| `/approval` | Open picker, Policy highlighted | 0 |
| Up/down | Move highlight; current checkmark still follows effective mode | 0 |
| Esc before submission | Close focused surface; never cancel Agent attempt | 0 |
| Esc after submission | Close UI only; submitted operation continues | 0 additional |
| Select Policy | Latch request pending | Exactly 1 `approvalModeSet("policy")` |
| Select Full access | Open confirmation with Cancel focused | 0 |
| Cancel confirmation | Close | 0 |
| Explicitly enable | Latch request pending | Exactly 1 `approvalModeSet("full_access")` |
| Repeated keys / reopen during request | Suppress submission for the same current owner | 0 additional |
| Snapshot replacement | Discard stale overlay; reconstruct native facts | 0 |

Only the single typed request commits intent. No optimistic effective mode is
stored. Three lifetimes remain distinct:

```text
picker/overlay state != submitted native request state != native ApprovalMode state
```

Each submitted operation has a unique object token carrying its submitting
`PresentationLease` (attachment identity and presentation epoch). Only a pending
request with a still-current owner blocks admission. Presentation invalidation
closes stale overlays and invalidates leases; it does not erase submitted native
operations. A new owner can submit while the old operation remains unresolved.
Completion clears the stored pending request only by exact object identity, so
A's late `finally` cannot clear B's token, including when the attachment object
is retained across a Session ownership change.

Popup redraw/close checks overlay identity. Success/error feedback instead checks
the submitting presentation owner: Esc after submission closes the popup but a
still-current owner receives the native acceptance or bounded failure. A replaced
owner's late result/error cannot repaint the new owner. An older response cannot
supersede a newer approval revision. Acceptance feedback describes the accepted
request; the footer always renders live native facts.

| Native attempt evidence | Effective label | Published pending label |
| --- | --- | --- |
| No attempt | `Effective: Policy` | `Next attempt: Full access`, only if published |
| Admitted or running | `Current attempt: Policy` | `Next attempt: Full access`, only if published |
| Settled historical attempt | `Effective: Policy` | `Next attempt: Full access`, only if published |

A pending field alone never proves current work. The picker reports the published
transition even without an active attempt rather than inventing one. The active
check reuses the native-phase-derived `isAttemptActive()` predicate.

Full access bypasses ordinary approval prompts for already-admitted Tools,
including command execution or file-changing operations when those Tools are
available. It does not grant unavailable Tools/capabilities, answer Questionnaire
or Workflow Review, or define a filesystem/network sandbox profile. The native
active attempt stays frozen: effective Policy plus pending Full access displays
current Policy and next-attempt Full access separately. While idle, effective mode
is labelled `Effective`, not `Current attempt`.

## Footer retention

| Fact | Presentation / priority |
| --- | --- |
| Current/frozen model | First; complete identity, with `attempt` when distinct |
| Effective approval / real pending transition | Essential; current first, `next attempt` only when published |
| Different next effective model | Essential, explicitly `next` |
| Disconnected/degraded, draining, read-only | Exceptional; child retains `Esc parent` |
| Configured disagreement | Secondary diagnostic; `/settings` retains full detail |
| Published context usage/window | Higher than optional Session/token detail |
| Optional capability degradation | Secondary exception |
| Human Session name | Only a nonempty published name |
| Token usage | Lowest priority, only when published |

The footer prefers one row. It drops whole optional segments before essential
facts use the second row. `FooterView` asks for current facts and width on every
render, so terminal-only resize also reruns selection. Width uses Pi's ANSI-aware
terminal-cell measurement. A physically unrepresentable model is explicitly
deferred to `model: /settings`, never shortened into another identity. At extreme
widths, only whole segments that fit can be retained; no layout can show arbitrary
native identities in two arbitrarily narrow rows.

Removed from steady state: raw Conversation/SessionNode IDs, redundant provider,
missing-value stringification, healthy `online`, idle `ready`, permanent shortcut
hints, queued/background/HITL/working duplication. Focused `/session`, `/debug`,
and `/settings` retain diagnostic identities. `workingStatus()` alone presents
compaction, thinking/streaming, Tool execution, and human-input waits. Rendering
performs no cwd, filesystem, Git, provider, Tool, or Session-state discovery.

## Deterministic regression map

| File | Coverage / representative test |
| --- | --- |
| `tui/test/tool-card.test.ts` | `keeps Bash and Read result bodies identical across every native settlement`; actual SGR contrast audit; existing native-outcome, duration, expansion, truncation, and argument/result-budget tests |
| `tui/test/tool-correlation.test.ts` | `every native lifecycle uses the same neutral transcript surface, including split continuation`; existing identity, canonical chronology, progression and snapshot equivalence |
| `tui/test/transcript.test.ts`, `tui/test/workflow*.test.ts` | Existing canonical rendering and Workflow detail regressions |
| `tui/test/commands.test.ts` | `opens approval selection without a native mutation and rejects raw arguments` |
| `tui/test/approval-selector.test.ts` | Deferred-response exact counts; Policy, confirmation safe default/cancel/enable, repeated keys, current/pending markers, snapshot replacement, constrained scrolling |
| `tui/test/app.test.ts` | `approval overlay routes one typed request, consumes Esc, and discards stale snapshot surfaces`; includes reopening during pending request and typed Policy submission |
| `tui/test/status.test.ts` | Stable/transient separation, absent fields, frozen/next model truth, native usage evidence, deterministic dropping and read-only navigation; `footer view recomputes segment selection on resize without a native event` |
| `tui/test/reconstruction.test.ts` | Native snapshot reconstruction; rendering effect tripwires for filesystem/process/Git/network and zero control requests; content-only specialized renderer boundary |

New input tests use explicit Kitty Esc and event-loop continuations, not the
third-party bare-Esc parser's timed disambiguation window. Native response ordering
uses deferred promises. No sleeps prove semantic correctness.

## Validation record (2026-09-12)

Base: `b52b53956f76f50b1864c698a694029a62e43666`, unchanged after final fetch.
Original worktree `/home/caismis/Documents/codes/rustX` remained clean on `main`
at that SHA. Implementation is in sibling `rustX-issue-267` on
`issue-267-tui-dogfooding`; no upstream integration was needed.

The current `.github/workflows/ci.yml` was reread before final validation.

| Command | Final result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `git diff --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins` | Passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features` | Passed: 3,406 tests, 0 failed, 6 existing ignored |
| `uv sync --frozen` in `test-support/fake-provider` | Passed |
| `uv run --frozen pytest` in `test-support/fake-provider` | Passed: 51 |
| `pnpm install --frozen-lockfile` in `tui` | Passed, lockfile unchanged |
| `pnpm typecheck` in `tui` | Passed |
| `pnpm test` in `tui` | Passed: 745 tests, 0 failed, 0 skipped |
| Focused Node test runs for status, Tool card/correlation/transcript, commands, approval selector, app and reconstruction | Passed; all included again in the full run |

Development iterations exposed obsolete footer assertions, incorrect test-fixture
types, and minimal app fixtures that lacked complete presentation state. Those
were corrected to assert the new contract; no failing semantic tests were skipped.
The manual resize smoke found the cached-width bug and led to `FooterView` plus
the terminal-only resize regression.

### Manual real-TUI smoke (PTY, native binary, local provider emulator)

Passed: idle footer; Policy selection; Full access Cancel and explicit enable;
active-attempt approval change at an explicit provider gate (effective Full
access / next Policy, then Policy after settlement); successful Bash and Read;
Bash nonzero exit with stdout `ok`; denied Bash with canonical split continuation;
Tool running/preparing and approval waits; 80-to-40-column resize; retained-session
reopen with reconstructed native context.

The first temporary smoke script omitted Bash's required `execution_mode`; native
validation correctly produced a failed card. The script was corrected and the
successful/nonzero-exit Bash scenarios were rerun. This was smoke-fixture setup,
not a production failure. The temporary scripts and isolated runtime are outside
the repository.

Manual limitation: direct inspection of the retained top-level smoke conversation
failed native attachment with `conversation tool runtime: No such file or directory
(os error 2)`. No successful manual child/inspection navigation is claimed.
Deterministic read-only/parent-navigation and snapshot tests passed. This issue
makes no native inspection changes. PTY output was inspected; this is not a claim
of visual testing across physical terminal emulators or macOS execution.

## PR #268 review correction validation

The review follow-up starts from `8840ab94c89afa15aa632f31bed91339b8ac4cc1`.
It changes approval interaction ownership and labels only. Tool rendering, footer
priorities, native ApprovalMode semantics, and protocol fields are unchanged.

New deterministic app regressions use deferred native replies and the existing
`/new` + `restartRuntime`/state-publication seams. Both actual attachment
replacement and a Session transition retaining the attachment object are covered:

- `success after approval submission and Esc still reports to the current owner
  and settles its token`;
- the equivalent `failure after approval submission and Esc` regression, including
  bounded error rendering and no optimistic projection update;
- `old approval success cannot clear or repaint B's request after attachment
  replacement`, plus its failure equivalent;
- the same success/failure race after `Session ownership changes on the same
  attachment`.

The A/B races prove B submits while A is deferred, A's late success/error installs
no feedback, repeated keys and reopening on B remain deduplicated after A settles,
and B's own completion releases its exact token. Esc never cancels the Agent.

`approval-selector.test.ts` explicitly covers absent/settled/admitted/running
attempts, each with and without a published pending transition. Idle and historical
fixtures use `Effective`; active fixtures use `Current attempt`. A pending field
alone never invents current work. Existing generic fixtures were made explicit
about their native phase instead of just replacing expected strings.

Final local checks for this correction:

| Command | Result |
| --- | --- |
| `node --test tui/test/approval-selector.test.ts tui/test/app.test.ts tui/test/reconstruction.test.ts` | 82 passed, 0 skipped |
| `pnpm install --frozen-lockfile` in `tui` | Passed, lockfile unchanged |
| `pnpm typecheck` in `tui` | Passed |
| `pnpm test` in `tui` | 759 passed, 0 skipped |
| `cargo fmt --all -- --check` | Passed |
| `git diff --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins` | Passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features` | 3,406 passed, 6 existing ignored |
| Emulator `uv sync --frozen` and `uv run --frozen pytest` | Passed; 51 tests |

The current CI workflow was reread. `origin/main` remained
`b52b53956f76f50b1864c698a694029a62e43666`; no integration was required. No new
manual-smoke claim is added by this follow-up; its asynchronous lifetime proof is
the deferred-response regression suite.
