# CFG3 Skill regression contract

The current tests live in `src/skills/package.rs`, `src/skills/catalog.rs`,
`src/local_runtime/launch_tests.rs`, Agent profile resolution tests and runtime
publication tests. They cover both canonical roots, whole-package shadowing,
malformed higher duplicates, bounded ordered diagnostics, exact/all/empty prompt
visibility, absolute root guidance, progressive reads and immutable admitted
versions. Filesystem aliases resolve through the native resource owner.

No Skill source-policy layer or explicit extra-root precedence exists. A Skill
selection changes prompt visibility, never filesystem authorization. See
[configuration](configuration.md) and [Agent profiles](agent-profiles.md).
