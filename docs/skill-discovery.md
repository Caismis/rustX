# Frontmatter-only Skill discovery and configuration reload

## Contract and ownership

Skill discovery visits `<workspace>/.agents/skills` and then
`~/rustx/.agents/skills`. The first valid definition for a name wins and aliases
to the same instruction file are deduplicated. Discovery skips hidden entries,
`node_modules`, and entries excluded by `.gitignore`, `.ignore` or `.fdignore`.
It stops traversal at a `SKILL.md`. Standalone Markdown files with Skill
frontmatter are discovered directly in each collection root. Nested directories
are searched for `SKILL.md` packages, not standalone Markdown files.

Directory and file symlinks can point outside either collection. Discovered
paths retain links below the collection root so package-relative references
resolve beside the offered instruction file. Broken links and directory cycles
are skipped. Package bodies and supporting files are opened on demand by the
tools that use them.

`skills/frontmatter.rs` reads up to 2 MiB of frontmatter with 4 KiB cancellable
read-ahead and stops at the closing YAML delimiter. Two matching prefix reads verify the
definition using content equality. Discovery requires parseable YAML and a
nonempty string description. It uses the parent directory name when the
frontmatter supplies no string name. Nonstandard names and descriptions over
1024 characters produce warnings. Optional fields, including generic
`metadata`, do not gate discovery and do not install anything.

`SkillPackage` stores one normalized definition, its version and effective
binding. It has no dependency manifest, resource file list, or
materialization root. `rustx.python-dependencies` and
`rustx.node-dependencies`, when present inside otherwise valid metadata, are
uninterpreted text.
`SkillSnapshot` publishes immutable catalog metadata, bindings, provenance and
diagnostics. `all`, `exact` and `none` remain visibility choices, never filesystem
ACLs. Bodies and resource files are read or executed only by normal Tools, at the
actual host paths and under their existing authorization rules.

## Version and configuration semantics

`SkillVersionId` is `sha256:<64 lowercase hex>` over the domain
`rustx-skill-definition-v1\n` and stable JSON serialization of normalized validated
frontmatter. Struct field order and sorted metadata keys are deterministic.
Absent optional fields and default false are normalized. Recognized standard
fields and metadata participate as preserved values. Dependency-looking
metadata is not parsed, normalized, or solved. YAML key order, comments,
quote style and line endings do not. Absolute paths, provenance, body
bytes, scripts, assets, timestamps and filesystem enumeration order do not.

Effective source and actual location remain independent binding facts. Equal
frontmatter at a different effective binding updates that binding. Resource
revision observation and overflow/domain fences reuse **the same** discovery
parser and validation rules, hashing its definitions, candidates, effective
provenance, invalid candidates and necessary diagnostics. They never treat the
Skill collection as a generic recursive resource tree.

Native notifications invalidate observations. The watcher registers collections,
grouping directories and package directories non-recursively. Atomic instruction
replacement, ignore-file edits and collection recreation are observed without
walking package assets. Configuration capture compares the same discovered
definitions on both sides of the capture fence for both collections. Changes
to linked instruction files and linked collection directories are also observed.
Every direct entry in a scanned collection or grouping directory participates
in its admission budget, including hidden entries, so their creation/removal
also invalidates source discovery. Hidden subtrees remain unobserved and package
assets remain outside configuration scanning.

A body/asset edit or semantically identical YAML produces no configuration
revision, capability preparation or `configuration_applied` fact. Changes outside
the Root/admitted Named Agent/Workflow Skill selection closure do not prepare
Session capabilities. The inert discovery catalog still publishes at the same
fenced safe boundary, so future invocation overrides observe additions, edits,
disabling and removal. This metadata publication shares existing Tool/environment
owners, preserves CapabilityRevision and records no effective configuration fact;
admitted Attempts retain their original catalog. The capability coordinator retains
that published catalog for subsequent live-source recovery. Selecting, hiding,
or removing Skills does not change the effective tool environment and does
not start an interpreter or package manager.

