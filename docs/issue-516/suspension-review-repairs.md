# Outline suspension and transient Session restoration repairs

Repairs for [review 6675579112](https://github.com/Caismis/rustX/pull/517#issuecomment-6675579112)
of `2a52622be26f8693b2c74b89369a48f11f2bc8a5`, checked on 2026-11-06,
Asia/Shanghai. The strict geometry latency gates remain open under
[issue 534](https://github.com/Caismis/rustX/issues/534).

## Outline demand across parked previews

An outline request can start while visible and wait for optional surface
preparation. If that preparation settles while hidden, the viewport returns
`off` without starting hidden outline work. Previously only requests first made
while hidden were deferred, so returning to the preview lost this enabled demand.

The component now retains desired demand until its current request finishes.
It distinguishes a running request from suspended work, so showing the preview
before preparation settles does not start a duplicate request. A current enabled
request returning `off` remains deferred and resumes when visible. Successful,
failed and over-limit results settle the demand. Explicit Off, copy replacement
and retirement invalidate older completions. The viewport API, resource limits
and import deadlines are unchanged.

The new component suite mounts the real `GeometryPreview` and viewport, with
controlled parser, WebGL, controls and Worker boundaries. It checks preparation
settling while hidden, returning before settlement, Off before/after suspension,
an already running outline Worker, late old-copy preparation/Worker completion,
and actual failure/limit results. Before the repair the initial six-case suite
had two failures, including the missing automatic outline Worker on return.

## Unresolved startup selection

With saved membership `[A, B]` and B selected, a transient `operation_failed`
leaves B as a local Session draft after startup admission fails. Closing A changes
membership and reruns persistence. Previously generic draft serialization wrote
New Conversation, discarding the previously committed selected identity.

Persistence now recognizes the existing startup-restore intent associated with
that exact draft and still-resumable membership. The saved value remains only
`{ kind: 'session', sessionId }`; draft text, commands, credentials and native
observations are never serialized. The draft records the existing navigation
intent revision, so a later explicit replacement, including a failed Fork that
leaves the old draft visible, cannot reuse that old startup hint. Automatic
same-claim recovery retains the revision. Native `unknown_session`, released
membership, cancellation and authority retirement retain their existing fences.
The persistence effect also checks the current actor context against its render
snapshot, rather than checking only route object identity.

A real-App regression fails before the repair at persistence after B's transient
failure and closing A. It now preserves B, remounts with a new client, retries
the native read and selects B after admission succeeds. Additional cases cover
explicit Workflows/New Conversation/A selection, failed Fork and a stale failure
after newer navigation. Existing unknown-session, endpoint, cold-draft and
authority tests remain in the validation group.

## Validation

Focused geometry tests: 364 passed across 32 files. Focused navigation/authority
tests: 141 passed across five files.

- Full Web/Host suite: **4,457 passed**, four platform skips; 309 files passed.
- TypeScript, production build, 336 source records, 355 production notices,
  runtime asset/source audits, localization and whitespace checks passed.
- Fixed-production-build browser checks: **three passed**, one optional
  approximately-200-MB STEP case skipped because its external source was absent.
  These exercise actual native delivery/Workbench residency and a full page
  reload restoring the selected Session. Installed Chrome 157 / SwiftShader is
  supplementary evidence, separate from the repository's pinned-browser CI.

The new controlled regressions prove the reported races, not browser transport
failure behavior or the strict 200-ms first-model / below-50-ms maximum-interaction
targets. No screenshot baseline, test timeout or resource limit was changed.
The final pushed revision requires its own CI results.
