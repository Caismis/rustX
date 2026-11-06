# Frontmatter-only Skill discovery and configuration reload

## Contract and ownership

A Skill is a registered capability definition, not an immutable asset bundle.
The process binds exactly User `~/rustx/.agents/skills` and Workspace
`<workspace>/.agents/skills`. Discovery enumerates their direct candidate entries
once, validates each candidate's `SKILL.md` frontmatter, and preserves deterministic
Workspace-over-User replacement. An invalid Workspace winner never exposes the
lower User definition. Collection/candidate bounds, root containment, canonical
UTF-8 locations, symlinked root rejection and regular/non-symlink SKILL.md checks
remain in the discovery owner.

`skills/frontmatter.rs` opens SKILL.md through the existing anchored directory I/O
owner. It reads a cancellable prefix with 4 KiB bounded read-ahead and stops on the
closing YAML delimiter. Two matching prefix reads verify the definition without
using mtime/size/inode as content equality. No body UTF-8 validation occurs.
The **frontmatter prefix**, including malformed newline-free input, is bounded to
1 MiB. The file, body and assets have no discovery size limit. Missing delimiters,
invalid YAML, field shapes, names, directory mismatches and malformed dependency
metadata retain explicit candidate diagnostics. Internal resource symlinks are
not inspected; actual Native Tools own their ordinary path/permission rules.

`SkillPackage` stores one normalized definition, its dependency manifest, version
and effective binding. It has no resource file list or materialization root.
`SkillSnapshot` publishes immutable catalog metadata, bindings, provenance and
diagnostics. `all`, `exact` and `none` remain visibility choices, never filesystem
ACLs. Bodies and resource files are read or executed only by normal Tools, at the
actual host paths and under their existing authorization rules.

## Version and configuration semantics

`SkillVersionId` is `sha256:<64 lowercase hex>` over the domain
`rustx-skill-definition-v1\n` and stable JSON serialization of normalized validated
frontmatter. Struct field order and sorted metadata keys are deterministic.
Description and compatibility use their validated trimmed values; absent optional
fields/default false are normalized. Every recognized standard field and all
metadata, including dependency declarations, participate. Recognized dependency
metadata uses the existing parsed normalized pins and canonical sorted JSON, so
embedded JSON formatting and equivalent normalized package names stay inert. YAML key order,
comments, quote style and line endings do not. Absolute paths, provenance, body
bytes, scripts, assets, timestamps and filesystem enumeration order do not.

Effective source and actual location remain independent binding facts. Equal
frontmatter at a different effective binding updates that binding. Resource
revision observation and overflow/domain fences reuse **the same** discovery
parser and validation rules, hashing its definitions, candidates, effective
provenance, invalid candidates and necessary diagnostics. They never treat the
Skill collection as a generic recursive resource tree.

Native notifications invalidate observations; they never supply trusted content.
The watcher registers the collection and each direct candidate directory
non-recursively, so atomic SKILL.md replacement and directory recreation remain
observable. Only candidate/collection structure and direct SKILL.md events route
to Skill capture; every direct collection entry, including a hidden entry or an
ordinary file, can change the collection admission budget and invalidates that
observation. Hidden subtrees, resource files and deeper paths do not. Existing bounded merging,
overflow rescan, rearming, physical-directory deduplication, case spelling and
explicit `configuration/reconcile` remain. A rescan still observes definitions
only. An explicitly configured non-Skill input keeps its own owner's semantics.

A body/asset edit or semantically identical YAML produces no configuration
revision, capability preparation or `configuration_applied` fact. Changes outside
the Root/admitted Named Agent/Workflow Skill selection closure do not prepare
Session capabilities. The inert discovery catalog still publishes at the same
fenced safe boundary, so future invocation overrides observe additions, edits,
disabling and removal. This metadata publication shares existing Tool/environment
owners, preserves CapabilityRevision and records no effective configuration fact;
admitted Attempts retain their original catalog. The capability coordinator retains
that published catalog for subsequent live-source recovery. Root dependency
preparation merges only Root-selected
Skills; a child prepares only its frozen selected declarations through the
existing supervised EnvironmentStore. Unselected conflicting declarations cannot
start package managers or fail an unrelated Agent's environment.

