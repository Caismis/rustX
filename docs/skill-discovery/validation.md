# Frontmatter-only Skill discovery: implementation and validation

## 1. Architecture and previous behavior

Skills are now capability definitions registered by normalized YAML frontmatter.
Discovery enumerates only the fixed User and Workspace collections' direct
candidates and reads verified bounded SKILL.md prefixes. Bodies and assets remain
ordinary runtime files. Previously, discovery recursively listed packages and
hashed every accepted file; source revision detection could repeat that work,
and child initialization copied and verified the complete package.

The authoritative contract and four-path structural proof are in
[Skill discovery](../skill-discovery.md). The local DSH source was reviewed for its
separation of catalog registration and progressive instruction/resource loading;
no new discovery roots, dependency packages or configuration modes were added.

## 2. Core modules

- `skills/frontmatter.rs`: cancellable, anchored, regular-file prefix I/O, 4 KiB
  read-ahead, a 1 MiB frontmatter bound and two matching prefix reads.
- `skills/package.rs`, `identity.rs`, `catalog.rs`: a single normalized definition,
  deterministic definition version and metadata-only publication.
- `resource_directory.rs` and `resource_directory/evidence.rs`: shared discovery
  evidence replaces Skill traversal in complete revisions and overflow fences.
- `session_runtime_manager/source_watch.rs`: nonrecursive collection/candidate
  watches, exact entry filtering, bounded invalidation and recovery.
- `configuration/{capture,prospective}.rs`, composition and capability coordinator:
  selected definition comparison and selected-only dependency preparation.
- `runtime/subagent/{resolver,ipc}.rs` and `composition/subagent_child.rs`: frozen
  catalog/dependencies/source binding, direct host locations and supervised
  preparation; IPC v38 and execution profile digest v8.
- `runtime/configuration_commit.rs`: selected definition versions, normalized
  metadata, actual locations and provenance in precise configuration differences.

## 3. Removed full-package paths

`skills/materialization.rs`, recursive `walk_package_files`, complete package
hashing, Skill file lists, child source-copy/remap fields and the unused anchored
copy helper were deleted. There is no legacy full-package mode or migration.

| Path | Structural evidence | Deterministic/real-process evidence |
| --- | --- | --- |
| Discovery | Direct candidates → frontmatter → normalized definition hash; no recursive package walker exists. | Actual anchored file-open/read hooks allow only SKILL.md; 10 GiB assets, a 4 GiB body and 4,200 nested files do not add asset opens or directory enumeration. |
| Revision/rescan | The Skill entry dispatches to the same discovery parser before generic traversal; overflow evidence uses the same owner. | Revision tests assert zero package directory listings and only the two prefix opens. Body/assets/formatting retain equality; semantic frontmatter changes do not. |
| Configuration preparation | Captured immutable definitions flow by value; dependency merging consumes selected manifests. | Unselected conflicting dependencies do not prepare environments; leases/gates prove old Attempt snapshots and stale-successor rejection; Journal tests distinguish body edits and definition changes. |
| Child initialization | Frozen metadata and original paths install directly; no resource list, copy or full-content verifier remains. | Frozen composition tests survive new ambient Skills and rewritten frontmatter. A real child with a 2 GiB sparse asset is held at the provider gate while its private runtime tree is checked for absent Skill copies, then settles normally. |

## 4. SkillVersionId

`sha256:<64 lowercase hex>` hashes `rustx-skill-definition-v1\n` plus stable JSON
of the normalized validated frontmatter. All recognized standard fields and
metadata participate. Python/Node dependency metadata is normalized through the
existing parser and sorted canonical pins. YAML key order, comments and unrelated
formatting do not change the version. Paths, provenance, bodies, scripts, assets,
file sizes and timestamps do not participate. Effective source/location bindings
are compared separately, including bindings exposed through Named Agents.

## 5. Child execution and runtime resources

The parent freezes selected identities, versions, catalog metadata, dependency
manifests and effective sources/locations. The child does not rediscover Skills or
expand its catalog from ambient filesystem changes. It uses ordinary authorized
Read/Bash operations on the actual host paths, observing the bytes present when
the Tool runs. A missing file fails that Tool operation; it does not invalidate a
frozen definition or force an eager child scan.

Selected declared dependencies use the existing supervised EnvironmentStore under
the stable product/Conversation identity, including child resumption. The child
preparation owner supplies cancellation to the environment backend. Cancellation
or parent loss awaits physical process settlement before preparation can finish.
The ordinary coordinator's shared environment-build ownership remains independent
of an individual waiting candidate.

## 6. Validation

