# STEP internal parse experiments

Diagnostic-only work under `/tmp`; no repository, source-offer, shipped JS/WASM or frozen build input changed. Original O3/no-LTO controls and isolated candidates use Emscripten 3.1.69 and the pinned importer/OCCT revisions recorded in [compact summary](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-534/step-parser-experiments/summary.compact.json). The historical run preceded the shipped bulk lexer in `0.0.23-rustx.4`. The tail and allocator candidates remain experimental.

## Where the wait goes

The generated valid 200,761,966-byte referenced B-spline STEP benchmark (no padding) has 4,218,202 records, 12,668,972 parameters and 2,108,378 loaded entities. Three diagnostic reads put STEP ReadStream at 23.04 s, of which scanner/parser is 19.91 s (~86.4%). Record conversion is 0.723 s, preparation 0.519 s, model loading 1.130 s and session/model graph setup 0.698 s. These nested aggregates are inclusive and cannot be summed.

Real 1 ms native-read CPU samples retain WASM function names. Original parse() self is 16.47 s / 57.4%, but it includes inlined scanner/refill/stream operations; no independent yy_get_next_buffer/scanner::lex frame distinguishes those. CreateNewArg has 2.115 s self. With bulk input, parse() self falls to 2.547 s, while CreateNewArg remains 2.095 s. Explicit LexerInput self falls 1.314 s → 2.13 ms, but its frame alone does not account for all per-byte replenishment/state/token-moving work. The causal test changes the whole byte-at-a-time refill path, rather than attributing a single sampled function as the cause.

## Ordinary paired results

Profiling OFF, three fresh-process/fresh-instance pairs per workload; same owned input release, coarse/final policy and full final tolerances. Every ordinary row passes source identity plus final canonical JSON, raw numeric-bit and metadata equality.

| Workload | Tail only: original→candidate ms | Lexer only: original→candidate ms |
| --- | ---: | ---: |
| lod-assembly.step | 144.27→148.66 | 144.53→154.40 |
| bracket-array.step | 297.23→294.18 | 307.36→289.84 |
| pr556-curved-200.step | 4,452.93→4,463.33 | 4,346.46→4,364.38 |
| pr556-large-bspline.step | 28,691.20→27,312.25 | 28,185.92→13,586.14 |

Dense tail-only gains 4.8% but process peak RSS rises 1149.6 → 1268.2 MiB in that matrix, with disjoint observed ranges. Bulk lexer gains 51.8% with the original tail/allocator/grammar untouched. A separate lexer-versus-combined dense matrix gives 13.712 → 11.538 s (another 15.85%); RSS ranges overlap widely, so no combined memory benefit is claimed. All individual ranges and scopes remain in raw JSON.

## Fixed-row scaling

Each valid degree-3 surface has exactly 4 control-net rows and widths N = 2048, 4096, 8192; all poles are referenced. Final output is 50 triangles for every size/runtime, so this is parser/list scaling rather than a large-mesh test.

| Width | Original ms | Tail ms | Lexer ms | Combined ms | Records | Parameters | Derived append-link hops |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2048 | 336.30 | 286.40 | 229.23 | 205.71 | 24,748 | 86,414 | 25,117,036 |
| 4096 | 571.40 | 446.88 | 408.29 | 261.26 | 49,324 | 172,430 | 100,565,356 |
| 8192 | 1317.49 | 789.88 | 946.99 | 438.37 | 98,476 | 344,462 | 402,456,940 |

Width doubling gives roughly 4× derived append traversals while records/parameters grow roughly 2×. Whole-import growth includes fixed initialization/transfer/tessellation overhead and should not be presented as a pure asymptotic parser measurement. Ordinary N→4N time ratios are 3.92× original, 2.76× tail, 4.13× lexer, 2.13× combined.

The dense model has 1,541,068,357 derived append-link hops; 1467 lists of length 1024–2047 account for 1,538,959,752 of them. The counter runs only add counting to the existing conversion walk, never time/count each append-chain hop. Counter clocks are excluded from primary comparisons.

## Allocation evidence

Native malloc/calloc/realloc/free wrappers delegate once, preserve arguments/result/errno and suppress nested libc wrapper double counting. Synthetic positive allocation/zero-fill/reallocation-content and overflow/null/errno checks pass with expected entrypoint counts. The original allocator is retained.

