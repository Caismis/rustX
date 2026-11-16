# PR 517 review follow-up

Work resumed on 2026-11-04 (Asia/Shanghai) from `093aa9ae9`. The
[comment inventory](comment-status.md) distinguishes implemented requests,
performance acceptance and later inspection features.

## Correctness and sidebar changes

The latest review's thin-box occlusion case uses a projection-aware outline
depth tolerance. Three browser cases at scales `1e-6`, `1` and `1e6` compare the
same camera with the hidden rear box present and absent, requiring identical
canvas pixels. A separate check verifies that the visible front outline still
appears. Parts panels and their focus tooltips are closed before captures;
changing those overlays initially contaminated the comparison.

Real installed OrbitControls tests exercise tiny focused pointer, keyboard and
wheel input. Final camera metadata survives retirement before queued input
observation, through the existing retirement publisher rather than reentering
body admission. No continuous idle rendering is introduced.

Side questions now occupy a Workbench sidebar tab. The existing auxiliary owner
retains bounded Session/Conversation drafts and execution results. Tab switching
and close/reopen do not submit or cancel execution. English/light and
Chinese/dark browser cases cover draft retention, running-result recovery,
explicit cancellation, primary-conversation coexistence and focus restoration.
The tab reuses the main conversation's user bubbles, streaming Markdown,
scroll-following viewport and composer styling. A small Temporary badge
distinguishes it. The input stays at the bottom; the single action switches
between Send and Stop. First acknowledgement consumes only the matching draft,
including while closed, without erasing later edits or unconfirmed submissions.
The real test fixture now closes Product Host ownership while its native
connection is still alive, and awaits that cleanup.

[English, alongside the primary conversation](review-side-question-en.png) ·
[Chinese, narrow dark layout](review-side-question-zh.png).

File-change hover cards use a dedicated wider variant, viewport-aware placement,
a full-path header with reserved Copy space and normal-size scrollable code.
Four browser cases cover both languages and themes, right-edge/narrow layouts,
pointer travel, text selection, Copy and unchanged ordinary-card width.

Loading labels follow real reading, CAD initialization, import and preparation
events. Finite ordered Worker observations do not replace model validation,
extend deadlines or publish after retirement. Parsing and tessellation inside
the synchronous importer are not presented as invented separate milestones.

## Performance work

The local diagnostic browser uses the existing pinned Playwright 1.63.0 OCI
filesystem through a read-only Bubblewrap root, image fonts and private temporary
storage. This does not change the repository's Docker/Podman screenshot lane.
The host is an Intel Core i9-14900K with no added CPU quota; Chromium uses
SwiftShader, DPR 1 and MSAA. These runs are not comparable as hardware speedups
against the earlier RTX 5080 or CPU-limited lanes.

The existing clock remains trusted input to actual model submission, GPU fence
completion and the next rendering opportunity. It does not observe physical
display presentation. Cold source modules use fresh browser contexts. The
200-ms cold / 50-ms interaction-P95 targets remain unchanged and unmet; the
explicit cold budget gate fails rather than being relabelled as passing.

Exact STL tuple indexing replaces nested stream/component iterator loops with
scalar hash and equality operations. Hash order, float-bit equality (including
signed zero and color/normal seams), probing limits, deadlines and geometric
certification remain unchanged. Twelve alternating baseline/candidate pairs
after four warmup pairs produced identical output hashes. The 992,016-vertex
closed-box median decreased from 53.26 to 27.44 ms; the colored curved case
decreased from 7.327 to 3.441 ms. These are preparation microbenchmarks, not
first-display measurements.

An additional renderer experiment reduced CAD surface submissions from 96 to
four using public BatchedMesh APIs. Its GPU/event latency ranges overlapped the
outline-only variant, so the implementation and its extra 8-MiB storage allowance
were removed. Generated identity-transform outlines instead pack into at most
two line draws using the original 5-MiB vertex-buffer allowance and a small
packed visibility uniform. Authored/transformed GLB outlines retain their local
normal-matrix path. CAD 96-part outline commands decrease from 96 to one;
sub-millisecond submission savings do not establish the 50-ms end-to-end goal.

The page now retains one immutable compiled CAD code module. Every import still
owns a fresh Worker, runtime instance and linear memory. This allows runtime
code reuse without a parsed-model or instance cache. First-demand compilation
is included in the parent deadline and original pointer-to-render clock.
Worker loading overlaps immutable code compilation; bytes are copied and sent
only after the code is available. The importer's own timings exclude that wait,
while total Worker lifetime can overlap it, so those durations are not summed.
The code wait and actual code compilation are reported separately. No work is moved
before visible-body demand to make the cold clock look smaller.

In three intermediate production-fixture contexts, STL preparation decreased from
112–114 to 84–85 ms, and first rendering opportunity from 354–359 to
327–339 ms. Interaction P95 remains below 50 ms. CAD runtime initialization
and synchronous import remain significant cold-path costs; renderer experiments
must demonstrate end-to-end benefit before their added storage is accepted.

The final source capture, with Worker loading overlapping code compilation,
records the following values in milliseconds. The STL row is one final-context
observation, not a new three-context distribution. Its preparation took 91.9 ms;
the earlier three-context results remain intermediate evidence.

| Final workload | Contexts | Cold rendering opportunity | Orbit P95 | Pan P95 |
| --- | ---: | ---: | ---: | ---: |
| 735,000-vertex STL | 1 | 354.6 | 44.3 | 39.3 |
| 96-part STEP | 3 | 700.0–803.8 | 55.7–61.5 | 57.7–59.6 |

The serialized-code experiment took 843.2–873.3 ms for cold CAD and was replaced
by overlapping Worker loading. The original matched CAD baseline was
752.1–835.6 ms: the final ranges overlap, so this is not a strong statistical
claim of a general cold-CAD speedup. Every final cold-budget assertion still
fails. CAD interaction also remains above 50 ms. Actual warm lifecycle checks
record 368 ms for a retained cube reopen and 457.8 ms for two retained CAD
owners, including outline completion; these use a separate clock and cannot
stand in for cold acceptance or isolate the code-cache contribution.

[Raw samples, experiments and bounded phase diagnostics](review-performance-samples.json)
and [environment/source/asset hashes](review-performance-environment.json)
preserve the evidence. [Production-fixture reproduction](production-fixture-benchmark.md)
provides the committed commands. Phase samples distinguish JS heap, backing
storage and WASM commitment; they are not allocation peaks or driver-memory
release measurements. Two overlapping CAD Workers each committed 29,949,952
bytes of WASM memory at the observed checkpoints, with separate instances.

The complete Web/Host suite through overlapping initialization and acknowledged
draft consumption passed **4,087 tests with four platform skips**. Type checking,
the full application build, localization ownership, 336 source-provenance
records, 345 dependency notices, exact runtime assets, Rust formatting and
whitespace checks pass. Six thin-box browser cases, the two actual compiled-code
lifecycle cases, both chat layouts and four hover layouts have browser evidence.
Each later source revision requires its own relevant checks; this checkpoint
does not certify later changes or satisfy the still-failing performance gate.
