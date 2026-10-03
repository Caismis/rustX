# PR #446 native supervision and current screenshot repair

## Starting evidence

Starting remote PR HEAD: `78d6ecfb5bd0f1ac3f96a4f9cb9941c8ebf6c474`.
Starting main/merge base: `7d08560a50840d71b465adc09b384c61fc744b0a`.
The clean issue worktree was ahead 5 / behind 0 and GitHub MERGEABLE.

Run [37091527536](https://github.com/Caismis/rustX/actions/runs/37091527536)
failed exactly two lanes. Complete logs and Web actual/diff/trace were downloaded
and inspected; the other six lanes passed. The macOS failure is job
[111112707850](https://github.com/Caismis/rustX/actions/runs/37091527536/job/111112707850),
and Web is job
[111112707863](https://github.com/Caismis/rustX/actions/runs/37091527536/job/111112707863).
Earlier similar evidence in issue-430-validation.md is history, not grounds to
exclude either failure from this repair.

## Native invariant and repair

An owned group is never declared terminal without physical proof. A stopped
rustX inner supervisor remains the exact direct-shell reaping owner; a stop
neither cancels work nor proves terminality. Resume that retained owner so the
existing cancellation/control channel can proceed. Abnormal owner loss still
requires fallback containment and the existing physical-proof gates.

The old outer transition was:

```
Running -> exact stop observation -> SIGKILL(inner)
-> retained terminal anchor -> fallback group SIGKILL
-> group wait ECHILD -> GroupChildrenReaped
-> Darwin absence probe Err -> ControlFailure -> ContainmentFailed
-> deliberately nonterminal
```

The failed hosted trace reaches that last branch without GroupAbsenceProven,
TerminalPublished, TerminalObserved or DirectChildReaped. It does not record the
raw absence-probe errno, so it cannot distinguish timeout from another probe
error. The confirmed ownership defect is killing the recoverable direct-shell
reaper. Darwin has no child subreaper; its outer cannot reclaim the shell's
reaping ownership, unlike Linux. This unnecessarily makes recovery depend on
orphan reclamation. No internal launchd timing mechanism is claimed as measured.

The outer now performs one exact-PID SIGCONT attempt while the anchor is retained
and still Running. Diagnostic AnchorResumeAttempt replaces AnchorUnwedgeKillAttempt.
It does not send semantic cancellation or declare terminality. The inner parses
the user's existing TERMINATE frame, sends TERM, and reaps its direct shell.
The outer's existing abnormal-exit containment remains intact; on Darwin even
an inner child-domain ECHILD still takes that conservative path. The subsequent
whole-group ESRCH proof remains mandatory.

The absence probe's single-observation decision was factored into a pure helper
without changing its deadline or polling behavior. Only ESRCH proves absence.
Signal success, EPERM, ECHILD, hard errors and deadline expiration never substitute
for that proof. ControlFailure still records failure intent without advancing the
runner's lifecycle to Terminal. No numeric group signal is added after anchor
release, and no timeout/grace/retry/skip policy was changed.

### Deterministic native evidence

The stopped-anchor test uses an actual Bash builtin blocked on a FIFO, so this
specific regression has no orphan descendant requiring launchd reclamation.
The existing after-stop socket gate establishes exact native stop observation.
User cancellation then wins; the existing before-TERM socket gate is reached only
by the resumed inner after parsing that cancellation. While parked there, the
invocation remains incomplete and no group signal or terminal publication exists.
After release, the test requires the inner's shell reap before anchor terminal
observation, group proof before terminal publication, and direct supervisor reap.
It asserts exactly one stop observation, resume, cancellation receipt, shell-exit
publication, terminal publication/observation and direct-child reap, with no group
signal after GroupChildrenReaped and no control failure/grace escalation.

Negative control against the old real supervisor binary failed at the unchanged
20-second deadlock guard: the owner never reached the pre-TERM gate. The trace
showed SIGKILL, abnormal anchor exit and fallback containment instead. After the
source repair the same gated test passes. This is a contract repair, not a rerun
of the old macOS race until it happens to pass.

A new pure absence regression verifies present/EPERM before expiry remain
unproven, present/EPERM at expiry fail, ECHILD/error never proves absence, and only
ESRCH succeeds. Existing owner-loss, terminal-frame, abandonment, pre-anchor,
EPERM and TERM/KILL tests remain unchanged and are included in focused/full runs.

## Web failure and measurement

Reference: `mobile-expanded-dark-linux.png`, 390×844.
Hosted actual differs at 24 sites, all x=303, maximum RGBA channel delta 49.
The actual/diff and trace show the expanded 280px mobile Sidebar, 110px main
column, Open Workspace header, ContextSeat and clipped wrapped transcript.
No live allowance existed for this reference.

Authority:
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`.

The fixed fresh-context cohort copies current shell.spec's exact navigation:
classified shell, Inspector open/close, Sidebar collapse/expand, search, Settings,
dark theme, 390×844 viewport, then explicitly expanded Sidebar. It uses the same
fixed date and reduced motion and every preceding screenshot's normal capture
cadence. captureStable is unchanged and no reference guides sample acceptance.
Each sample must independently reach two consecutive RGBA-identical captures.
After stabilization, two identical canonical product fingerprints are recorded.

The fingerprint includes complete root DOM/text/classes/attributes, every element's
computed CSS and before/after pseudo CSS, exact floating-point element and text
Range rectangles, scroll offsets/extents, focused element, HTML/theme state,
viewport/visual viewport/device scale, Sidebar state, and the existing
ChatViewport owner's following/reading/navigation/detached/historical state.
Reading that test-fixture owner is passive and introduces no production hook.

Two preliminary diagnostic cohorts are not acceptance evidence: the first sampled
before stabilization and detected the expected follow-scroll adjustment (0→42);
the second inserted expensive fingerprint reads between captures. The accepted
cohort keeps original capture cadence and observes state after stabilization.
Neither diagnostic cohort consulted references to retry toward a match.

### Accepted current-state results

All 30 fresh contexts passed, outer investigation command exit 0. Every sample
needed exactly two byte-identical captures; no sample was selected against the
reference. All 60 post-stabilization canonical fingerprints were identical.
The measured source state is PR HEAD `78d6ecfb` on main `7d08560a`; no Web product
source or reference was changed during this investigation.

SHA256 values below hash decoded RGBA bytes, not PNG compression:

| Stable rasterization | Fresh contexts | RGBA SHA256 |
|---|---:|---|

| Combined variant | 4 | `f23070483875d43834555c8fae6848a8f874293b463bd4e47ec2007f37c378b6` |
| Checked-in reference | 19 | `81b1552ab35f6c5ace10c2d2a12e0bc79cf23bc3382c21fc2cd40165ce9b9ec0` |
| Rounded-card variant | 6 | `f1dc128c4037bc992ee4813e80e1b23b8c30e7e80976150157ae932b29e574c5` |
| Hosted glyph-edge variant | 1 | `c35466f1b179ca625beca2eac7202a454b38789a0265cced9a6d2d12c8c2db6e` |

The hosted actual hash is `c35466f1b179ca625beca2eac7202a454b38789a0265cced9a6d2d12c8c2db6e`,
independently reproduced exactly in this cohort. The product fingerprint hash is
`d5603307d358d55e50b40430d9db4672d5fcb6ec8b92bc44f78e022b816c0393`.

Key identical geometry: Sidebar `[0,0,280,844]`, main `[280,0,110,844]`,
Session header `[280,0,110,203]`, transcript viewport `[280,203,110,327]`,
ContextSeat `[280,536,110,76]`, Composer seat `[280,530,110,314]`.
Transcript scrollTop is 42, scrollLeft is 0, scrollHeight is 369; follow is true,
reading/navigation are null, detached/historical false, viewport 390×844, DPR 1.
The canonical fingerprint also includes every descendant and exact text-line
rectangle, including the Open Workspace controls.

Decision: measured rasterization variance with identical observable product state
and geometry, not an observed layout/scroll defect. This does not establish a
Chromium-internal cause. The baseline remains unchanged. The policy adds only
46 disjoint 1×1 sites: the 24 hosted glyph sites (max delta 49) and 22 independently
measured rounded-card sites (max delta 2). Each site's changed-pixel budget is one
and its channel bound is its own measured delta. No gaps are admitted.

### Exact current-reference evidence

Each row is one 1×1 region, budget one. Alpha is 255 in every before/after tuple.
Full RGBA tuples are also stored in the live manifest.

| x | y | Reference RGB | Stable variant RGB | Maximum channel delta |
|---:|---:|---|---|---:|
| 303 | 226 | 21,28,58 | 21,21,23 | 35 |
| 303 | 227 | 21,21,30 | 21,21,23 | 7 |
| 303 | 237 | 21,21,23 | 21,21,30 | 7 |
| 303 | 238 | 21,21,23 | 21,21,30 | 7 |
| 303 | 279 | 21,21,30 | 21,21,23 | 7 |
| 303 | 280 | 21,21,30 | 21,21,23 | 7 |
| 303 | 311 | 21,21,23 | 21,21,30 | 7 |
| 303 | 312 | 21,21,23 | 21,21,30 | 7 |
| 303 | 333 | 21,21,23 | 21,21,30 | 7 |
| 303 | 334 | 21,21,23 | 21,21,30 | 7 |
| 303 | 353 | 21,21,30 | 21,21,23 | 7 |
| 303 | 354 | 21,21,30 | 21,21,23 | 7 |
| 303 | 375 | 21,21,30 | 21,21,23 | 7 |
| 303 | 376 | 21,21,30 | 21,21,23 | 7 |
| 303 | 381 | 21,21,23 | 21,21,30 | 7 |
| 303 | 382 | 21,21,23 | 21,21,30 | 7 |
| 303 | 402 | 21,21,23 | 21,35,72 | 49 |
| 303 | 423 | 21,21,30 | 21,21,23 | 7 |
| 303 | 424 | 21,21,30 | 21,21,23 | 7 |
| 303 | 432 | 21,21,23 | 21,21,30 | 7 |
| 303 | 433 | 21,21,23 | 21,35,72 | 49 |
| 303 | 444 | 21,35,72 | 21,21,23 | 49 |
| 303 | 474 | 21,21,30 | 21,21,23 | 7 |
| 303 | 475 | 21,35,72 | 21,21,23 | 49 |
| 310 | 617 | 25,25,27 | 24,24,26 | 1 |
| 311 | 617 | 27,27,29 | 28,28,30 | 1 |
| 312 | 617 | 29,29,31 | 31,31,32 | 2 |
| 313 | 617 | 31,31,33 | 32,32,34 | 1 |
| 356 | 617 | 31,31,33 | 32,32,34 | 1 |
| 357 | 617 | 29,29,31 | 31,31,32 | 2 |
| 358 | 617 | 27,27,29 | 28,28,30 | 1 |
| 306 | 618 | 25,25,27 | 27,27,29 | 2 |
| 307 | 618 | 34,34,35 | 34,34,36 | 1 |
| 308 | 618 | 41,41,43 | 40,40,42 | 1 |
| 311 | 618 | 43,43,46 | 44,44,45 | 1 |
| 312 | 618 | 44,44,45 | 43,43,45 | 1 |
| 313 | 618 | 43,43,45 | 44,44,46 | 1 |
| 356 | 618 | 43,43,45 | 44,44,46 | 1 |
| 358 | 618 | 43,43,46 | 44,44,45 | 1 |
| 361 | 618 | 41,41,43 | 40,40,42 | 1 |
| 362 | 618 | 34,34,36 | 33,33,35 | 1 |
| 363 | 618 | 25,25,27 | 27,27,29 | 2 |
| 305 | 619 | 43,43,44 | 42,42,45 | 1 |
| 364 | 619 | 42,42,43 | 42,42,45 | 2 |
| 365 | 619 | 31,31,32 | 30,30,32 | 1 |
| 304 | 620 | 44,44,46 | 44,44,45 | 1 |

### Comparator boundaries

Current-main frozen comparator evidence and live-reference binding remain intact.
The new tests verify the exact baseline, hosted glyph-only, card-only and combined
RGBA hashes and acceptance. Each measured variant fails without policy and under
a different reference name. Every approved region is 1×1 with budget one; every
site's delta+1 fails. Both x=302 and x=304 neighbors fail for every glyph row, as
does every unmeasured y gap at x=303, another transcript content pixel, a one-pixel
layout shift, width/height changes and a broad low-amplitude theme shift.

No running-draft live allowance was restored. No PNG reference, product CSS,
captureStable code, global tolerance, retry or screenshot deadline changed.

## Local validation

All commands below exited 0 on Linux x86_64, unless explicitly identified as the
old-policy negative control above. No existing ignored test was added or changed.

| Validation | Result |
|---|---|
| stopped_anchor_supervisor_is_contained_by_the_outer | 1 passed |
| stopped_observation_preserves_stop_and_terminal_status_for_owner | 1 passed |
| tools::native::bash focused module | 51 passed |
| runtime::supervised_unit | 18 passed |
| runtime::process_runner | 12 passed |
| runtime::interactive_process | 21 passed |
| runtime::process_wait | 1 passed |
| cargo fmt --all -- --check | passed |
| cargo clippy --all-targets --all-features --locked -- -D warnings | passed |
| cargo build --bins --all-features --locked | passed |
| TMPDIR=/var/tmp/rustx-issue-432-ci-repair RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked | 4,091 passed, 0 failed, 8 existing ignored; 19 targets |
| screenshot comparator/stability | 42 passed |
| desktop Host/UI, submission policy, preferences, cancellation/Escape, Composer, ContextSeat, context, residency and incremental presentation | 197 passed, 11 files |
| Web full unit suite | 1,485 passed, 80 files |
| Web typecheck / i18n / provenance / build | passed; 148 source records, 132 production package notices |
| Dev typecheck / tests | passed; 38 tests |
| Protocol v33 generation check / typecheck | passed; generated output unchanged |
| TUI typecheck / required-provider-emulator tests | passed; 895 tests |

The repair changes no App Server protocol, desktop production behavior, Composer
policy or ContextSeat ownership. Current v33 readers remain; Windows desktop is
still unsupported. No current running-draft allowance or PNG reference changes.
No real Finder/Terminal GUI smoke is claimed. macOS physical settlement requires
fresh hosted validation of this source, not an inference from Linux success.

Full pinned browser acceptance:
`CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` completed with **163
passed, outer exit 0**. It ran in a separate process session so owned Vite cleanup
could complete normally; no SIGTERM was accepted as success. No screenshot update
mode, test retries or skipped failure. The repaired shell reference, strict
running-draft light/dark references, native compaction and desktop cases all pass.
`git diff --check` passes. Fresh hosted results are reported on the PR for the
new repair HEAD; old-head checks are not reused as evidence.