Completed commands and final counts are recorded below. Cargo build, lint,
generation and tests are serialized: tests that spawn their own executable must
not run while another Cargo invocation replaces that executable. Final Rust
validation uses independent `CARGO_TARGET_DIR=/tmp/rustx-skill-validation-target`,
because another worktree symlinks the workspace target directory and can replace
the top-level child binary with an older IPC implementation. Dependency artifacts
were reused; this change's crate artifacts and binaries were rebuilt independently.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --all-features --bins --examples` | Passed |
| Focused all-feature Skill/revision/watcher/coordinator/child/automatic-commit tests | 217 checks passed; the two real-Workflow checks were rerun with matching isolated binaries |
| Real-child process conformance | Passed: all 45 Subagent integration tests, including the 2 GiB no-copy provider-gate check |
| `cargo test --all-features --no-fail-fast` | Passed: 5,965 tests, 0 failures, 20 existing ignored tests |
| macOS FULL native suites | Passed: 4,148 in-crate tests and 558 external tests; 12 existing ignored tests and the Linux-owned lane filters are checked by CI |
| App Server protocol generation and TypeScript contracts | Passed; regenerated files have zero drift |
| Web Console TypeScript and Vitest | Passed after the Office launcher follow-up: 4,069 tests, 4 skipped |
| Selected rendered browser regressions | Passed: 82 tests with strict pixel comparison; original deadlines and cold-start counters retained |
| Workflow editor and Harness/native detail parity | Passed after renderer follow-up: 17 tests, including native 256-node startup, semantic port edits and all 8 paired detail presentations |
| Office conversion and launcher lifecycle | Passed: 17 focused tests, including actual descendant cgroups and deterministic exit-before-pipe-close settlement |
| Office sidebar rendered regressions | Passed: 8 cases covering continuous pages, first-page readiness, localized retry and desktop/mobile layouts |
| Response-loss recovery | Passed: 3 consecutive local repetitions with the original assertions, followed by the final FULL run's complete 302-case first browser shard |
| Deletion-confirmation screenshot | Passed: 3 consecutive focused local repetitions with the original exact pixel comparison |
| Resource revision regressions after macOS fixture correction | Passed: 8 focused Linux tests and the case-sensitive/case-insensitive coverage in FULL CI on Linux/macOS |
| Terminal Console TypeScript and Jest | Passed: 1,320 tests, 1 skipped |
| CI controller Python tests | Passed: 64 tests |
| Runtime benchmark controller Python tests | Passed: 37 tests |
| FULL CI and browser coverage | Passed: all 15 selected jobs, 655 browser cases, complete inventory coverage; one unchanged second-shard rerun described below |
| Workspace Host platform conformance | Passed on Linux and macOS |
| `git diff --check` | Passed |

The final full run on `9120fd5c410fd3a398201c0b5f289c50cd345caf` contains
5,277 passing library/in-crate tests, 679 passing
external integration tests and 9 passing doctests. Per-target counts, elapsed
times and the exact command are retained in
[the test summary](measurements/validation-tests.json).

The new regression coverage includes every standard definition field, canonical
dependency metadata, malformed/unterminated/cancelled prefixes, deterministic
replacement during prefix inspection, large bodies/assets, internal resource
symlinks, source precedence, all/exact/none visibility, direct/atomic/directory
watch changes, overflow/coalescing, unused dependency conflicts, frozen children,
Native Read runtime semantics and physical cancellation settlement. Existing
automatic configuration, lifecycle and Tool authorization suites remain intact.
No sleep establishes the new concurrency assertions.

### Rendered startup QA

The flow under test is: connect or restore saved views → choose the intended
Session or New Conversation Workspace → type, submit, reconnect or refresh →
retain the intended draft and exact native admission. Browser plugin not
available; the repository Playwright workflow uses its pinned Linux x86_64
Chromium container, with host fixtures at `http://127.0.0.1:5273` and
`http://127.0.0.1:5274`. Desktop 1440px and mobile 390px, light/dark and English/
Chinese states were exercised.

| Check | Result and evidence |
| --- | --- |
| Page identity / meaningful screen | Passed: fixture titles, route attributes, editor and navigation assertions. |
| Framework overlay | Passed: existing Vite-overlay absence assertions. |
| Console health | Passed: workspace/default and reconnect cases retain empty relevant page-error/console-error lists. |
| Screenshots | Passed: 82-case normal run uses exact pixel comparison after reviewing the intended silent-loading differences; no noise masks were added. |
| Interaction proof | Passed: native cold attach, first Send, reconnect, default Workspace, unsent manual choice, reload and fresh-page restoration. |

Initial CI exposed a startup ordering race: automatic default-Workspace selection
could prepare an unrelated draft before saved-view restoration, especially while
Connection Settings was open. `App.tsx` now lets saved-view restoration finish
first. Cold navigation fixtures explicitly restore their intended source view;
the original zero/one/six-peer assertions remain unchanged. The existing default-
Workspace tests still prove automatic Home selection and unsent choice persistence.

