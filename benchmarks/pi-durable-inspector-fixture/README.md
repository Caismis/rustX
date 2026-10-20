# PR485 active Trace inspector fixture transfer

This branch transfers the existing standalone v3 benchmark fixture unchanged. It is based on main6771928953a9b2815a27fd784f6d39c5ccde8b56 and changes no product code. Do not merge this handoff branch or use its product tree for the Source candidate comparison.

The11files under`v3/` are byte-for-byte copies of the previously exercised local fixture. `fixture-identity.json` is the original SHA-256 manifest. The request oracle and expected-tools JSON also match published623 inputs exactly. Keep all11files, including the fixture README. Keep this transfer README and manifest outside the executable fixture directory so they do not become extra fixture inputs.

Measurement source heads:

-623:623b41b869a95c888eb04f907cbf607232699a2f
-89:89d6b618eeba8afaf4b95af4406057330baa60a6
-316:31668f7a74b63446c538e00ed41ebfc42f0bc1a8
-Pi Durable:90d863f7c1183b965f099855715d6e88c7b8a52c from https://github.com/earendil-works/pi.git

Native controller/build/oracle code is already published at all three Source heads. Pi's local tsconfig.benchmark.json is reconstructible from published benchmarks/runtime/pi-tsconfig.json. No historical binary or raw archive is required to generate a fresh cohort. Historical unique evidence remains preserved separately; this fixture-only commit is not an upload of that evidence.

Reconstruct from a fresh Rust checkout without switching its measured source:

```bash
git fetch origin codex/pr485-benchmark-fixture-handoff-20261020
mkdir -p "$PR485_FIXTURE"
git archive FETCH_HEAD:benchmarks/pi-durable-inspector-fixture/v3 | tar -x -C "$PR485_FIXTURE"
git show FETCH_HEAD:benchmarks/pi-durable-inspector-fixture/fixture-identity.json > "$PR485_BENCH_ROOT/fixture-identity.json"
python3 - <<'VERIFY'
from pathlib import Path
import hashlib,json,os
root=Path(os.environ['PR485_FIXTURE'])
expected=json.loads((Path(os.environ['PR485_BENCH_ROOT'])/'fixture-identity.json').read_text())
actual={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir() if p.is_file()}
assert actual==expected,(actual,expected)
print('All11original fixture hashes match')
VERIFY
```

Use normal optimized freshly built root libraries and matching native helpers. `build-host.py --build <published-build-revision-output> --output <new-host-output>` links the unchanged host to the selected root library. Preserve targets/runtime shared libraries and compiled supervisor paths. Build/measure serially; no build, profiler, tracing or compression overlaps timed products.

The runner command is:

```bash
node --experimental-transform-types "$PR485_FIXTURE/run.mjs" "$PR485_SOURCE" <exact-fresh-inspector-host> <short-small|short-large|details-resync> <new-output-directory>
```

Before each run, write source-identity.json in the measured source checkout with the actual native host's head; this is fixture metadata only. Use the same verified frontend source/bundle across the comparison, with its identity recorded separately. Root production source, source refs, protocol, original UI assertions and all deadlines remain unchanged.

Preflight strictly more than5GiB usable free on both benchmark and Docker-root filesystems before each browser product. The previous2.5GiB attempt failed ENOSPC before browser metrics because it omitted Docker VFS copy cost. Install the pinned browser image during setup. Do not change Docker storage, prune shared images, loosen assertions, skip failures or inflate timeouts.

The full-drain barrier requires every loaded Trace row at the latest Trace obligation revision and exact Source cut, no pending browser RPC, two actual browser paints and unchanged generation. Its poll uses the existing5-second default. It sends no extra Native publication/RPC to manufacture completion. Current-row completion and full-repair endpoints remain separate.

short-small is8warm+32measured; short-large is64warm+32measured, short text in both. The details-resync case holds an actual old-cut detail, appends through the UI, rereads/equates that page, releases it, performs2050append/remove pairs while disconnected, verifies>=4097actual publications and resync_required, then completes128text deltas over512SSE gates. See the unchanged fixture README and code for exact policy and oracles.

The earlier10-product browser driver used2alternating pairs each for short-small/short-large and1details-resync pair. That is diagnostic; fixture README requires3alternating pairs for formal runs. Keep fresh Source comparisons and old cohorts separate. No Rust/Pi browser-equivalence or parity claim follows.

Use Rust1.98.1,Node24.19.0,pnpm11.13.1 and the pinned Playwright image embedded in run.mjs. Shared provider/model-facing time remains2026-10-19 00:00:00. No real model APIs/credentials or model-data hydration. Capture raw native/browser counters, held-detail/resync memory, full request/canonical/reopen oracles and every failure. Process writes are not physical-media writes; sampled memory is not peak memory. Archived semantic-reader flags remain a disclosed trust limitation.

This transfer was explicitly authorized solely to move the existing requested benchmark implementation to a new saved environment. No #485/main update, merge, force push, reset, original-writer takeover, model API use or Library401bypass is authorized by it.

Validation note: the default git diff --check reports existing final blank lines in canonical_history.rs, common.rs and verify_requests.rs. They are intentionally retained to preserve all original SHA-256 values. A read-only check excluding only blank-at-eof passes; no repository whitespace rule or product assertion was changed. Python sources were AST-parsed and JavaScript sources syntax-checked; no heavy build or benchmark was run for this transfer.