Existing capture freshness checks, stale-preparation fences and safe publication
boundaries remain authoritative. An Attempt owns its admitted immutable definition
snapshot until settlement. Configuration Journal differences report selected
Skills as added/changed/removed, retaining normalized definition metadata, source,
actual catalog location and definition version. Resource bytes are not archived
by a configuration fact.

## Local child execution

IPC **v38** carries selected Skill bindings, catalog metadata, dependency
manifests and effective provenance. The child installs those values directly and
retains the actual SKILL.md location and source roots. It never rediscovers Skills,
receives a package file list, copies assets or verifies a full-content digest.
Profile digest **v8** frames the definition, frozen dependencies and live
source/location binding. No compatibility branch remains.

The child's model-visible definition and authorization cannot expand when another
Skill appears or frontmatter changes on disk. Ordinary Read/Bash operations can
observe the current body or script at execution time. Deleted/moved runtime files
fail through the ordinary Tool operation, rather than child initialization.
The environment store uses the ordinary product/Conversation identity, so a
resumed child reuses immutable prepared dependency environments across activations.
Dependency preparation is explicit selected execution work; it does not inspect
Skill bodies/assets. Existing child cancellation, parent-loss, ownership commit
and terminal semantics remain the physical lifecycle authority. The child's
environment backend receives the preparation owner's cancellation signal, and
the guard awaits physical process settlement before reporting preparation failure.

## Structural proof and regression map

| Path | Authoritative implementation | Excluded work and evidence |
| --- | --- | --- |
| Discovery | `skills/package.rs`, `frontmatter.rs`, `identity.rs` | Direct collection enumeration and SKILL.md prefix only. Tests use 10 GiB assets, a 4 GiB body, 4,200 nested files and an internal resource symlink; actual file-open/read counters admit only SKILL.md, with no package directory listing. |
| Revision / rescan | `local_runtime/resource_directory.rs`, `resource_directory/evidence.rs` | The skills entry delegates to discovery before generic traversal. Both complete and overflow observations use the same definition evidence. Revision tests assert only two SKILL.md opens and zero resource-directory listings. |
| Configuration preparation | `configuration/capture.rs`, `prospective.rs`, `capabilities/coordinator.rs` | Captured definitions are passed by value. Selected-only dependency merging; no body/asset inputs. Tests cover inert body edits, unused conflicts, safe Attempt snapshots, stale successors, source replacement and Journal differences. |
| Child initialization | `runtime/subagent/resolver.rs`, `ipc.rs`, `composition/subagent_child.rs` | Metadata/manifest installation, explicit selected environment preparation, no Skill file I/O. `skills/materialization.rs`, recursive package walking and package-content identity APIs are deleted. Composition tests retain original host locations and versions across rewritten definitions/new ambient Skills; no private Skill copy exists. |

Native Tool, lifecycle, automatic watch (atomic replacement/recreation, overflow,
merging and repeated-rescan no-op), source precedence and client protocol suites
remain regression coverage. Tests synchronize through leases/channels/gates; no
sleep is used to establish correctness.

## Measurement method and raw evidence

`examples/skill_discovery_benchmark.rs` creates sparse files and performs discovery
and a second identical rescan in a fresh process. Cases are one 1 KiB asset, one
2 GiB asset plus 256 MiB body, and 32 Skills with 16 MiB assets each. Sparse file
creation is outside measured discovery/rescan. `/proc/self/io` `rchar` measures
actual process read requests, including tiny measurement overhead; it is **not**
storage-device physical I/O. `/usr/bin/time -v` records process CPU and peak RSS.
Run the same debug/all-features executable, host and case in both revisions:

```sh
cargo build --all-features --example skill_discovery_benchmark
for skill_case in small large many; do
  /usr/bin/time -v target/debug/examples/skill_discovery_benchmark "$skill_case"
done
```

Baseline revision: `eae92389c01abb2b214c0222070dabd84f38ba2f`.
Copy the same harness into that isolated baseline checkout and use separate Cargo
target directories for the two revisions.
Raw JSON and time outputs are under [measurements](skill-discovery/measurements).
These are local debug-build observations, not a portable fixed speedup claim.
Structural counters and syscall traces establish zero asset reads independently
of hardware timing. Configuration revision and child-copy boundaries are proved
by their dedicated tests and deleted call paths, rather than extrapolated from
this discovery-only benchmark. See the [validation report](skill-discovery/validation.md) for completed commands,
counts, comparison data and CI status.
