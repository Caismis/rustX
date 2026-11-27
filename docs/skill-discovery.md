# Frontmatter-only Skill discovery and configuration reload

## Contract and ownership

Skill discovery visits `<workspace>/.agents/skills` and then
`~/rustx/.agents/skills`. The first valid definition for a name wins and aliases
to the same instruction file are deduplicated. Discovery skips hidden entries,
`node_modules`, and entries excluded by `.gitignore`, `.ignore` or `.fdignore`.
It stops traversal at a `SKILL.md`. Standalone Markdown files with Skill
frontmatter are discovered directly in each collection root. Nested directories
are searched for `SKILL.md` packages, not standalone Markdown files.

Directory and file symlinks can point outside the Skill collection. For a
bound Workspace, the Client resolves the link target under the admitted
Workspace and offers its real path to ordinary Read and Bash. It observes
both the authored link and its resolved target. Broken links, external
Workspace targets and directory cycles are skipped. Unbound discovery
retains the authored package path. The model receives metadata and a
Client-visible path; it opens the instruction file and its supporting files
only when needed.

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
ACLs. During agent execution, normal Tools read or execute bodies and resource
files at the Client-visible catalog paths under the existing authorization rules.

## Skill display information

Add an optional `agents/openai.yaml` inside a skill directory to customize its
card in the Web UI.

```yaml
interface:
  display_name: PDF Toolkit
  short_description: Extract and review PDF content
  icon_small: ./assets/pdf.svg
  icon: lucide:file-text
```

Images use paths relative to the skill directory under `assets/`. The card
tries `icon_small`, then `icon_large`, then the RustX `icon` field with a
bundled Iconify Lucide icon. Images can be SVG, PNG, JPEG, or WebP up to 32 KiB.
When display fields are missing, the card uses the skill's name and description
from `SKILL.md` and the default icon.

For skills installed in the global or workspace skill directory, open the card
menu and choose **Edit display information**. **Upload image** previews a local
image in the editor. On save, rustX stores it in the skill's `assets/` directory
and sets `interface.icon_small` to its relative path. Identical uploads reuse
the same asset. Saving preserves other metadata and leaves existing asset
files in place. The skill's identifier, instructions, and runtime description
come from `SKILL.md`.

In the Web UI's Workspace skill list, a directory skill's menu can copy or
move the package between the current Workspace and Global. The operation
includes all authored files and directories, including the presentation
sidecar and assets. A changed destination name updates the `name` in
`SKILL.md`. Copies also set `metadata.rustx-default-disabled: true`, so
automatic selection leaves the copy off until it is explicitly selected.
An unchanged-name move preserves the instruction file verbatim. **Delete**
removes the confirmed directory from its own scope.

Package transfer accepts regular files and directories, with limits of
4,096 entries, 64 nested levels, and 256 MiB of files. Symlinked installs
and standalone Markdown files remain browseable.

## Global User Skills on a Client

The data Hello pins the Client's canonical User root to the binding. Each
subsequent root inquiry must report that same path, including after recovery.

The Host's `~/rustx/.agents/skills` is the User authority. At Client bind,
rustX synchronizes its valid discoverable packages before publishing their Client
paths. Malformed definitions retain their diagnostics; their attachment trees
are not opened or copied. When the Host has no valid User Skills, the Client
collection is not inspected or changed. For a local Unix binding, matching
opened root and collection identities establish sharing in the same kernel;
each package directory and marker is checked separately. A remote binding
requires a fresh root probe and, when the collection is on another filesystem,
a fresh collection probe. A candidate on another filesystem must be copied
unless its identity can be established locally. Standalone root Markdown files
are checked individually using bounded, no-follow metadata inquiries. These
checks permit read-only shared roots and collections where sharing is proved.
Configured HOME and `rustx` aliases resolve to Client-readable physical paths.
Equal path strings alone are insufficient. Discarding a Client never removes
the standing User root.

