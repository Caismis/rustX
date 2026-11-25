# Streaming STL source evidence

The STL preview now overlaps authorized source reads with decoding in its disposable Worker. GLB, STEP, IGES and BREP still require complete input. The benchmark compares a fixed one-read window, a fixed two-read window, and a bounded adaptive experiment. The shipped default uses bounded adaptive one-to-two reads for streamed STL, and fixed two reads for complete-input GLB/CAD. These are internal implementation policies, not user configuration.

Local checks and retained failure history are recorded in [validation.md](validation.md).

## Source and decoder ownership

- One persistent source operation captures size/version and validates the descriptor and authorized path again before successful close. The Worker receives EOF only after that close succeeds.
- Reads and transferred chunks are at most 2 MiB. There are at most two pending read pages, one currently acknowledged decoder page, and one outstanding Worker chunk message. An earlier slow page blocks the finite reorder window.
- Binary records can span pages; the decoder retains at most 83 header bytes and 49 incomplete record bytes. ASCII decoding uses fatal UTF-8, bounded token/name carry, decoded attribute pages, and preserves separate solid names, normals and part groups. Binary Magics colors retain the existing sRGB conversion.
- Both the producer and scene receiver independently admit the completed model. Source reads, decoding and final validation share the original 15-second STL deadline; chunk acknowledgements never reset it.
- Production pages transfer their exact owned backing buffers. A generic operation returning a subview gets one bounded page copy before transfer, preserving its larger owner buffer.
- For sources above 64 MiB, the parent does not assemble a complete STL source or clone it into the Worker. Sources at or below 64 MiB retain one raw owner copy within the existing inactive-content cache budget: each page is copied into that buffer before transfer, and the copy becomes reusable only after final source validation succeeds. Large uncolored binary STL files above the existing preparation threshold retain **one Worker-owned source buffer**, up to 200 MiB, filled while decoding. This preserves the established exact moving-view representation and first-GPU-frame preparation gate. Small, colored and ASCII inputs retain no complete raw source in this path.
- Mounted parked bodies retain the same scene and source identity. Actual renderer unmount retires GPU resources but keeps validated raw copies within the existing 64 MiB cache. After an authorized reactivation probe, the same source identity replays that original cached copy in acknowledged pages of at most 2 MiB, preserving its copy ID and saved camera even if the on-disk file changed. Larger uncached sources are evicted on unmount and require a fresh read. Refresh creates a fresh copy generation; eviction or authority loss revokes replay. No parsed-model cache was added. Download remains an independent read.

## Final production-bundle experiments

[Full production samples](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/streaming-source-production.json.gz) contain five alternating-order pairs for four scenarios (60 samples). [Focused competition samples](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/streaming-source-production-tail.json.gz) contain 20 alternating-order pairs of two 32 MiB previews plus Download (60 samples). All 120 samples completed without failure. Both experiments use the same source and minified bundle fingerprints, static HTTP asset delivery, the actual persistent source adapter and production Worker. Bundle construction and warmup occurred outside the timed pairs. Other agent-owned compiler/test jobs were paused throughout both complete experiments.

| Scenario | Fixed one median | Fixed two median | Adaptive median |
| --- | ---: | ---: | ---: |
| Real local 200 MiB, no added delay (5 samples each) | 1405.4 ms | 1312.6 ms | 1311.1 ms |
| Two real local 32 MiB previews plus Download (20 each) | 477.75 ms | 459.55 ms | 478.60 ms |
| 32 MiB, diagnostic 10 ms added per read (5 each) | 399.5 ms | 290.5 ms | 330.4 ms |
| 32 MiB, diagnostic 50 ms added per read (5 each) | 1045.8 ms | 647.6 ms | 710.0 ms |

The selected streamed-STL default is adaptive: it matched fixed two in the actual local single-preview median and had the lowest observed Download p95/maximum in the 20-pair experiment. This accepts its diagnostic delayed-transfer throughput penalty. Complete-input GLB/CAD retains fixed two because it has no simultaneous decoder ACK stage to guide a ramp. No new tuning threshold or user setting was added.

The actual 200 MiB source/read-to-decoder overlap medians were 338.8 ms, 534.8 ms and 521.1 ms respectively. Fixed two and adaptive had essentially equal local single-preview medians, about 6.6% below fixed one. Fixed two had the lowest focused competing median and diagnostic delayed-transfer medians.

| Focused 20-pair measure | Fixed one | Fixed two | Adaptive |
| --- | ---: | ---: | ---: |
| Complete previews: p95 / maximum | 619.0 / 693.0 ms | 530.9 / 886.0 ms | 518.6 / 660.8 ms |
| Download median | 92.55 ms | 93.20 ms | 93.55 ms |
| Download p95 / maximum | 130.4 / 135.3 ms | 135.8 / 171.6 ms | 103.2 / 106.2 ms |

