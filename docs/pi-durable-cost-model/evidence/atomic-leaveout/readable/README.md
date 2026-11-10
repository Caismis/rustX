# Readable separate-final leave-out evidence

This is an evidence-only view of the existing measurement, not another run.
The original archive and its manifest are unchanged.

- `paired-samples.json` is the byte-identical original full derived summary:
  all three pairs for each of six workloads, both sides, lifecycle phase deltas,
  ratios and medians. `paired-samples.csv` exposes the nine headline metrics for
  each pair (162 rows). Run `python3 recompute.py` here to recompute every ratio
  and every published before/after and paired-ratio median.
- `production-variant.patch` is the exact full diff from public full candidate
  `a3c116fe91d7689d86c5009c7b563289d3cd34cc` to diagnostic `7295a1d5`.
  It includes the production changes, removal/restoration of the corresponding
  atomic/separate-cut tests, and diagnostic note. Tests under `src` are not
  concealed or presented as production-only code. It retains bounded cadence
  and canonical tool proposal projection, restores separate ordinary-final P/U/C
  commits, and uses diagnostic gate 77.
- `checksum-fixture-follow-up.patch` is the separate exact `7295a1d5` →
  `13e09b13c2a8b19455cdbbf6a23b693a066126ad` test-fixture correction.
  Apply both patches in that order with `git apply --unidiff-zero` on the public
  base. No remote diagnostic commit is needed. `variant-proof.json` records
  exact full-tree equality after applying each patch through a temporary index.
- `build-identity.json` records the actual measured head, source tree, fixture
  hashes, compiler/flags, commands and executable SHA. `build.log` is the
  existing successful optimized conformance build log.
- `product-tests.log` contains the 36 existing passing conformance sample logs;
  `formal-acceptance.log` contains their controller acceptance results.
  `test-proof.json` records original log hashes and 18 ordered paired-oracle
  results. Both sides here are Rust revisions; historical comparator field
  names do not indicate a Pi run. No diagnostic-variant FULL, client suite or
  separately executed recovery suite is claimed. The native recovery/test diffs
  are exposed for independent parity review.

Before is separate finalization, after is the full atomic candidate. Ratios are
medians of three within-pair after/before ratios, not ratios of unrelated medians.
CPU includes driver/self threads with 10 ms ticks, memory is sampled RSS/PSS,
and process writes are not physical-device writes. Lifecycle and adverse cases
remain intact. See the parent report for full scope and interpretation.

Readable log copies omit trailing blank lines; original logs remain byte-identical
in the unchanged archive. The manifest hashes every readable file and records the
original full-summary hash. This subset does not replace full ordered raw evidence.