Existing capture freshness checks, stale-preparation fences and safe publication
boundaries remain authoritative. An Attempt owns its admitted immutable definition
snapshot until settlement. Configuration Journal differences report selected
Skills as added/changed/removed, retaining normalized definition metadata, source,
actual catalog location and definition version. Resource bytes are not archived
by a configuration fact.

## Local child execution

IPC **v39** carries selected Skill bindings, catalog metadata and effective
provenance. The child installs those values directly and retains the actual
instruction file location. It never rediscovers Skills, receives a package
file list, copies assets, verifies a full-content digest, or materializes a
dependency environment. Profile digest **v9** frames the definition and the
live source/location binding. No compatibility branch remains.

The child's model-visible definition and authorization cannot expand when another
Skill appears or frontmatter changes on disk. Ordinary Read/Bash operations can
observe the current body or script at execution time. Deleted/moved runtime files
fail through the ordinary Tool operation, rather than child initialization.
Existing child cancellation, parent-loss, ownership commit and terminal
semantics remain the physical lifecycle authority. Managed Python keeps its
own package store and supervised uv lifecycle; that owner is not a Skill
environment.

## Structural proof and regression map

| Path | Authoritative implementation | Excluded work and evidence |
| --- | --- | --- |
| Discovery | `skills/package.rs`, `frontmatter.rs`, `identity.rs` | Collection traversal and instruction-file prefix only. Tests use 10 GiB assets, a 4 GiB body, 4,200 nested files and an internal resource symlink; actual file-open/read counters admit only SKILL.md, with no package directory listing. |
| Revision / rescan | `local_runtime/resource_directory.rs`, `resource_directory/evidence.rs` | Skill discovery owns its revision separately from generic resource traversal. Revision tests assert only two SKILL.md opens and zero resource-directory listings. |
| Configuration preparation | `configuration/capture.rs`, `prospective.rs`, `capabilities/coordinator.rs` | Captured definitions are passed by value. No dependency merge and no environment writes. Tests cover inert body edits, conflicting metadata, safe Attempt snapshots, stale successors, source replacement and Journal differences. |
| Child initialization | `runtime/subagent/resolver.rs`, `ipc.rs`, `composition/subagent_child.rs` | Frozen metadata and location only. IPC v39 and profile digest v9 carry no dependency manifest. Composition tests retain original host locations across rewritten definitions; no private Skill copy or Skill environment exists. |

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

## Pi parity and removal map

Reference pin: `earendil-works/pi` `473515f9c297c75cf2c3ebb3ba027049ed836d27`
(`packages/coding-agent/src/core/skills.ts`, `resource-loader.ts`,
`docs/skills.md`, and regression `2781-skill-collision-precedence`).

Pi is the feature boundary for instruction discovery, not a product to copy.
rustX keeps its own User and Workspace directories and does not adopt Pi's
package distribution, `~/.pi` layout, slash commands, or plugin host.

| Pi behavior | rustX |
| --- | --- |
| Discover `SKILL.md` and publish a compact name/description/location catalog | Preserved. Bodies and assets stay on demand. |
| Project sources win over user sources; the same real file is one Skill | Preserved as Workspace over User, with same-file deduplication. |
| Malformed YAML or a missing description skips that Skill and leaves the others usable | Preserved. Name and description-length warnings do not hide a valid definition. |
| `disable-model-invocation` hides a Skill from the model catalog | Preserved. |
| Standard `metadata`, `compatibility`, and `allowed-tools` are descriptive | Preserved, including uninterpreted `rustx.python-dependencies` and `rustx.node-dependencies`. They grant no install and no permission. |
| No Skill-owned virtualenv, `PATH` prefix, or package-manager probe | Removed from rustX. `skills/dependencies.rs`, `skills/environments.rs`, `ToolEnvironmentOverlay`, and the Skill fields on snapshots, IPC, and profile digests are deleted. |
| Managed executable packages keep their own environment | Preserved on Managed Python (`src/tools/python.rs`), which is not a Skill. |

The following Pi product surfaces are intentionally absent: slash commands,
skill installers, package registries, and a second watcher. Native refresh
and configuration publication remain the only application path.