P95 uses the nearest-rank statistic; with 20 samples it is the second largest sample. The fixed-two outlier persists in production and is retained in the evidence. Adaptive produced the lowest observed Download p95/maximum in this local experiment; fixed two's faster median and delayed-read throughput do not establish a Download tail improvement. These results describe this device and workload, not a general tail guarantee. The original five-pair production competition medians were 482.0 / 508.0 / 495.5 ms, illustrating why the focused larger sample was needed.

The source/decode benchmark directly exercises `readGeometrySource` and the Worker, and excludes the logical owner’s existing inactive raw-copy cache. Its zero parent-assembly counters therefore describe this direct pipeline; the application additionally retains the bounded <=64 MiB owner copy described above. Every source open, read and final close, plus Download, used the same two-slot FIFO. Download had zero-based read admission rank 2 in every competing sample. All chunks were at most 2 MiB; production page buffers transferred without a `Uint8Array.slice` copy, with zero parent full-source assembly. Eligible large uncolored binary inputs retained one Worker source as described above. Full geometry counts and post-timing authored-attribute digests passed in every sample. Network logs confirm that no CAD Worker, WASM runtime or development `/src/` module was requested.

The comparison runner passes each policy explicitly. The final default-selection change only chooses the already-measured adaptive algorithm when a streaming consumer exists; it does not change that algorithm.

[Final omitted-policy smoke](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/streaming-source-default-smoke-contended.json) exercises the shipped default on one complete 200 MiB STL and two competing 32 MiB previews plus Download. Both runs passed full geometry/digest, transfer, FIFO, capacity and original-deadline checks. They started with one read and ramped within the two-read bound. Native runtime builds were active, so this artifact is a **contended functional diagnostic only** and its timings are not policy evidence. It records the final source fingerprint `d1970f91…` and final entry `6057ed0f…`; the STL Worker remains byte-identical to the comparison artifact.

[Large ASCII functional smoke](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/streaming-source-ascii-smoke-contended.json) passes one exact 32 MiB valid ASCII file through the same production persistent source and streaming Worker under native build load. All 365,781 triangles, authored normals and both solid names/groups are preserved. An emoji begins at source byte 2,097,151, deliberately splitting its UTF-8 sequence across a real 2 MiB read boundary. The import completes within the unchanged 15-second deadline; the adaptive reader stays at one because decoder ACK work dominates. Parent assembly, page slice-copy bytes and Worker full-source retention are all zero in this direct source/decode harness. The logical application owner additionally caches this 32 MiB raw copy within its existing budget. This is a contended functional feasibility check, not policy timing evidence. The final comment-only source change records fingerprint `aeec783a…`; the compiled STL Worker remains identical.

The full experiments fingerprint `geometry-source.ts` as `5659cc9a…`, `parse.ts` as `b0b22ac1…`, the production entry as `d01eb81f…`, and the STL Worker artifact as `5f3e64aa…`; full hashes and the native descriptor-addon hash are in both raw files. The emitted but unrequested CAD assets are inventoried separately and do not participate in the STL measurements. After final raw-cache/GLB correctness verification and the final native runtime staging, the refreshed source-harness bundle still emits the identical STL Worker `5f3e64aa…`. Its entry fingerprint is `e0e30d8c…` because the bundled but unrequested CAD asset inventory changed (WASM `40223e18…`). No measured STL policy body changed, and no additional timing claim is made from this bundle refresh.

## Initial paired browser experiment (development modules)

[Raw samples and source fingerprints](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/streaming-source.json.gz) contain 60 samples: five alternating-order pairs for each of four scenarios, with three policies per pair. No samples failed or were removed. The run used installed Chrome 157.0.8103.38 on Linux 6.14.0-37, an Intel Core i9-14900K and 32 logical CPUs. Other agent-owned Rust/C++ builds were paused for the complete timing interval.

This initial run serves the browser code and Worker through Vite development modules. Worker startup/transformation can affect the measurements; the results are retained as development-mode diagnostics, not final production policy evidence. The browser reads real local STL files through HTTP binary pages backed by production `openWorkspaceBytePreview` persistent descriptors. Production `PreviewDemand(2)` admits source opens, reads, final closes and competing Download. Production `parseGeometryStream` and its STL Worker perform the actual decode and receiver admission. This isolates source/import latency; it does not measure first GPU frame or certify pinned screenshot output.

