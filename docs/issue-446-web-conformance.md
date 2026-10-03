# PR #446 light composer conformance repair

## Scope and hosted evidence

The reviewed PR HEAD is `940cbe95d87c6c4337f6917f8ba876feb97a513b`;
its merge base is `06dc8b101acf5a880466548e9e9b3214e41ac7e8`.
Fetched main moved to `45e8ca434994910587078538d80ca10d4a7ce9d0` through
PR #445, which changes composer behavior and references and conflicts with this
PR. The user explicitly kept this repair scoped to the reviewed HEAD; integration
of that newer product state is separate. This repair does not claim merge readiness.

[Run 37038446759](https://github.com/Caismis/rustX/actions/runs/37038446759/job/110942323119)
and [run 37081974267](https://github.com/Caismis/rustX/actions/runs/37081974267/job/111084264546)
both failed `agent.spec.ts:96`, `composer primary seat, uploads and context stack
light 390`. Downloaded actual/diff artifacts agree byte-for-byte. All other required
lanes passed on the latter run, including the repaired macOS Node desktop test.

The reference is `composer-running-draft-light-390-linux.png`. Expected and actual
are 334×174, with 15 changed pixels and maximum channel delta 13, confined to the
upper-left blue focus stroke. Decoded RGBA SHA256:

- Reference: `668cb3c9bb354fdd53a6272ab2e171e34532d3c87983b18a1bb6be1885590df2`
- Both hosted actuals: `234c5deacf218da82be06aa7d7690dacfc5984797318bd81c8d66b2c7fce9c55`

`docs/issue-430-validation.md` already records the same dimensions, pixel count,
delta and corner before #446. This evidence does not attribute the variance to
#446. The dark evidence in `docs/issue-444-web-conformance.md` informed the probe
method, but neither its pixels nor its bounds were copied into the light policy.

## Independent pinned-browser measurement

The probe uses exactly
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`
through the repository's `scripts/browser-tests.sh`. Every sample creates a fresh
context: light theme, English, 390×844 viewport, device scale 1, reduced motion,
fixed date `2026-09-18T12:00:00Z`. It follows the existing composer fixture flow:
empty → `Review the composer interaction contract.` → click Send → mark running
→ `Queue the next review.` with the textarea focused and primary action Queue.
Each preceding screenshot state is captured too. Every screenshot uses the
unchanged `captureStable` and the repository capture cadence/options: two animation
frames, the existing settle schedule, animations disabled, caret hidden, CSS scale.
There are no baseline-driven retries or comparisons controlling sample collection.

Across 30 fresh contexts, 24 stable captures matched the reference and 6 matched
the hosted variant, byte-for-byte in decoded RGBA. No third output occurred. Every
sample stabilized in exactly two captures. Sorting computed CSS property names
(the browser enumerates custom properties in varying order) before deterministic
JSON serialization gives all 30 samples the same complete product-state SHA256:
`566b7df826fb1d8f455285e0eac2bf91f4fb8a7e211fb92a316e9144b55d372a`. No property or
value is omitted in this canonicalization.

Before and after each final stable capture the probe records the complete composer
context-stack HTML, all descendant tags/classes/text/attributes, textarea value and
selection, every computed CSS property (including both pseudo-elements), exact
floating-point bounding rectangles, element scroll offsets and dimensions, focused
element identity, fixed date, motion preference, viewport/visual viewport, device
scale and page scroll. Before/after fingerprints must be exactly equal.

Samples 7 (reference) and 11 (hosted variant) have byte-identical complete serialized
state SHA256 `804546f809a3d78763b1f0efaf8fd214ea45d95838a809b287e87fa08d0da19e`.
Their decoded screenshot hashes differ exactly as listed above. Both require two
consecutive RGBA-identical captures to establish stability. Relevant shared facts:

| Surface | Exact viewport rectangle | State |
| --- | --- | --- |
| Composer card/focus stroke | x=72, y=742, width=302, height=98 | White surface; focus shadow rgb(65,118,230), spread 0.5px, blur 0 |
| Textarea | x=72, y=750, width=298, height=36 | Focused; same draft and selection; scrollTop=scrollLeft=0 |
| Viewport | 390×844, scale 1 | Page scroll=(0,0); visual viewport offset=(0,0) |

Probe source, per-sample complete fingerprints, stable PNGs and hash records are
retained in `/tmp/rustx-446-light-probe`; downloaded hosted artifacts are in
`/tmp/pr446-light-latest` and `/tmp/pr446-light-previous`. Raw pixel tuples remain
in the checked-in manifest, so comparator regressions do not depend on these
temporary files or on the renderer reproducing variance during tests.

The measurement demonstrates rasterization variance with identical observable
product state and geometry. It does not establish an internal Chromium root cause.
The checked-in reference is one valid rendering and remains unchanged.

## Minimal reference-local allowance

`test/fixtures/rasterizer-noise.json` adds only the light reference. Each tuple's
`before` comes from the checked-in reference and `after` from the stable measured
variant, which equals both hosted actuals. Five disjoint one-row regions cover
exactly the 15 measured pixels:

| y | x | Maximum changed pixels | Maximum channel delta |
| --- | --- | --- | --- |
| 71 | 30–33 | 4 | 12 |
| 72 | 26–30 | 5 | 13 |
| 72 | 32–33 | 2 | 9 |
| 73 | 24–26 | 3 | 10 |
| 74 | 24 | 1 | 12 |

Pixel (31,72) did not change and remains exact. No unmeasured site is inside these
regions. All other pixels and reference names retain their previous exact contract.
The dark manifest entry, comparator implementation, capture stabilization,
deadlines, screenshot references, product CSS and desktop production code are unchanged.

## Deterministic regressions and validation

The composer comparator cases are parameterized for both light (15 sites) and
dark (16 sites), retaining the dark checks. They prove the measured variant passes,
no-policy and foreign-reference comparisons fail, immediate neighboring pixels
(including the light row-72 gap) fail, every region's channel bound plus one fails,
and text, focus contrast, broad color shifts, one-pixel layout shifts and width or
height changes fail. Since every region's budget equals its area, an additional
changed site necessarily falls outside it and fails. A test-only copy lowering each
region's budget by one also proves the count guard rejects the same measured
pixels independently of location and delta guards; the production policy is never
expanded for this test.

- Focused comparator/stability tests: 36 passed in 2 files.
- Web typecheck, i18n, provenance and production build: passed; 147 source records and 132 dependency notices verified.
- Web unit tests: 1,397 passed across 75 files, including all 30 desktop Host tests.
- Dev typecheck/tests: passed, 38 tests.
- App Server protocol generation check and typecheck: passed; no generated changes.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo build --bins`: passed.
- `git diff --check`: passed.
- `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e`: Playwright reported **150 passed (11.5m)**, including light/dark composer cases, with no failed assertions or reference updates. After that success summary the outer command exited 143 (SIGTERM); this is not recorded as a clean command exit. The signal source was not established. The run's container and issue-432 Vite processes were gone; the remaining browser run belonged to issue-435 and was left untouched. Log: `/tmp/pr446-light-e2e.log`. No screenshot retry was used to obtain these results.

Fresh hosted PR validation remains blocked by the explicitly deferred main merge
conflict. Prior hosted results describe the reviewed HEAD, not this new repair.
No Finder/Terminal GUI smoke is claimed; desktop production behavior is unchanged.
