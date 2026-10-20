# Read-only storage attribution and metadata ownership review

No artifacts, build caches, images, containers or failures were deleted or deduplicated. No container, browser, benchmark, build or candidate source change was launched. Only read-only observations and small report files were produced. The current 5 GiB guard remains unchanged.

Current usable capacity is 9,414,164,480 bytes (8.768 GiB), on the same backing filesystem for `/workspace` and Docker `/var/lib/docker`. The filesystem total is 33,770,192,896 bytes. Docker 28.4.0 reports driver **vfs**, one pinned Playwright image, zero containers, zero local volumes and zero build cache. There is no leftover container to reclaim. Root-owned VFS directories remain unreadable to direct du; no permission or daemon configuration change was attempted.

## What Docker preparation copies

The pinned image's logical size is 2,505,762,089 bytes (2.334 GiB), including Chromium/headless shell, Firefox, WebKit, Node and Ubuntu dependencies. Existing host node_modules is bind-mounted read-only; it is not copied into this image. Existing Rust inspector-host and benchmark binaries also remain on the host.

The [VFS driver documentation](https://docs.docker.com/engine/storage/drivers/vfs-driver/) describes copying parent-layer contents when creating a layer. Matching Moby 28.4.0 source makes the actual chain explicit:

1. [Normal container creation](https://github.com/moby/moby/blob/v28.4.0/daemon/create.go#L213) supplies setupInitLayer to the image service.
2. [CreateRWLayer and initMount](https://github.com/moby/moby/blob/v28.4.0/layer/layer_store.go#L439) create the internal init filesystem layer, then the container writable layer from that initialized parent.
3. [VFS create](https://github.com/moby/moby/blob/v28.4.0/daemon/graphdriver/vfs/driver.go#L149) copies the parent directory, using [content copying on Linux](https://github.com/moby/moby/blob/v28.4.0/daemon/graphdriver/vfs/copy_linux.go).

Thus this preparation makes approximately **two complete root-filesystem copies**, not just a tiny writable delta. The internal filesystem init layer is separate from the fixture's `--init` process flag. Removing that flag or making the container read-only does not remove this creation chain. Changing storage drivers or constructing a smaller browser image would change the reviewed environment and is not the minimal plan.

Two times image logical size is **5,011,524,178 bytes** (4.667 GiB). The last attempt's shared-filesystem availability fell by at least **5,084,258,304 bytes** (4.735 GiB) before cleanup restored the space. This is consistent with the two-copy mechanism, including physical-block/metadata overhead. It is not a measured exclusive VFS peak or upper bound: the aborted attempt did not provide a continuously observed full setup peak, and direct root du was unavailable.

The image itself also has seven nonempty history layers. Their logical diff sizes sum to 2.506 GB, but summing cumulative contents for VFS's retained parent copies gives an approximate **8,697,989,021-byte** content model. Whiteouts, deletion, hardlinks, allocated blocks and metadata make that a model, not actual root-directory du. Docker system df reports logical diff size and should not be interpreted as a measurement of all physical VFS copies. Exact history byte counts and model inputs are in `docker-readonly.json`; no image export or new container was used to obtain them.

## Largest task-owned consumers

The inode scan observed 50,139 unique file inodes across 78,806 file paths, no scan errors, 12,824,985,600 allocated file bytes plus 35,762,176 directory bytes under `/workspace/pr485-study`. File accounting deduplicates hardlinks; these rows must not be blindly summed where they share files.

| Consumer | Allocated file bytes | GiB | Treatment |
|---|---:|---:|---|
|evidence/products, original Native/browser raw results|4,755,128,320|4.429|Protected; retain all samples and failures|
|three preserved build bundles combined|3,447,226,368|3.210|Protected exact binaries, libraries, logs and provenance|
|active target compiler/runtime tree|2,612,637,696|2.433|Retain to avoid discarding the existing dependency compilation graph|
|pnpm store, reachable unique file blocks|642,416,640|0.598|571,748,352 bytes shared via hardlinks; deleting the store alone would reclaim only about 70,668,288 file bytes|
|xdg cache|126,705,664|0.118|Optional cache deletion would require explicit approval|
|node-gyp, corepack and temporary caches combined|121,786,368|0.113|Optional cache deletion would require explicit approval|

The node_modules subtree reaches 631,422,976 unique file bytes, of which 563,752,960 are shared; it is already reused by the fixture and cannot be added again to pnpm-store occupancy. Counting independent du totals for those two paths would overstate disk use and reclaimable capacity. `inode-accounting.json` records per-path exclusivity conservatively: links outside the scanned tree are not counted as reclaimable. Directory blocks are excluded from the table's file values.

The original `/workspace/rustX/target/debug` appears as 18,114,830,336 bytes to merged-tree du, but it predates this task and is not a credible 18 GB reclamation opportunity. The root/workspace mount is overlay over a pre-provisioned EROFS lower layer; original files have lower-layer inode identities. Initial preflight df reported only 631,861,248 bytes used while this original tree was already visible. Deleting a lower-layer name hides it through upper whiteouts rather than freeing the immutable lower bytes, as described in [the kernel overlayfs documentation](https://docs.kernel.org/filesystems/overlayfs.html#whiteouts-and-opaque-directories). Its reclaimed-byte estimate is conservatively **zero** and it is outside the task-owned cleanup plan. It has not been changed.

## Reuse and hypothetical cleanup

The existing inspector-host, conformance binaries, matching helper executables, shared libraries, common frontend output and installed node_modules can be reused directly at their recorded hashes. The current wrapper already does so. It restores a helper only when the matching target differs, using a temporary staging file; no new persistent per-run binary bundle or rebuild is necessary. Running the same geometry host again does not require copying a fresh 128 MB host executable. The reviewed fixture's small source-provenance copy is approximately 100 KB, not the multi-GB growth. Fresh per-run workspace/SQLite state is necessary to preserve the workload; it cannot be replaced with a prior run's populated database.

A read-only hash comparison verified 12 duplicate alias groups (24 files) among the immutable preserved bundles: each bundle's root conformance/rustx/helper executable and its artifacts counterpart have identical SHA-256 but separate inodes. Keeping both paths on one immutable inode could hypothetically save **863,596,544 bytes** (0.804 GiB). Exact paths/hashes are in `verified-preserved-alias-duplicates.json`. This would mutate artifact storage identity and reduce independent-copy separation, so it requires explicit approval. No hardlinks or replacements were made. Linking mutable compiler targets to preserved evidence is not proposed.

The pnpm-only exclusive bytes plus xdg/node-gyp/corepack/tmp exclusive file bytes total about **319,160,320 bytes** (0.297 GiB), before small directory overhead. Deleting those caches also requires approval. Even that plus all verified immutable-alias deduplication would reclaim only **1,182,756,864 bytes** (1.102 GiB), still **1,751,109,632 bytes short** of the conservative minimum below. Those actions are therefore not an adequate standalone solution.

Deleting the full target could reclaim about 2.61 GB file blocks but would discard compiled dependencies and invite a subsequent compile cycle. It would also require a carefully retained/restored runtime helper/shared-library layout. This is not recommended merely to repeat a browser diagnostic. Removing raw results, originals, preserved variants or the pinned Docker image is not part of the plan.

## Concrete minimal capacity plan

1. Retain every current artifact, cache, failure, image and binary. Reclaimed bytes in the recommended plan: **zero**. Deletion approval required: **none**.
2. Add at least **3 GiB of usable persistent free capacity** to the current backing store. This does not require a new execution environment or recompile if the current volume can be expanded. Account for filesystem reservation: 3 GiB usable is not the same as a 3 GiB nominal disk increase.
3. Before a separately authorized diagnostic replay, require **strictly more than 11.5 GiB available**: 6 GiB planned VFS preparation allowance + 0.5 GiB fresh case/output growth + the unchanged 5 GiB floor. The 6 GiB allowance is a conservative planning estimate above the observed/modelled approximately 4.7 GiB preparation, not a guaranteed measured peak. Current shortfall is 2,933,866,496 bytes (2.732 GiB); adding 3 GiB usable gives approximately 11.768 GiB available. Keep monitoring and stop if the original guard is breached.
4. The stronger original target of **16 GiB available after setup** requires about **7.232 GiB** additional usable headroom instead. Prefer that if capacity provisioning allows it. If expansion is impossible and no specific cleanup mutation is approved, a larger persistent environment is necessary under the current VFS/pinning/retention constraints.
5. If relocating, copy and rehash the existing exact binaries/library/runtime/provenance bundles and reuse the fixed fixture/frontend on a matching x86_64, Node and system-library environment. Restore required helper paths. Do not launch Cargo simply to recreate already verified bytes; a runtime identity/preflight failure should be reported rather than silently rebuilding. New environment measurements cannot be divided by old-machine timings.

No browser rerun is authorized or launched by this report. No deletion is requested as an immediate action. Any later cleanup option needs approval for its concrete paths; the expansion plan avoids that requirement.

## Minimal metadata ownership candidate, not implemented

Inspected exact geometry source `89d6b618eeba8afaf4b95af4406057330baa60a6`; the projection.rs blob `90347d0436d1018710476fe75fb0160aed317f50` is identical to baseline. [PublicationDraft::encode](https://github.com/Caismis/rustX/blob/89d6b618eeba8afaf4b95af4406057330baa60a6/src/runtime_client/projection.rs#L542) first constructs owned ProjectionMetadata from checkpoint read_cut/collections/content, then clones it into the provisional ProjectionTransition while retaining the original for fallback.

The minimal candidate removes only that second whole-metadata clone:

```text
construct metadata exactly as today
move metadata into the provisional ProjectionTransition
stamp cursors and attempt encoding exactly as today
if replacement is required:
    compute event_kind before consuming the provisional event
    recover its owned metadata by destructuring that ProjectionTransition
    construct the existing CheckpointAdvanced fallback with the recovered metadata
continue the existing bounded encoding/error/EncodedPublication path
```

The key proof is [set_checkpoint_transition_cursor](https://github.com/Caismis/rustX/blob/89d6b618eeba8afaf4b95af4406057330baa60a6/src/runtime_client/projection.rs#L7669): it updates nested checkpoint cursors through the events vector, does not modify the outer metadata, and does not replace the outer variant. encoded_event_bytes borrows the event. Therefore the metadata recovered from that event has the same value as the original reserved copy used by current fallback code. Compute event_kind before the move, and keep the invariant localized; no general fallback/helper/cache abstraction is needed.

Keep the existing initial checkpoint-to-metadata clones: fallback clears checkpoint content/collections while outer metadata must retain those values. Do not move those fields out of checkpoint. Keep cursor stamping order, forced-fallback reasons (fitted/captured_eviction/replacement), provisional size failure, replacement content/collection clearing, structural integer encoding, budgets and final error behavior unchanged. Only allocation/refcount/drop timing should differ; this does not change source geometry, transport packing or the wire protocol.

If implemented later, compare complete serialized event bytes/metadata/bytes counts for normal admission; each forced fallback; a provisional-over-budget fallback; and final over-budget error. Verify nested checkpoint cursor updates and nonempty metadata collections/content. Existing prepared publication/trace/settlement coverage should still pass. This report does not claim those candidate checks passed: source was not edited, candidate was not compiled and no new timing was run. A later implementation needs its own narrow commit and independent review before measurement. The sampled encode/allocator hotspots motivate this common-path hypothesis, not a quantified speedup or a geometry regression explanation.