Separate filesystems use an ownership-marked cache under
`<client-home>/rustx/.agents/skills/.rustx-sync`. An immutable package version
is staged and published as a whole, so an already bound session can continue
using its earlier paths. The Host stores a random, owner-only authority ID in
its standing `~/rustx` root and combines it with the collection's filesystem
identity. A read-only root without that ID uses a process-local identity, so
its cache can be reused only during that Server process. One authority's binds
serialize during synchronization;
live sessions retain leases on their versions. Unleased versions are evicted
when necessary to keep that authority's cache under 4 GiB and 1,024 versions.
The Client admits at most 64 cached authorities.
If live leases exhaust that bound, a new synchronization fails rather than
deleting a live version or personal files. Client cleanup removes only its
private staging directory and releases its leases.

The Client inquiry stream-hashes only the named package's `SKILL.md`, up to
the 512 MiB per-package bound.
A matching digest skips the package without examining attachments. Attachments
can consequently remain stale when `SKILL.md` is unchanged, including when an
attachment subtree has a different mount view; this is an intentional
performance tradeoff. A missing or different digest causes a whole
package copy. Copies stream regular files and directories with a 512 MiB
per-package and 1 GiB per-bind payload cap; executable files remain executable.
When `SKILL.md` is at the collection root, the copy excludes rustX's reserved
cache, cache lock, and identity probe directory.
A collection-level link to a package directory is resolved and pinned; links
inside a package and special files are refused. Neither hardlinks nor writable
bind mounts are used. A User can exclude a
legitimately oversized package by adding its path to the User collection's
`.ignore` file; the same discovery ignore rules then omit it from both sync
and the catalog. Standalone root Markdown Skills that require transfer are
copied as individual files.

File tools deny writes into the official User Skill tree, whether it is the
shared standing root or the synced cache. Bash can still modify a shared
standing root; this residual risk is accepted. The Client cache is not a
second authoring authority. Unknown transfer outcomes are not retried as
ordinary failures. Synchronization uses bounded data records while health,
cancel and revoke use the independent control lane. Each exchange has its own
deadline; a healthy transfer can continue making progress beyond the inquiry
and admission budget.

Losing the data connection releases the synchronization lock and discards
its unpublished stage. Versions already published stay pinned by the Client
while a same-instance connection recovers. A lost publication reply retains
its unknown outcome.

The execution-boundary change and User Skill sync share one release gate:
do not ship the boundary without both this behavior and #612 in place.
On Linux, verify the combined release revision with
`sh test-support/prove_paired_skill_read.sh` in addition to the package's
bound-resource protocol preflight. This uses a real Client with a separate
filesystem view to exercise the published Skill path through Read and verify
file-tool write denial. The broader mount-view acceptance can include the same
check by setting `RUSTX_PAIRED_RELEASE_PROOF=1`. Packaging may run on macOS;
the Linux paired proof remains a release requirement.

## Workspace change hints

A bound Client starts one asynchronous fingerprint observation for its
Workspace. Configuration watches share this sample and can schedule a fresh
one after five seconds. A changed observation requests an exact resource
rescan. The data lane and Session startup continue while the observation runs.

Git workspaces use `git rev-parse --verify --quiet HEAD` and
`git diff-index --quiet` for tracked index and worktree changes. The latter
uses Git's stat information and produces a dirty bit. Both commands share
a 750 ms deadline. Non-Git workspaces and repositories without a first
commit use the root path and root modification time. The initial sample
establishes the baseline. An unavailable sample preserves the last known one.
Native watches continue to detect individual resource edits.

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

The definition version ignores body and asset edits. On a separate Client
filesystem, a changed `SKILL.md` produces a new cache path even when frontmatter
is unchanged. An attachment-only edit can remain cached until `SKILL.md` changes.
Changes outside
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
| Discovery | `skills/package.rs`, `frontmatter.rs`, `identity.rs` | Definition discovery reads instruction-file prefixes only. Bind synchronization separately traverses a package when its instruction digest does not match. |
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