Reviewed references remove the earlier temporary welcome and “Queued · updating”
cards. A single confirmed queued item now shows its native row/actions, rather
than an optimistic duplicate collapsed into a pending count. The narrow footer
geometry assertion was updated for that confirmed row; x positions and font
metrics remain exact. 31 affected image references were regenerated with the
existing stable-capture helper, then checked by the separate normal run.

The 256-node Workflow capacity test exposed duplicate initial measurements:
every mounted node forced a complete internals update, although React Flow's
ResizeObserver already batches initial measurement. The renderer now explicitly
updates node internals only after semantic definition edits. The native compiler
and layout engine are unchanged. Under the original 10-second budgets, local
render time changed from 8,849.4 ms to 975.1 ms; organize layout measured 919.1 ms
before and 935.2 ms after. These are single-run observations from the same pinned
browser and host, retained in [the raw startup comparison](measurements/render-startup-before-after.json).
The full 17-case follow-up includes field/port editing and connection behavior.

Harness/native detail screenshots now park the pointer at the same idle location
before each capture. This removes differing copy-toolbar hover states caused by
the two ledger rows opening the same panel at different positions. Exact pixel
comparison and its empty noise policy remain unchanged; failures now attach both
images for inspection.

Rendered QA covers pinned Chromium on Linux; native macOS behavior is separately
covered by FULL CI. The Browser plugin remains optional for future in-app visual
inspection.

### Office launcher settlement follow-up

The third FULL run exposed an existing Office launcher-loss race: a nonzero
`systemd-run` exit recorded failure but waited for `close` before retiring its
cgroup. Descendants can retain inherited pipes, so this deferred cleanup until
the external deadline and could replace the intended failure with an unavailable
error. Nonzero exit now enters the existing stop/retirement path immediately;
completion still awaits physical settlement. The new event/gate test fails on the
old code because no kill starts before pipe close, and proves the corrected path
cannot settle before the retirement gate releases. All 17 focused conversion and
real-cgroup lifecycle checks pass, including cancellation, external deadline,
launcher loss, empty descendant cgroups and a successful next operation. No
boundary assertion or timeout was relaxed.

The third FULL run also had one navigation timeout in the response-loss recovery
case: its screenshot retained the new MCP editor while the test waited for a
list entry. Three consecutive focused repetitions passed, and the unchanged case
passed in the final 302-case first shard. No MCP implementation or recovery
assertion was changed for that isolated failure.

The final run's second browser shard initially passed 352 cases and failed one
confirmation screenshot: 61 pixels behind the backdrop blur differed by at most
one channel level. The foreground dialog matched. The same case passed three
focused local repetitions, and the preceding FULL run had passed that shard.
References, pixel policies, timeouts and product code remain unchanged for the
manual rerun of the complete second shard on the same implementation commit.
That rerun passed all 353 cases. Combined with the unchanged first shard's 302
cases, the coverage gate verified all 655 discovered cases against the same
commit and prepared inputs.

## 7. Measurements and reproduction

Baseline: `eae92389c01abb2b214c0222070dabd84f38ba2f`. Both revisions use the same
`examples/skill_discovery_benchmark.rs` harness, local host and debug/all-features
build. Each case runs in a fresh process, creates sparse files outside the timed
sections, then measures first discovery and an identical second discovery
(rescan). This is **discovery-component startup**, not end-to-end Session startup.
`/proc/self/io` rchar is actual process read-request bytes with tiny measurement
overhead, **not physical device I/O**. CPU and peak RSS cover the process,
including fixture setup/cleanup, from `/usr/bin/time -v`. These single-run debug
observations are not a statistical latency distribution or portable speedup.
To reproduce the baseline, copy this unchanged benchmark harness into an isolated
checkout of the baseline revision; the harness is new, while the measured library
comes from that revision. Give each checkout its own Cargo target directory so
another worktree cannot replace its executables.

| Case | Revision | First discovery (ms) | Rescan (ms) | rchar delta (bytes) | CPU user + system (s) | Peak RSS (KiB) |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 Skill, 1 KiB asset / 1 KiB body | Before | 0.587 | 0.326 | 8,514 | 0.00 + 0.00 | 3,840 |
| Same | After | 0.245 | 0.115 | 4,418 | 0.00 + 0.00 | 4,096 |
| 1 Skill, 2 GiB asset / 256 MiB body | Before | 72,564.455 | 72,454.799 | 9,663,676,738 | 143.39 + 1.67 | 3,840 |
| Same | After | 0.271 | 0.121 | 16,482 | 0.00 + 0.00 | 4,096 |
| 32 Skills, each 16 MiB asset / 1 KiB body | Before | 16,209.125 | 16,200.694 | 2,147,622,079 | 32.08 + 0.35 | 3,328 |
| Same | After | 1.590 | 1.378 | 138,431 | 0.00 + 0.00 | 3,584 |

