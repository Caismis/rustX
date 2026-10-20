# Diagnostic follow-up: browser storage stop and offline libc resolution

These are two separate observations. The browser replay stopped during environment setup and produced no browser performance sample. Offline Native symbolization used existing profiles only; it did not run, rebuild or change a product. Neither establishes the cause of the earlier geometry regression, and no profiler overhead estimate follows from the four unmatched Native control/profile products.

## Single reviewed browser attempt: incomplete before browser start

Run identity: `geometry-short-large-cef4d78a-20261020T051249Z`. Reviewed fixture commit: `cef4d78ad234b8cf60acb563a5e42bc0263ea0bf`. Source: geometry `89d6b618eeba8afaf4b95af4406057330baa60a6`. Preserved inspector-host SHA-256: `fdfc4d0d9bf53d2a6796527486ddaa27a74ac82b4c65c276204b309ae842af30`; native library SHA-256: `c72106cd7f3b0a1b8c930b2d5f109110e1c88c8bc8b362ca934555190d9aebbd`. Existing common frontend and the pinned browser image were verified. The unchanged case is 64 warm-up plus 32 measured turns, with the same fake provider, input and original timeout values.

One fresh private output directory was used. There was one launch, no rebuild, matrix, retry, compression or profiling overlap. An external 600-second watchdog also sampled shared backing-store availability every second and enforced the strict 5 GiB guard. The guard fired first. This is an environment/setup interruption, not evidence that the original browser failure reproduced or disappeared.

| Observed boundary | UTC / value |
|---|---|
| Fixture Node launch |2026-10-20T05:13:01.381543Z; PID 37616, starttime 1479388 ticks|
| Available at launch |9,472,839,680 bytes on the single workspace/Docker backing filesystem|
| Owned container creation |2026-10-20T05:13:10.488409975Z|
| Disk watchdog SIGTERM |2026-10-20T05:13:34.581151Z; elapsed 33.197744 s; 5,341,106,176 bytes available|
| Fixture Node exit observed |2026-10-20T05:13:35.588564Z; return code -15|
| Subsequent availability observation |4,388,581,376 bytes; creation storage continued to grow after the caller was terminated|
| Owned-container removal completed |2026-10-20T05:14:03.698315Z; docker rm -f returned 0|
| Availability after removal |9,471,385,600 bytes|

Container `53eb63192e4b90a01011f4df604c5a0fd9e9b91b62645f4c27321d2411877ca5`, name `pr485-inspector-37616-1792473190432`, was inspected as **Created**, PID 0, Running=false, StartedAt zero. It never started. Its state ExitCode=0 is not a successful process exit. Browser connection, workload turns, WebSocket boundaries, visible-row completion and all-loaded wire-repair barriers were not reached or captured. The free-space drop occurred during Docker vfs container preparation and reversed after removal; exclusive VFS bytes were not directly measured. A read-only `du /var/lib/docker/vfs` was denied by filesystem permissions; no escalation was attempted. Docker API state and backing-filesystem counters were available.

The final `diagnostics.json`, `sample.json` and `failure.json` are all absent. The abrupt signal prevented the fixture's in-memory diagnostics and cleanup path from completing. External snapshots captured only fixture Node; descendant discovery did not supply Native/provider/preview rows. This missing evidence is disclosed, not interpreted as absence of their activity or exit. Fixture wait4 was user 4.021278 s/system 1.154152 s (potential waited-descendant scope is stated in JSON). The last Node proc sample was 5.13 s CPU. Native PID 37628, starttime 1479743, was subsequently observed as a matching zombie with 0.72 s cumulative CPU and raw proc exit_code field 0; this is a post-stop proc observation, not a parent wait4 record. Provider/preview final outcomes and browser/server process counters are unavailable. The historical Docker-events query returned no entries; it is not used as evidence that no events occurred.

Raw capture remains private under the recorded run directory. Fixture stdout was empty; stderr was 178 bytes and identified as a Node experimental warning with no sensitive-pattern matches. No browser console buffer exists. Raw fixture metadata, console or arbitrary close-reason text is not included in this public deliverable. Reviewed instrumentation limitations still apply: metadata extraction precedes timestamps, observers add cost, 50,000-event buffers retain the beginning, and missing terminal records cannot prove a process stayed alive. No dropped-count value is available because diagnostics.json was never written.

The original failure file remains unchanged at SHA-256 `c2c4e57116b767ab6808c4ee299976d016d19425fa7deed84484f6441f23cddc`. Reviewed fixture and preserved binary hashes were reverified after the attempt. No original sample was overwritten. See `browser-replay-summary.json` for the exact argv, timing windows, identity checks and missing observations; `run-browser-diagnostic-once.py` is the exact executed outer wrapper.

The browser task is blocked on storage headroom during container preparation. Current roughly 8.8 GiB available cannot retain the 5 GiB reserve through the observed allocation window. The narrow next step is additional persistent headroom, honoring the original 16 GiB planning target, before a separately authorized single-case retry. No automatic retry or alternative Docker storage configuration was attempted.

## Exact-build-ID offline libc leaf resolution