| Scenario | Fixed one median | Fixed two median | Adaptive median |
| --- | ---: | ---: | ---: |
| Real local 200 MiB, no added delay | 1552.6 ms | 1410.4 ms | 1499.2 ms |
| Two real local 32 MiB previews plus Download | 720.3 ms | 682.8 ms | 720.5 ms |
| 32 MiB, diagnostic 10 ms added per read | 600.1 ms | 466.1 ms | 573.3 ms |
| 32 MiB, diagnostic 50 ms added per read | 1142.4 ms | 754.4 ms | 853.5 ms |

Fixed two improved the 200 MiB median by 9.2% against fixed one and 5.9% against adaptive. Median measured overlap between actual read intervals and Worker chunk send/ACK intervals was 297.9 ms, 505.2 ms and 444.5 ms respectively. This overlap begins during the source operation, before a complete source exists in the parent.

The initial competing-workload Download medians were 356.2 ms / 355.1 ms / 344.9 ms. Download always received the next admission after the two initial read requests (zero-based rank 2), and all physical operation concurrency stayed at or below two. **A fixed-two outlier remains in the evidence:** its preview maximum was 1198.4 ms and Download maximum 541.6 ms, versus 815.8 / 376.5 ms for fixed one and 771.6 / 362.4 ms for adaptive. Five Download samples per policy do not establish a reliable tail estimate. The separate production 20-pair experiment above follows up this outlier with static bundled code.

The adaptive experiment compares bounded rolling read wait and decoder ACK cost, with two-sample hysteresis. The real workloads produced upgrades but no downshifts. A focused controlled test demonstrates a read-bound upgrade followed by a downshift to one when ACK processing dominates; this test is not presented as real-device timing evidence.

Every successful sample checks complete triangle and normal counts, finite admitted geometry, independently sampled authored coordinates/normals after the timed interval, and fixture SHA-256 identities. Samples record zero parent source assembly in the direct source/decode harness, zero `Uint8Array.slice` bytes on the production page path, every transferred chunk detached from its parent, and honest Worker raw-source retention for eligible large binary inputs. The logical owner cache is deliberately outside these source-policy measurements.

## GLB input ownership

GLB remains a complete-input backend. Its importer now passes an exact owned ArrayBuffer read-only to the pinned GLTFLoader instead of unconditionally cloning the whole source. A subview receives one exact-range copy so unrelated prefix/suffix bytes cannot enter the loader. The loader’s intrinsic BIN and bufferView copies remain, and all source, decoded-allocation and receiver admission limits are unchanged. Real-loader tests verify buffer identity, unchanged/non-detached caller input, subview isolation and a valid retained scene after the caller releases its raw buffer. No GLB timing claim is inferred from the STL measurements.

## Reproduction

Start a Playwright server using the installed Chrome executable, then run from `web-console`:

```bash
node --experimental-transform-types scripts/geometry-stream-browser-benchmark.ts \
  --browser-ws <Playwright WebSocket endpoint> \
  --output ../docs/issue-556/streaming-source-production.json
```

The harness defaults to a minified production bundle, static Worker assets, and a static HTTP server; it asserts that no `/src/` modules, CAD Worker or WASM runtime are requested for the STL experiment. `--dev` explicitly selects the original development-module control. Bundle and native descriptor-addon fingerprints are recorded with the source fingerprints.

To build outside an uncontended timing window, use `--prepare-only --bundle-dir /tmp/pr556-stream-production-bundle`, then run with `--reuse-bundle --bundle-dir /tmp/pr556-stream-production-bundle`. Reuse verifies that both source and bundle fingerprints still match.

`--ascii-smoke` runs the bounded 32 MiB multi-solid UTF-8 boundary functional check through the actual omitted-policy default. `--default-policy --pairs 1` exercises the actual omitted-policy application default with one full 200 MiB source and one competing-preview/Download run. `--smoke` runs an 8 MiB diagnostic only. `--competition-only --pairs 20` runs the larger focused tail experiment; these options affect the harness, not application configuration. The runner fingerprints source before timing, rejects source changes during the experiment, retains individual samples and failures, and performs authored-attribute digest checks outside the measured interval. The [evidence archive](evidence.md) retains the three large raw evidence files as deterministic gzip archives; their uncompressed sizes and SHA-256 hashes are listed in [archives.json](archives.json). After extracting the evidence tree, decompress the selected file before inspecting or comparing it, for example: `gzip -dc docs/issue-556/streaming-source-production.json.gz > streaming-source-production.json`.

Focused regression coverage includes cross-chunk binary/ASCII records and UTF-8, names/CRLF/groups/colors/normals, input and decoded limits, pre-rounding coordinate rejection, source final-validation failure, read settlement after cancellation, finite reorder/ACK windows, shared Download FIFO admission, parent deadline preservation, page ownership normalization, unfinished stream invalidation and existing parked renderer residency.