Raw [JSON/time results](measurements) retain the measurements without rounding.
The [large-case syscall evidence](measurements/rustx-skill-after-large-syscalls.json)
contains all relevant asset calls and SKILL.md open/read lines: **zero asset read
opens**, four header opens and four 4,096-byte reads across two discovery passes.
The single asset open is O_WRONLY fixture creation. The bounded read-ahead can
include a few body bytes; it never grows with body or asset length. Timing alone
is not used to infer the absence of scans.

```sh
cargo build --all-features --example skill_discovery_benchmark
for skill_case in small large many; do
  /usr/bin/time -v target/debug/examples/skill_discovery_benchmark "$skill_case"
done
strace -f -yy -e trace=openat,getdents64,read \
  -o /tmp/skill-discovery.strace \
  target/debug/examples/skill_discovery_benchmark large
```

## 8. Limits and risks

- Internal IPC v38 and profile digest v8 deliberately replace the old execution
  contract. Rebuild the native binaries and restart local runtimes together;
  old child executables and frozen execution profiles are not migrated.
- Runtime resource bytes remain mutable and are not archived by configuration
  events. Strict execution reproducibility requires a separate snapshot owner.
- The 1 MiB prefix bound rejects excessively large or unterminated frontmatter;
  SKILL.md bodies and asset files have no discovery size limit.
- Dependency installation is real selected execution work and can still require
  network access and package-manager time; this refactor removes implicit asset
  work, not explicit environment installation.
- Native watcher delivery is platform-dependent. Existing bounded rescan,
  rearming and explicit reconcile remain recovery paths, all definition-only.
- Local syscall measurements are Linux-specific. Cross-platform lifecycle,
  filesystem and client behavior is covered by the repository's FULL CI profile.
- Managed Python and other resource families retain their separate complete-source
  semantics; no unrelated source contract was weakened.

## 9. Delivery and CI

Branch: `codex/ui-experience`. The implementation is committed and pushed directly
on this branch; no pull request is created, as requested.

Implementation commit:
[`fe3aa5ba5f1476789ed6b81754c316d0128f2ac5`](https://github.com/Caismis/rustX/commit/fe3aa5ba5f1476789ed6b81754c316d0128f2ac5).
The local full suite and follow-up local checks pass. The initial
[FULL CI run](https://github.com/Caismis/rustX/actions/runs/42203691573) passed
Linux contracts/boundaries, protocol, TUI, Web/Host/launcher and macOS desktop
adapter checks. It exposed the obsolete macOS raw-byte revision fixture and the
rendered regressions described above. The case-insensitive fixture now compares
valid semantic frontmatter edits and proves body edits remain inert.

The follow-up commit contains that fixture correction, startup restoration
ordering, reviewed silent-loading image references and this validation evidence.
Follow-up implementation revision:
[`9120fd5c410fd3a398201c0b5f289c50cd345caf`](https://github.com/Caismis/rustX/commit/9120fd5c410fd3a398201c0b5f289c50cd345caf).
The complete local Rust suite was rerun successfully on that revision. The
[second FULL CI run](https://github.com/Caismis/rustX/actions/runs/42211680619)
passed Linux, protocol, client, Host and macOS full checks, but exposed the two
browser issues described above. The renderer/hover follow-up is committed as
[`50ffb31c22018171702c58c10e3f7bbbc28f5b98`](https://github.com/Caismis/rustX/commit/50ffb31c22018171702c58c10e3f7bbbc28f5b98).
The [third FULL CI run](https://github.com/Caismis/rustX/actions/runs/42218559090)
exposed the Office launcher-loss issue described above. The final implementation,
including that fix and its deterministic regression test, is
[`28bb6d4fd63f86ff2ec61a0e0947f6ac6edfa7a5`](https://github.com/Caismis/rustX/commit/28bb6d4fd63f86ff2ec61a0e0947f6ac6edfa7a5).
No Rust implementation changed after the fully tested `9120fd5c4` revision.

The [final FULL CI run](https://github.com/Caismis/rustX/actions/runs/42221459145)
**passed on attempt 2** for that exact implementation revision. All 15 selected
jobs succeeded, including Merge verification and complete 655-case browser
coverage. The three skipped jobs are alternative FAST-profile lanes, replaced
by the selected FULL suites. [Workspace Host platform conformance](https://github.com/Caismis/rustX/actions/runs/42221431550)
also passed on both Linux and macOS.

[Raw CI evidence](measurements/validation-ci.json) records both run identities,
the implementation SHA, job conclusions, platform counts and the unchanged
manual shard rerun. The concluding delivery commit only adds documentation and
validation evidence; the verified implementation remains `28bb6d4fd`.
