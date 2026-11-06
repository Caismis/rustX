# Memory ownership and actual-write evidence

Production: **b0ac8ed4f06ef15c6da52438eee06a4e00fd7737** (schema 94). Baseline: preserved O3/schema 93 production 1039cc44d, documentation 1c947513f. The initial 2-KiB/clustered-header candidate 2a8c72166 is a separate cohort, including its adverse page-density outcomes.

[Report](../../2026-11-03-memory-writes.md) · [All values](../../2026-11-03-memory-writes-tables.md).

Archive: **83,082,524 bytes**, **7,270 regular members**, SHA256 **5d1af2e7d55d04f3d7a153717bf28b061189008d55273d15842546d4ccdd681e**. The manifest authenticates every included regular member and the archive. `archive-selection.json` describes omissions: live runtime/workspaces, ephemeral tokens/environment files, private diagnostic checkout, compiled executables/libraries, Python caches and duplicate provider stdout. Original reports, consistent SQLite backups, profiling exports, counter/oracle records, commands, source patches, frozen benchmark inputs, build/check identities and failed attempts remain.

Successful offline verification is recorded in [replay-result.json](replay-result.json): all 7,270 members authenticate and all tables reproduce.

From this directory, using Python 3 with SQLite/JSON support:

```sh
python3 replay.py --archive raw.tar.xz --manifest manifest.json --output /tmp/rustx-memory-writes-replay.json
```

The output path must be new. Replay extracts into a temporary directory, authenticates all members, reparses both 32-product ordinary cohorts and eight WAL products, and rechecks 23 native products (including diagnostics), 16 non-chat functional products, nine unique complete sampled heap profiles, direct SQLite/replay probes, 16 physical-page backup images, source maps and host observations. Generated tables must match byte for byte. These mixed products are not independent repetitions of one performance scenario.

It executes no benchmark, native product, compiler, git, symbolizer or network. Archived build/source identities and prearchive executable checks remain evidence for excluded binaries; replay cannot freshly authenticate absent executable bytes or rerun symbolization. **Pi and DeepSeek were not rerun.**

Native 128×8 memory endpoints use two balanced ordinary O3/default/glibc pairs and native-server-PID counters only. SQLite and replay-release ownership probes use separate private root-O0/tcmalloc builds. The after release probe additionally records per-field destruction; both drain the whole ring, but destructor ordering, the small stats map and asynchronous cleanup can affect global deltas. Report their 95 untouched-session medians/ranges as approximations, not an exact 925-MiB partition. Wire-byte replay accounting does not represent another retained encoded buffer.

The complete sampled exports use the installed tcmalloc `GetHeapSample` API and 64-KiB sampling; all raw rows sum to each header, and independent Poisson correction agrees with collapsed symbolization weights. Requested estimates, allocator charged bytes and ordinary PSS have separate denominators. Inclusive stack groups overlap. The older capped full-tracking trial is retained as exploratory/partially signalled evidence and is excluded from this complete-profile comparison.

The shared host includes unrelated worktree compilation. Compiler-quiet gates and low-rate observations are recorded, including compiler activity that began during an ordinary block. No unfavorable product or outlier is removed or adjusted. The final native width1 wall/CPU increases and the 6–7% larger main-image growth are retained alongside memory/write reductions. Full repository CI/browser E2E and a main merge are outside this verification.
