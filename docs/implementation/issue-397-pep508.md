# Issue 397: PEP 508 validation for managed Python tools

## Decision and suitability audit

Base audit SHA: `5df40f29c72ee5179864bf2f4671c3f8d32dbc18`.

rustX uses `pep508_rs` `0.9.2`, with default features disabled (the crate
has none) and `non-pep508-extensions` enabled. That feature is required only
to retain the existing accepted relative local-reference forms when the parser
is supplied the package root; it is not permission to add a requirements-file
language. `thiserror` `2.0.19` is the direct error-derive dependency.

The selected parser exposes `Requirement::<VerbatimUrl>::parse(input,
working_dir)`, whose `name: PackageName` is the normalized PyPA distribution
identity used by rustX. Its primary source documents `PackageName` as
lowercasing and collapsing `-`, `_`, and `.` runs to `-`, and links that
behavior to the PyPA name-normalization specification. `pep508_rs` is
maintained in its public repository at
<https://github.com/konstin/pep508_rs>; crate metadata and source are at
<https://docs.rs/pep508_rs/0.9.2> and
<https://crates.io/crates/pep508_rs/0.9.2>.

`pep508_rs` is dual licensed `Apache-2.0 OR BSD-2-Clause`. It declares no
MSRV, but its 2021 edition and resolved dependency graph compile under
rustX's declared Rust `1.92` MSRV. The narrow lockfile delta is
`pep508_rs 0.9.2`, `pep440_rs 0.7.3`, `boxcar 0.2.14`, `itertools 0.13.0`,
`version-ranges 0.1.3`, and their `thiserror 1.0.69` derive dependency;
`thiserror 2.0.19` was already transitively locked and is now direct.

The finite corpus in `src/tools/python.rs` covers names, extras, specifiers,
markers, HTTPS URLs with fragments, VCS URLs, file URLs, and relative local
references; malformed names/extras/specifiers/markers/URLs fail. A locked
debug build of the binaries is the reproducible build-cost observation; no
claim of a binary-size improvement is made because this parser is adopted for
correct grammar and identity, not optimization.

## Requirements-file contract

`requirements.txt` is UTF-8 only. It has at most one declaration per physical
line. Empty files and blank lines are accepted. A comment is a `#` at line
start or after whitespace outside quoted marker text; a URL fragment such as
`#sha256=...` is preserved. CRLF is accepted. The accepted effective text is
trimmed only at the file boundary and otherwise passed unchanged, in order, to
the generated `pyproject.toml`.

The file is deliberately not pip syntax. Lines beginning with `-` reject pip
options, includes and editable directives (including `-r`, `--requirement`,
`-e`, `--editable`, and `--index-url`). Backslash continuation and `${...}`
environment-variable expansion are rejected. There are no recursive includes,
marker evaluation, index policy, resolver, or source-management layer.

PEP 508 direct HTTPS and VCS references, `file:` references, and relative
local references remain accepted declaration forms. Parser acceptance changes
neither filesystem authority nor installation behavior: frozen source bytes
are still copied and uv alone resolves, locks, and materializes the
environment. A previous hand-written name-prefix check accepted malformed
declarations; rejection of those non-PEP-508 lines is intentional.

## Managed FastMCP and identity preservation

Every effective declaration is parsed before rustX checks its normalized
`PackageName`. `FastMCP`, extras, version constraints, direct URLs, and
marker-false declarations therefore all identify `fastmcp` and are rejected.
`fast_mcp` normalizes to `fast-mcp`, which is distinct and accepted. This is
applied through the same production discovery owner for User and Workspace
resources.

The AST is transient validation only. rustX does not serialize it, retain it
in runtime state, expose it on the wire, or use it to resolve or evaluate
markers. The original accepted effective declaration text remains in
`PythonToolPackage.requirements`; raw `requirements.txt` bytes remain in the
frozen source digest and package fingerprint unchanged. Existing frozen
manifest validation and corrupt-published-state fail-closed behavior are
unchanged.

## Test mapping

| Matrix | Coverage |
| --- | --- |
| PEP-01 | `pep508_requirements_corpus_preserves_effective_declarations` |
| PEP-02 | `pep508_requirements_reject_invalid_grammar_and_file_directives` |
| PEP-03 | `pep508_requirements_reject_invalid_grammar_and_file_directives` |
| PEP-04 | `requirements_file_comments_crlf_and_quoted_markers_are_bounded` and `requirements_parse_normalizes_comments_and_blank_lines` |
| PEP-05 | `managed_fastmcp_uses_pep503_normalized_identity_without_marker_evaluation`; `managed_fastmcp_policy_applies_to_user_and_workspace_discovery` |
| PEP-06 | corpus preservation assertion, existing `the_generated_pyproject_pins_the_probed_series_and_the_managed_fastmcp`, and fingerprint tests |
| PEP-07 | existing `read_prepared_state` tamper/fail-closed tests |
| PEP-08 | parser unit tests are synchronous and do not invoke a runner, Python, uv, network, or preparation |
| PEP-09 | User/Workspace discovery integration plus existing fake-backend Python preparation tests |
| PEP-10 | parser corpus and error classification tests; `PythonToolError` retains its three typed variants through `thiserror` |
