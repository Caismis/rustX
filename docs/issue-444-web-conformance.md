# PR #444 Full Web conformance repair

The failing run is [37010526549, Full Web conformance](https://github.com/Caismis/rustX/actions/runs/37010526549/job/110848899659).
It checked out merge `4cd2754e3ad4d14786efdd863ced09e9709b142f`, incorporating
PR head `2835d21922711169996a85bda83aa150c7864ad7` and main
`89f35bdf5794b7e30f23ee7ce96a5dabdf262551`. All Web type/unit/i18n/provenance
checks passed. Browser acceptance passed 147/148; the only failure was
`agent.spec.ts:96`, dark 390px composer, `running-draft` screenshot.

## Evidence

Expected and actual dimensions are exactly 334×174. Only 16 pixels differ, all
on the top-left blue focus stroke. Maximum raw channel delta is 18. The actual
decoded RGBA SHA256 is
`aba0af7af53c4ddda1163224ca7cb5d405aff44d52f45c7c44dc450ea3ecbebf`.
The baseline RGBA SHA256 is
`763a963d3b479db44951a2dc939dd49367202f37ee3846befe0c4d43ca7b0f96`.
The CI artifact retains the trace, actual and diff. No baseline was regenerated.

A separate probe used the same digest-pinned Playwright 1.63.0 container as CI,
30 fresh browser contexts, 390×844, English, dark theme, reduced motion and the
test's fixed date. It replayed the fixture flow: empty composer → fill initial
draft → submit → mark running → fill `Queue the next review.`. It captured the
preceding states before capturing the running draft, using the same screenshot
options (`animations: disabled`, `caret: hide`, `scale: css`). There were no
baseline-driven retries, sleeps or edits between samples. Thirteen captures
matched the baseline; seventeen matched the CI variant, including every decoded
RGBA byte. No third rendering occurred.

Before each capture the probe recorded every element in the composer stack:
tag, class, text content, exact floating-point bounding rectangle, every computed
CSS property, scroll offsets and focused-element identity. Samples 0 (baseline)
and 5 (CI variant), among others, have identical complete serialized probe SHA256:
`251e4ecba1d52523d4003aca071eef08d656cc0e8b6dc4f2e963f640b6a7e5f3`.
Their relevant values are:

| Surface | Exact viewport rectangle | Style/state |
| --- | --- | --- |
| Composer card | x=72, y=742, width=302, height=98 | radius 22px; `superellipse(1.5)`; background rgb(44,44,46) |
| Focus stroke | Same card | rgb(103,158,254), shadow spread 0.5px, blur 0 |
| Textarea | x=72, y=750, width=298, height=36 | focused; same draft, styles and scroll offsets |

Thus the variance is in rasterization, not product geometry, text, theme, focus
state or file-delivery behavior. The browser's internal cause is not inferred.
The local probe artifacts are retained in `/tmp/rustx-444-composer-probe`, with
CI evidence in `/tmp/rustx-444-ci-37010526549`. The manifest preserves the raw
reference/observed RGBA tuples, so deterministic tests do not depend on those
temporary files or on the browser reproducing its variance.

## Repair contract

The existing measured-noise manifest gains one reference-local entry. Its four
disjoint, one-pixel-high row regions contain exactly the 16 measured positions:

| y | x coordinates | Maximum changed pixels | Maximum channel delta |
| --- | --- | --- | --- |
| 71 | 30–33 | 4 | 14 |
| 72 | 26–33 | 8 | 18 |
| 73 | 24–26 | 3 | 11 |
| 74 | 24 | 1 | 13 |

Every other pixel remains exact, including adjacent corner pixels, the rest of
the stroke and all text. The comparator, capture stabilization, timing budget,
browser image, product code and baselines are unchanged. Other reference names
receive no new allowance. This follows the repository's existing rule: independently
prove rasterizer-only variance, record raw evidence, then bound it spatially and
by measured channel delta. A global perceptual threshold is not introduced.

`screenshot-comparison.test.ts` deterministically replays the evidence and proves
that neighboring pixels at delta 1, each row's bound plus 1, another reference
name, text/focus contrast changes, a whole-image theme shift, a one-pixel layout
shift and a dimension change still fail. The ordinary stable-capture contract
still requires two consecutive byte-identical renderings before comparison.

## Validation

| Command | Result |
| --- | --- |
| `pnpm --dir web-console test screenshot-comparison.test.ts screenshot-stability.test.ts` | 33 passed in 2 files |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | 1,358 passed in 73 files |
| `pnpm --dir web-console check:i18n` | Passed |
| `pnpm --dir web-console check:provenance` | Passed: 147 source records, 132 production package notices |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | Production build passed; 148 browser tests passed in 9.1 minutes |
| `git diff --check` | Passed |

Before the fix, the unmodified isolated composer case passed 20 repetitions;
that alone did not disprove the hosted failure. The independent 30-context
probe above supplied the actual contrasting evidence. The first new focused
test run caught a test setup mistake: adding a delta to an already changed
observed pixel does not establish that delta against the reference. The negative
fixture now applies each over-bound delta to the reference and fails as intended.
The complete Web lane then passed on its first post-repair run. No automatic
test retries, baseline updates, skipped tests or longer time bounds were used.

Native/macOS, protocol and TUI code are unchanged by this repair. The original
repair head's hosted macOS boundary lane completed successfully in run
37010526549, alongside Linux, protocol and TUI lanes.
The post-validation fetch still found main at
`89f35bdf5794b7e30f23ee7ce96a5dabdf262551`; no integration was needed. The
existing PR #444 branch and worktree are retained; the original worktree was
not modified.