Dense ReadStream sees 61,272,345 outer entry/free calls and 1,964,178,892 requested-byte traffic. This is allocation attempt/reallocation traffic, not unique/live/peak bytes; phase attribution is exclusive. Top traffic: record conversion 883.2 MB, scanner 361.3 MB, graph construction 210.8 MB, entity load 160.4 MB, entity setup 160.2 MB. [Allocation detail](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-534/step-parser-experiments/allocation-traffic.json) maps every phase index to its name and entrypoint counts. The wrapper diagnostic reads 30.484 s versus the preceding unwrapped phase diagnostic 27.697 s (~10% extra); neither is primary optimization timing.

## Correctness and scope

Tail-only 8-case and lexer-only 12-case corpora preserve outcomes, final raw bits and metadata. Lexer cases include successful EOF without newline, CRLF and trailing whitespace. The inherited `valid-buffer-boundary.step` name is nominal: all original/candidate runs reject that 16 KB before-HEADER-comment input, so it establishes failure equivalence only. Positive cross-buffer evidence comes from the separate 24,314-byte semantic case below. Empty/truncated/malformed input remains equivalent; malformed-argument recovery still produces the same complete cube. Separate IGES/BREP controls also remain exact. These do not establish complete ISO grammar coverage.

The [separate semantic diagnostic](semantics/report.md) completes 34 configurations across all four variants (136 ReadStream calls): 18 corpus inputs, eight same-position EOF cases and eight actual injected stream failures. All returned fields agree, including complete raw-value/type/order dual64 digests and independently traversed final `model.Value(1..N)` entity count/type-order digests. Canonical capture is bounded to 64 records, 32,768 total bytes and 256 bytes per value; longer values are fully hashed but only a prefix is captured. These rolling digests are noncryptographic. `entityLoadObservations` is duplicated instrumentation and is not an entity count; authoritative model checks use the independent final traversal.

Nonempty targeted assertions cover empty/nested lists, typed LENGTH_MEASURE, complex entity component order, parameter order and error recovery retaining a later entity. The dedicated successful 24,314-byte case contains an 11,700-byte comment and a 12,191-byte multi-line quoted string crossing the 8192/16384 boundaries, followed by another entity. Eight true stream failures set `faultTriggered`, scanner bad/fail true and eof false, returning IFSelect_RetFail without escaping an exception. Caller-stream flags remain clear because the scanner owns its stream state. Unknown TEST_ENTITY fixtures exercise raw parser preservation rather than full schema validity; rejection with seed hashes is not positive semantic evidence. No complete ISO grammar conformance is claimed.

The official tail patch contains a myLast-null fallback, retained in this experiment. Under the audited pinned constructor/five initialization paths plus first append, it is unreachable; production simplification would require a separate reviewed change. The lexer experiment undefines YY_INTERACTIVE after its generated definition. Defining it to 0 would not disable an #ifdef branch. Its .lex regeneration/source-offer consistency is a production-integration concern, not silently changed here.

## Reproduction and recommendation

Rebuild the unchanged pinned baseline with the repository vendor recipe; retain its compile_commands/build.ninja/effective commands. Scripts here record exact run paths under `/tmp/pr556-*`; adapt only base/output paths on another host. They reuse original compiled objects, recompile copied selected units, and relink to new outputs. Source deltas, diagnostic helpers, the input generator/manifests, command fingerprints and sample JSON are included for review. Experimental JS/WASM and raw CPU profiles are intentionally excluded from this staging archive; hashes/bytes are listed in the manifest.

The clearest established optimization is replacing the byte-at-a-time scanner refill path with its existing noninteractive bulk-read path. Tail pointers address a separate quadratic long-list cost and matter more on the fixed-wide-list case, with an observed dense memory tradeoff. Parser allocator changes remain untested and must not be conflated with either result. Production integration requires separate review, source-generation consistency and memory-budget acceptance. No quality, tolerance, model-completeness, format/size classifier or browser deadline was changed. No weak-device or physical first-image budget is certified.


The 200 MB input is a generator-produced valid referenced surface benchmark, not a user production CAD assembly. The 96-part bracket array is a test assembly. No real production-assembly CPU profile or weak-device/browser first-image validation was collected, and the observed ~52% reduction must not be generalized to all STEP files. The production bulk lexer has separate browser validation. Tail and allocator changes still require memory-budget and representative workload validation.
