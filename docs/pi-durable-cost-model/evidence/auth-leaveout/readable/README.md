# Authentication restoration review evidence

Measured restoration: `a3c116fe91d7689d86c5009c7b563289d3cd34cc` → `b8ce4cb649b96affcfcfa22d9f1447d976f8a219`. The complete diagnostic tree
is `8a24bc99a173ca14899c2721094e683c6e6acca1`. Its follow-up supplies the missing test-only differential helper;
it was **not** the benchmark build. The benchmark library profile disables cfg(test).
No complete diagnostic-variant FULL pass is claimed.

The three patches are readable and independently applicable to their named bases
with `git apply --unidiff-zero`. Zero context avoids whitespace-only patch context
being mistaken for trailing whitespace in this evidence file. The measured and
complete restoration both retain the RawValue parser repair. Build identity includes
compiler, flags, source and harness hashes, binary checksum and build times.

`formal-acceptance.log` records all 36 samples; `paired-oracle-results.json`
records all 18 paired comparisons with original file hashes. The
36 product test logs are included directly. Full ordered request evidence and
provider/process records remain in the parent directory's original evidence archive.
This readable subset does not replace the archive or claim independent raw review.
Standalone baseline lexical validation is linked from
[the correctness evidence](../../rawvalue-correctness/manifest.json).
