# Skill definition regression contract

See [frontmatter-only discovery](skill-discovery.md) for the authoritative
architecture, four-path I/O proof, measurement procedure and current limitations.

Coverage lives in `src/skills/tests.rs`, `package.rs`, `catalog.rs`,
`local_runtime/resource_directory.rs`, the native source-watch tests, capability
coordinator lease/commit tests, real child composition tests and
`tests/scripted/app_server/configuration/{automatic_commit,mcp_readiness}.rs`.

The suite exercises canonical User/Workspace roots, definition precedence,
invalid higher duplicates, bounded malformed prefixes and diagnostics, normalized
frontmatter versions, metadata/dependencies, body/script/asset non-inputs, exact/
all/none selection, live child locations, immutable admitted definitions, source
replacement, atomic writes, directory recovery, notification overflow/coalescing,
unselected dependency isolation and precise Journal change facts. Large resources
are sparse and deterministic I/O counters prove assets are neither opened nor
recursively listed. Native Tool tests preserve ordinary authorization/progressive
read behavior. Concurrency tests use leases and gates, never elapsed sleep.

No extra roots, compatibility mode, full-package identity or ordinary child
package materializer exists. Skill visibility never changes Tool filesystem
permission. Managed Python packages retain their separate complete-source contract.
