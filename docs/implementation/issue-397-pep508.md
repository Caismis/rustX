# Issue 397: PEP 508 validation for managed Python tools

## Corrected parser decision

The first review implementation used `pep508_rs` `0.9.2` with
`non-pep508-extensions`. Its minimal reproduction rejected the
current-standard declaration `demo; python_version === '3.12'`. `0.9.2`
remains the newest published `pep508_rs` release, so it cannot be rustX's
grammar boundary.

The only Rust-1.92-compatible `uv-pep508` release (`0.0.40`) has the same
runtime incompatibility; later releases require newer Rust. rustX therefore
uses `pep-508` `0.5.0` for grammar and `uv-normalize` `0.0.40` for the
library-owned PyPA package identity. The executable production-wrapper corpus
parses `demo; python_version === '3.12'` with `pep-508`; it also parses direct
HTTPS, VCS, file, and non-existent relative references without a network
request or path traversal. `pep-508` is MPL-2.0. `uv-normalize` is dual
licensed MIT OR Apache-2.0 and supplies the validated normalized name. Both
compile in the locked Rust 1.92 build. Parser values remain inside
`parse_requirements`; no AST or third-party type leaks into rustX APIs.

`pep-508` is maintained at <https://github.com/figsoda/pep-508>. Its public
`parse` API returns a dependency AST or Chumsky span/found-token errors; it
does not declare an MSRV, so the locked Rust-1.92 build is the compatibility
evidence. `uv-normalize` is maintained in Astral's uv repository and declares
Rust `1.92.0`. Its public `PackageName::from_str` API is used only inside this
validation boundary.
Although extracted uv components do not promise a broad stable Rust API,
that coupling is contained: replacing it changes only this local name check,
never `PythonToolPackage`, runtime state, protocol DTOs, or persistence. It
does not invoke uv resolution or runtime behavior.

The replacement removes `pep508_rs`, `pep440_rs`, `itertools 0.13`,
`version-ranges 0.1`, and `thiserror 1`; it adds `pep-508`, its pinned
`chumsky` parser runtime, and `uv-normalize`. This is a bounded delta chosen
for current grammar correctness, normalized identity, and relative-reference
support. The locked Rust-1.92 build is the MSRV evidence.

## Requirements-file boundary

The file layer owns UTF-8, one physical declaration per line, comments, CRLF,
blank lines, option/directive rejection, continuation rejection, and exactly
the unsupported pip expansion syntax `${NAME}`. It does not classify `$NAME`:
that is not pip expansion and remains parser-owned text. Thus quoted marker
`'$TOKEN'`, another quoted `$`, and URL dollar signs are passed unchanged to
the dependency parser; `${TOKEN}` is rejected before parsing. A bare `$TOKEN`
in a URL is likewise not an invented file-layer directive.

Comments are a `#` at line start or after whitespace outside quoted marker
text. Quoted `#`, quoted `$`, and URL fragments such as `#sha256=...` are
preserved. Lines beginning with `-` (including `-r`, `--requirement`, `-e`,
`--editable`, and `--index-url`) and backslash continuation are rejected.
PEP 508 owns all remaining grammar: direct HTTPS and VCS references, file and
relative references, markers, extras, names, and specifiers. There is no
fallback parser, marker evaluation, index policy, network check, or resolver.

## Diagnostics and ownership

`pep-508` returns structured Chumsky span/token errors. It does not expose
public grammar-production labels, so rustX never formats its error value or the
authored declaration. Instead, after the parser rejects the declaration, the
boundary selects a coarse, fixed rustX-owned class: invalid extra, version
specifier, environment marker, or direct-reference URL. The maximum parser
reason is the documented `MAX_REQUIREMENTS_PARSE_REASON_BYTES` bound (28
bytes); the `line N: reason` parse result is therefore at most 36 bytes for
the tested one-digit line case. The classification is diagnostic-only and
cannot change parsing or acceptance.
This prevents disclosure of URL credentials, tokens, long input, and source
excerpts while retaining a useful error class.

Every effective declaration is parsed once before rustX compares the parser's
normalized name to `fastmcp`. Consequently `FastMCP`, extras, constraints,
direct references, and marker-false `fastmcp` declarations all reject;
`fast_mcp` normalizes to distinct `fast-mcp` and remains accepted. The AST is
validation-only. Original effective declaration text and order remain in
`PythonToolPackage.requirements`; raw `requirements.txt` bytes remain part of
the source digest and fingerprint. uv still owns resolution, locking, and
installation; prepared-state publication and validation are unchanged.

## Test mapping

| Matrix | Deterministic coverage |
| --- | --- |
| PEP-01 | `pep508_requirements_corpus_preserves_effective_declarations` includes `===`, normal comparison operators, `in`, and `not in` |
| PEP-02 | `pep508_parser_diagnostics_are_useful_bounded_and_safe` covers malformed extra/specifier/marker/URL, line location, useful reasons, secret non-disclosure, and long-input bounds |
| PEP-03 | `pep508_requirements_reject_invalid_grammar_and_file_directives` rejects `${TOKEN}`; `requirements_file_comments_crlf_and_quoted_markers_are_bounded` accepts literal `$TOKEN` |
| PEP-04 | `requirements_file_comments_crlf_and_quoted_markers_are_bounded` covers CRLF, comments, fragments, quoted `#`, and quoted `$` |
| PEP-05 | `managed_fastmcp_uses_pep503_normalized_identity_without_marker_evaluation`; `managed_fastmcp_policy_applies_to_user_and_workspace_discovery` |
| PEP-06 | `pep508_requirements_corpus_preserves_effective_declarations`, `the_generated_pyproject_pins_the_probed_series_and_the_managed_fastmcp`, and `the_fingerprint_is_stable_and_tracks_every_material_input` |
| PEP-07 | existing `read_prepared_state` tamper/fail-closed tests |
| PEP-08 | synchronous parser tests invoke no runner, Python, uv, network, or preparation |
| PEP-09 | User/Workspace discovery integration plus existing fake/supervised preparation regressions |
| PEP-10 | one `pep-508` grammar owner, no fallback or AST leakage, bounded safe diagnostics, and unchanged `PythonToolError` variants |
