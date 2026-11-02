# CAD preview validation

Implementation baseline: `b0aa75fe110ca53a1dfc0637280f2f08508c8b7e`.
Runtime behavior and bounds are specified in [geometry previews](../geometry-previews.md).

## Executed checks

| Check | Result |
| --- | --- |
| TypeScript `tsc --noEmit` / `pnpm typecheck` script | Passed |
| `pnpm build` | Passed, including native Host build and production provenance/runtime checks |
| `pnpm check:provenance` | Passed: 336 source records, 344 production notices, pinned geometry artifacts |
| `node scripts/check-i18n.ts` | Passed |
| Geometry adapters, Worker admission, view races, content ownership and evidence | 66 passed |
| Pinned-container geometry browser suite | 9 passed |
| Native Workbench Host suite after preparing standard supervisor paths | 22 passed |
| `git diff --check` | Passed |

The full Web run covered 261 files / 3,949 tests: 3,935 passed, 10 failed and
4 skipped. Nine failures were native Workbench tests run before their standard
supervisor paths were prepared; that complete 22-test file passes on its focused
rerun. One local failure remains in the unchanged `workflow-model.test.tsx`
case `keeps invalid schema text inside the dialog and discards it on Cancel`,
which times out awaiting `Structured output Schema`. A focused rerun reproduces
that failure. The original PR head's CI Web client/Host/launcher job subsequently
passed. This is not a claim of a clean local all-Web run; exact-head CI remains
the merge gate for the current repair head.

The geometry browser suite covers both committed-delivery and Workbench entry
points for GLB, embedded-texture GLB, binary/ASCII STL, STEP including inch units,
IGES and BREP. It also exercises current-original Download, copy refresh,
authority retirement, split/fullscreen, narrow English/Chinese and light/dark
presentation, keyboard menus, external-reference rejection, context loss,
late-image disposal, failed embedded-image URL cleanup, Top/Bottom orbit before
and after restoration, and bounded retiring decode admission. Adapter/owner
regressions also cover guarded STL format classification, inaccurate accessor
bounding spheres and interrupted reauthorization without replacing a copy.

Fixtures are independently authored/exported as described in
[the fixture manifest](../../web-console/test/fixtures/geometry/README.md).

## Measurements

Environment: Windows host, Ubuntu 24.04 under WSL 2, Node 24.21.0, Linux x86_64
Chromium 153.0.8010.12 in the repository's pinned Playwright 1.63.0 image
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`.
Browser viewports were 1440x1000 and 390x844. Playwright loopback forwarding
connects the pinned browser to the existing test servers through Docker Desktop.

- Geometry viewer bundle: approximately 661 kB raw / 167 kB gzip. CAD and STL
  Worker entries are separate (approximately 64 / 118 kB raw respectively).
- CAD WASM: 7,604,031 bytes, approximately 3.1 MB compressed. Exact original
  release sizes and hashes are in `web-console/geometry-runtime.json`; bundler
  compression estimates can differ with zlib settings.
- [Display measurements](format-display-timings.json) use monotonic elapsed time,
  assert finite nonnegative durations before publication, and include UI gesture,
  transport/test overhead and import/display, rather than claiming isolated
  tessellation timings. They cover first delivery opens and later Workspace
  opens in the same browser context.
- [The STL envelope](stl-envelope.json) contains 245,000 triangles, 735,000
  vertices and 12,250,084 source bytes. Observed first display was about 1.5 s,
  with a maximum sampled animation-frame gap of about 283 ms on this host. That is an
  observation, not a hard responsiveness guarantee.
- Browser-container cgroup `memory.peak` across acceptance runs was
  **1,190,846,464 bytes (about 1.11 GiB)**. It includes browser/Worker processes
  and page cache, and excludes the WSL build/test host. Separate per-Worker RSS
  was not attributed. The verified 2-GiB WASM maximum is an admission ceiling,
  not the measured import peak or a JavaScript/process memory cap.
- Instrumented Workers, WebGL contexts, buffers, textures and resource URLs
  return to zero after retirement. Context loss accounts for driver-owned
  fallback textures. Image bitmaps completing after cancellation are closed.

## Visual evidence

The viewer reuses DSH Button/Menu primitives, floating document controls,
typography and theme tokens. Camera fitting preserves a nonzero up vector;
source names remain human-readable and fixed-copy text describes Refresh.

![Desktop geometry preview](cad-preview.png)

![Narrow Chinese dark geometry preview](geometry-narrow-zh-dark.png)

## Reproduction

Run the repository's normal `pnpm test:e2e -- geometry.spec.ts` path on its
supported Linux/Docker setup. In Docker Desktop environments with a separately
exposed pinned browser server, set `RUSTX_BROWSER_WS_ENDPOINT` and run the same
`geometry.spec.ts` through `playwright.config.ts`. The test uses Playwright's
native loopback forwarding; it does not substitute an unpinned browser or
change screenshot authority.

The focused deterministic command is:

```sh
pnpm exec vitest run test/geometry-adapters.test.ts test/geometry-worker.test.ts test/geometry-view.test.tsx test/preview-content.test.ts test/geometry-evidence.test.ts
```

Source/geometry admission and Worker termination are enforceable at their
documented seams. Browser JavaScript heap, in-flight native image decoding and
graphics-driver RSS are not general per-owner hard limits. See the resource
accounting in the normative geometry document for retiring GLB decode batches.
