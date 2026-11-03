# Geometry performance follow-up

This follow-up implements the camera-publication, visible-picking, drawing-buffer
and canceled-payload opportunities from PR #517. The comparison baseline is
`47b46025f2d78459cec305a46ccf792051685f8b`. The earlier
[performance report](performance.md) remains a separate historical measurement.

The subsequent latency-budget work is paused at the user's request. See the
[2026-11-03 checkpoint](performance-budget-checkpoint.md) for retained changes,
rejected experiments, raw samples and remaining bottlenecks. The 200 ms camera
and 500 ms first-display targets have not been met. The results below continue
to describe the earlier published follow-up.

## Changes

- Retain the latest camera pose and publish at most once per 100 ms during
  movement. Interaction end flushes immediately. Normal body retirement saves
  pending inert metadata only for the retained occurrence and exact authorized
  content copy; split/move retain that last update. Refresh, replacement and
  revoked authority cannot publish obsolete metadata.
- Batch initial hidden-part restoration, retain unchanged React part rows and
  memoize their derived metadata. Repeating an unchanged selection or visibility
  action neither publishes nor schedules another draw.
- Prune hidden subtrees before raycasting visible mesh candidates, then resolve
  the closest hit through an object-to-occurrence map. No acceleration structure
  or additional mesh data is created.
- Cache CSS dimensions and effective DPR. A real size/DPR change uses one public
  `setDrawingBufferSize` call; unchanged notifications do nothing. Resolution
  media-query listeners are rearmed on DPR changes and released on disposal.
- Consume and validate canceled HTTP multipart responses through their physical
  terminal, while skipping the final unused `Blob.arrayBuffer()` conversion.
  Bounded parsing still creates a Blob; this is not a streaming-memory or RSS
  reduction claim. Malformed responses and unknown settlement retain their
  existing precedence over canceled publication.

No camera quality, lighting, CAD tessellation tolerance, resource ceiling,
source-copy ownership or idle-rendering policy changes. No new dependency,
server-side conversion or parsed-model/WASM cache is introduced.

## Comparison method

Both checkouts ran the unchanged `geometry-performance.spec.ts` and observation
fixture on the same Intel Core i9-14900K host, with Chromium 153.0.8010.12 from
the repository's exact Playwright 1.63.0 OCI image. Bubblewrap mounted the image
root read-only with its own fonts and private temporary browser data. Browser
processes were bound to logical CPUs 0 and 1; no additional memory quota was set.
The renderer was ANGLE/SwiftShader. These conditions differ from the older
Windows/WSL, CPU-quota and 2-GiB report; its numbers are not the baseline here.

The viewport was 1440 by 1000 at DPR 1. Each action has five samples and ends
after two animation frames. P50/P95 use nearest rank, so P95 is the maximum of
those five elapsed samples. Orbit/pan use consecutive eight-step drags. The
recorded publications and React commits are totals across all five actions and
cover the development fixture, not only the geometry component. First display,
frame distributions, long tasks and two-model load remain in the raw samples.
No unrelated build or unit suite ran during either performance capture.

[Raw before/after samples](performance-followup-samples.json) and
[environment/source/fixture digests](performance-followup-environment.json)
identify the compared inputs. Both captures use the 735,000-vertex STL and
96-part STEP assembly, with the same retained source-copy and two-pane checks.

## Results

Elapsed values are milliseconds. Counts are before to after.

| Workload | Action | Before P50 / P95 | After P50 / P95 | Publications | React commits |
| --- | --- | ---: | ---: | ---: | ---: |
| STL, 735k vertices | Remount | 2233.3 / 2332.5 | 1833.4 / 1949.8 | 30 → 20 | 35 → 35 |
| STL, 735k vertices | Orbit | 3000.1 / 3047.5 | 3000.1 / 3095.6 | 40 → 40 | 60 → 60 |
| STL, 735k vertices | Pan | 1816.6 / 2663.6 | 1816.6 / 2715.8 | 40 → 40 | 60 → 60 |
| STL, 735k vertices | Repeated picking | 399.9 / 427.6 | 33.3 / 44.9 | 5 → 0 | 8 → 0 |
| CAD, 96 parts | Remount | 2633.0 / 2668.9 | 2580.0 / 2629.1 | 30 → 20 | 35 → 35 |
| CAD, 96 parts | Orbit | 616.7 / 650.0 | 599.9 / 745.1 | 40 → 26 | 60 → 39 |
| CAD, 96 parts | Pan | 578.5 / 583.5 | 566.7 / 600.0 | 40 → 22 | 60 → 33 |
| CAD, 96 parts | Repeated picking | 75.1 / 126.4 | 33.2 / 33.7 | 5 → 0 | 8 → 0 |

CAD camera publication/commit counts fell by 35% for orbit and 45% for pan.
This does not establish a camera-latency improvement: observed P95 elapsed
times were higher. The software-rendered STL remained limited by rendering;
its camera events were already slower than the publication interval, and its
orbit/pan counts and median durations did not improve.

Repeated picking still performs actual raycasts. The benchmark clicks an already
selected occurrence, so its improvement includes avoiding unchanged selection
outlines, publications and redraws. It does not establish the same speedup when
selecting different parts. A separate deterministic test verifies that a hidden
76,800-triangle mesh receives zero raycast calls while the nearest visible mesh
remains selectable.

Both before/after two-model runs observed two contexts. After retirement, owned
Workers, contexts, buffers, textures and URLs were all zero. Logical disposal
does not imply immediate browser/driver memory reclamation. No process-memory,
per-Worker peak, hardware-GPU or general smooth-frame-rate claim is made.

## Validation and reproduction

The full Web suite passed 4003 tests with four platform skips. After the final
disposal reentrancy guard, 117 focused viewport/component/retirement/Session/
workspace tests passed. Decoder/HTTP regression checks cover 63 cases, including
skipped allocation, pending multipart completion, malformed attachments, queued
reads and settlement-unknown precedence. Type checking, production build,
localization, source provenance/runtime assets, Rust formatting and whitespace
checks pass. All 16 fixed-browser geometry functional cases and both repeated
performance/retirement cases pass. The new camera-burst regression bounds
publication count, immediately hides before interaction end, restores the final
pose, and verifies that the retained copy needs no second complete source read.

Run the same fixed-browser path in baseline and candidate checkouts:

```sh
pnpm test:e2e -- geometry-performance.spec.ts
pnpm test:e2e -- geometry.spec.ts geometry-review.spec.ts geometry-sidebar.spec.ts
```

Keep hardware, browser, CPU policy, DPR and fixtures equal for a new comparison.
Use the source/fixture digests when interpreting these five-sample results;
they are not cross-machine performance guarantees.
