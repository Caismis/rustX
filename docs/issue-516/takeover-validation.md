# PR 517 takeover validation

Checked on 2026-11-04, Asia/Shanghai. The local development branch and the PR
head were both `54ef383f7a988d4eb84a1eb3ae83e94ea5a186cd`. Fetching both remotes
confirmed `origin/main` at `b0aa75fe110ca53a1dfc0637280f2f08508c8b7e`, with no
new base commits and fourteen PR commits. The separate performance worktree at
`ea51060ab` is already an ancestor of this branch. Existing local projection and
Parts search changes were reviewed and completed on the original PR branch.

## Request classification repair

[Comment 6623187578](https://github.com/Caismis/rustX/pull/517#issuecomment-6623187578)
identified three failures in the default Vite browser lane. A WASM URL-import
helper is JavaScript metadata; intercepting it prevents the viewer from loading
before its waiting CAD Worker exists. Counting it also double-counts the later
binary fetch and misidentifies STL's URL metadata as CAD runtime loading.

Shared test classifiers now recognize actual WASM binary requests, executable
CAD glue and Worker entry points separately from Vite URL/import metadata. The
cancelled-wait, two-owner compiled-code sharing and STL lazy-loading assertions
retain their original resource, source-read, deadline and retirement checks.
Nineteen request-shape regressions cover development and hashed production URLs.

The prior exact-head CI also had one Workflow editor connection-refused failure;
its unchanged `keyboard selection and node-move undo` test passes in a separate
local browser run. That isolated result does not establish its cause or certify
the next complete CI run. Historical CI success or failure is not verification
of the new revision.

## Completed local viewer work

The View menu adds perspective/orthographic switching while retaining the same
renderer/context, target, direction, up axis and target-plane apparent scale.
Orthographic wheel zoom saves effective vertical span, including final-pose
retirement and same-copy restoration. Resize, picking, Fit, Focus and Reset use
the active projection. Invalid saved spans are rejected. Outline facing tests
use parallel rays for orthographic views, and depth tolerance follows the active
projection. Authored and generated thin-overlap pixel cases cover both
projections at model scales `1e-6`, `1` and `1e6`.

Parts search filters only admitted names and localized fallback names. Matching
ancestors appear through collapsed branches, using complete occurrence-ID
segments. The query is bounded to 256 characters and is transient per panel/copy.
Clearing it restores manual expansion. Search leaves camera, selection,
visibility, source reads and imports unchanged. The input stays above the
independently scrollable tree. English/light and narrow Chinese/dark captures
were inspected for clipping and usability.

No dependencies, CAD assets, source authorization, resource limits or native
runtime code changed in this increment. The original 50-ms interaction P95 /
200-ms cold first-complete-display goals remain unmet. These correctness checks
do not recertify performance or physical display presentation. The PR stays draft.

## Validation

The complete Web/Host suite passes **4,141 tests with four platform skips**.
The default development-fixture browser selection passes **43 tests**, including
the three request-boundary regressions and all projection/search cases.
The committed production-fixture script passes the same **three request-boundary
tests**. The separate Workflow editor regression passes **one test**.
Type checking, the production application and fixture builds, localization
ownership, 336 source-provenance records, 345 dependency notices, exact CAD assets
and the 2-GiB WASM maximum pass. Rust formatting and whitespace checks pass.

Browser runs use the existing pinned Playwright 1.63.0 OCI filesystem via
Bubblewrap, with its fonts and Chromium/SwiftShader. The default run serves
the actual `test/workbench.vite.config.ts` development fixtures; the second run
serves the production geometry fixture build. Isolated ports 5373/5374 and direct
Node entry points avoid local port conflicts and the host pnpm launcher's
unrelated auto-install attempt. No screenshot references or comparator settings
are changed. These runs preserve the existing browser test bodies and assertions.

Browser results and source/capture hashes are recorded in
[takeover-evidence.json](takeover-evidence.json). The fixture build's explicit
`esnext` target remains distinct from the ordinary application build, as described
in [production-fixture reproduction](production-fixture-benchmark.md).

[Orthographic menu](takeover-orthographic-menu.png) ·
[English Parts search](takeover-parts-search-en.png) ·
[Chinese dark Parts search](takeover-parts-search-zh.png).

To reproduce using the repository's pinned browser container and installed
lockfile dependencies, from `web-console`:

```sh
pnpm typecheck
pnpm test
pnpm build
pnpm check:i18n
pnpm check:provenance
pnpm test:e2e -- geometry-code.spec.ts geometry.spec.ts geometry-review.spec.ts geometry-roadmap.spec.ts geometry-parts-search.spec.ts geometry-sidebar.spec.ts
```

The [production-fixture reproduction](production-fixture-benchmark.md) provides
the committed command for the same three actual-binary cancellation/reuse and STL
boundary test bodies. An ordinary development-only result does not establish
both lanes. Full CI on the pushed head remains separate from these local checks.
