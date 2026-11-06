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
| Real-child process conformance | Covered by the final all-feature run below |
| `cargo test --all-features --no-fail-fast` | Pending final serialized run |
| App Server protocol generation and TypeScript contracts | Passed; regenerated files have zero drift |
| Web Console TypeScript and Vitest | Passed: 4,068 tests, 4 skipped |
| Terminal Console TypeScript and Jest | Passed: 1,320 tests, 1 skipped |
| CI controller Python tests | Passed: 64 tests |
| Runtime benchmark controller Python tests | Passed: 37 tests |
| `git diff --check` | Passed |

The new regression coverage includes every standard definition field, canonical
dependency metadata, malformed/unterminated/cancelled prefixes, deterministic
replacement during prefix inspection, large bodies/assets, internal resource
symlinks, source precedence, all/exact/none visibility, direct/atomic/directory
watch changes, overflow/coalescing, unused dependency conflicts, frozen children,
Native Read runtime semantics and physical cancellation settlement. Existing
automatic configuration, lifecycle and Tool authorization suites remain intact.
No sleep establishes the new concurrency assertions.

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
on this branch; no pull request is created, as requested. Implementation revision
and final full-verification status will be recorded after the final local run.
