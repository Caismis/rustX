# Hover-preview diff Copy restoration and CI follow-up

Checked on 2026-11-10 for PR 517, review comment [6761761816](https://github.com/Caismis/rustX/pull/517#issuecomment-6761761816). Main integration removed the file-change hover preview's Copy action even though review comment [6600307712](https://github.com/Caismis/rustX/pull/517#issuecomment-6600307712) explicitly requested it. The preview now has a localized, keyboard-operable Copy button with copied feedback. It reuses the existing diff serialization and clipboard-feedback paths, copies the exact text admitted for preview, and works while the preview is open. Copy remains available in full review.

The preview retains `PreviewDiff` and Floating UI positioning. Lazy reading and preview admission remain bounded at 128 KiB and 2,000 lines, with the truncation notice. The header has localized `Copy`/`已复制` text and reserved button width; path and diff scrolling remain usable.

## Validation

- The 16 component tests and TypeScript check passed.
- The complete Web/Host Vitest suite passed: 4,780 tests, 4 skips, across 327 passed files and one skipped file, in 166.50 seconds.
- Final TypeScript, 349 source-provenance records, i18n, production build, production-fixture build, geometry-runtime assets and the 2,048 MiB WASM memory-maximum audit, 356 package notices, and 357 provider icons all passed.
- The pinned workspace-changes suite passed eight tests in 49.8 seconds. Its four long-preview cases cover real clipboard writes from Enter, Space and click in English/Chinese and light/dark themes, each at 1214 px and 390 px. Selection, path/diff scrolling and existing full-review/card references also pass. Two preview cases were rerun after a serialization-efficiency cleanup in 23.4 seconds and produced four review images. The native Workflow case that was active when its CI job was canceled also passed in 52 seconds: nine distinct browser tests in total.
- The exact-image-cache run used Playwright 1.63.0, Chrome 153.0.8010.12, and image digest `bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`. The image root filesystem was read-only and had no host fonts; strict baselines and deadlines remained unchanged.
- Python's 45 planning tests and Vitest's 27 browser-verification tests passed.

The full Web/Host suite is complete locally. Browser shard 2 of CI run 42775024217 was canceled at its 45-minute job limit (started 07:30:19 UTC; browser tests began 07:32:38; canceled 08:15:31). No browser assertion failure was logged before cancellation. The finalized source revision still needs its own remote CI run.

## Watcher CI follow-up

CI 42775024217 exposed a macOS failure in `watch_identity_recheck_survives_atomic_write_lock_contention` after 180 seconds at `review.rs:290`. The test had assumed a post-contention native callback must contain changed paths. Under valid lock contention the callback instead reports a resync with no paths.

The test-only hook observes publication of the same-directory observer callback after the file observer physically settles. The test requires `resync_required=true` and empty `changed_paths`, releases the environment I/O lock, then checks authoritative `FsReadFile` bytes. Capacity, lock and unwatch assertions remain. Watcher release behavior is unchanged; instrumentation is `cfg(test)`. The focused Linux all-feature test passed (1 test, 2.41 seconds) with proxy variables unset; formatting passed. The 23 relevant environment-group tests passed under the inherited proxy settings; only the separate credential case was rerun with proxy variables cleared. macOS verification remains for remote CI; no local macOS pass is claimed.

## CI planning evidence and remaining gates

Prior complete-run weights came from [workflow 42743604185](https://github.com/Caismis/rustX/actions/runs/42743604185), which completed 766 cases in 151 specs in both reports (including nine visual failures). The report SHA-256 values were `8871738a08750cf0e4bf2a3579c30a1ecb05b1590b071f0a12b2a6838a82df9c` and `1cc65451893c03bda955ae800e15f68017e44970286b02456b9a78c811a02754`. Its cost reports are [13731922579](https://github.com/Caismis/rustX/actions/runs/42743604185/artifacts/13731922579) and [13732632481](https://github.com/Caismis/rustX/actions/runs/42743604185/artifacts/13732632481).

The current Playwright discovery verified 768 cases across 151 specs assigned exactly once: shard 1 has 334 cases across 51 files, shard 2 has 220 across 50 files, and shard 3 has 214 across 50 files. The plan uses three weighted whole-file shards under the unchanged 45-minute job budget. The prior complete-run weight replay projected loads of 1492.9, 1493.6 and 1494.3 seconds; these are estimates, not observed timings or a new CI speedup. FAST remains one shard, or two for presentation changes. Remote CI for this exact source revision and macOS watcher verification remain open. The PR remains Draft and performance gates remain open.