The libc preserved by both original builds matches the current bytes: SHA-256 `fa430b8f298f817a266046af84a77533185ad6fc4406c7d3787b5a0a0c207826`, build-ID `c495b62edadd6c356265942ec1282d98058a7b41`, Debian libc6 `2.41-12+deb13u3` amd64. The matching debug ELF has that same build-ID and SHA-256 `183cfa015bb32959c19e41514b708f5de9a585f69601113000ee85603773a813`. Identity checks compare the earlier saved library records, preserved bytes, current file and debug ELF.

Official Debian debuginfod and current Debian package mirrors returned 404 for this superseded build. The [official Debian Snapshot API](https://snapshot.debian.org/mr/binary/libc6-dbg/2.41-12%2Bdeb13u3/binfiles) supplied the amd64 archive hash `a9db5c76b23e2a1617997a10f07796b38bc3286d`. The matching [snapshot package](https://snapshot.debian.org/file/a9db5c76b23e2a1617997a10f07796b38bc3286d) downloaded successfully: 7,394,092 bytes, SHA-256 `5a6ab1e991c0240ddc7faade86c571601d136968ecb0d1ef7c88d018f11826a7`. Only its exact build-ID debug member was extracted; nothing was installed or substituted at runtime. Package, debug ELF, original libc, hashes, failed lookups and commands are retained. `readelf` warns that the detached debug ELF has no interpreter name; its build-ID was read successfully and matched.

Raw profile PCs were relocated through their recorded libc executable mapping and its matching ELF PT_LOAD segment (file offset and virtual address 0x28000). Existing GNU addr2line resolved each address against the matching debug ELF, retaining inline frame chains and source locations. No nearest-export name was guessed. The original profile bytes and denominators are unchanged.

| Existing steady profile | Total weighted samples | Previously unresolved leaves | Exact libc leaves resolved | Remaining unknown leaves |
|---|---:|---:|---:|---:|
|baseline|733|299 (40.79%)|296 (40.38% of all samples)|3 (0.4093%)|
|geometry|710|263 (37.04%)|261 (36.76% of all samples)|2 (0.2817%)|

All libc leaves resolved; the remaining 3/2 leaves have no matching file mapping and remain unknown. Sampling loss is still unavailable, not zero. Debug symbols improve names, not signal sampling coverage or statistical precision.

Selected flat leaf counts, baseline/geometry: `__syscall_cancel_arch` 61/56; `_int_malloc` 36/23; `__GI___libc_malloc` 13/26; `malloc_consolidate` 20/15; `__memcpy_avx512_unaligned_erms` 21/21; `syscall` 31/18; `__memcmp_evex_movbe` 15/13. Leaves whose innermost location is in malloc sources total 133/733 (18.14%) and 115/710 (16.20%). This location-based count excludes allocator-related locks located elsewhere; it is not allocation count, retained memory or exclusive wall time. Signal delivery at a syscall wrapper also does not measure syscall count or blocked duration. Full address rows, all functions and inline locations are in `libc/address-resolution.json` and `libc/resolved-libc-functions.csv`.

Self-contained offline reproduction, with products stopped:

```sh
python3 resolve-libc-leaves.py --profiles profiles --output libc
```

The archive includes both existing steady profiles, the earlier pprof row data, exact original libc and debug package/ELF. Reproduction does not execute them. This follow-up preserves the earlier inclusive hotspots (publication encoding about 23%, SQL prepare about 19–21%); those overlapping shares must not be added to flat libc shares. Full lifecycle CPU remains separate from steady turns because warm-up and verification were materially variable. No profiler overhead is inferred.

## Narrow structural hypothesis, not an established optimization

There is one concrete common-path ownership hypothesis worth reviewing: `PublicationDraft::encode` builds a ProjectionMetadata value, then clones it unconditionally into the initial ProjectionTransition while retaining the original for the fallback replacement. A future change could move the owned metadata into the initial event and recover that same value only when constructing the fallback, removing that extra clone while preserving serialized fields, limits, event ordering and fallback semantics. The exact `projection.rs` blob is identical in baseline and geometry (`90347d0436d1018710476fe75fb0160aed317f50`). See [the geometry source at the clone](https://github.com/Caismis/rustX/blob/89d6b618eeba8afaf4b95af4406057330baa60a6/src/runtime_client/projection.rs#L542).

The hot encode path and allocator leaves make this a testable structural hypothesis. They do not quantify this particular clone, establish a speedup, or explain geometry's regression. No patch was implemented. It would need its own narrow commit, semantic/byte-equivalence checks and independent review before measurement. Generic advice to add a SQL cache is not supported: the inspected fact spool and index operations already use prepare_cached. Earlier rejected packing/SQL probes remain excluded and unmerged.

No further Native product run is requested by this evidence. If the coordinator later chooses to reproduce the steady regression, the small predeclared design in `native-reproduction-plan.txt` uses four serial unprofiled products in ABBA order on existing binaries and the unchanged one-case workload; it has not been launched.

Main and PR #485 remain untouched and unmerged. Pi remains deferred.
