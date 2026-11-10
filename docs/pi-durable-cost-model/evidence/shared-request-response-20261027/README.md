# Shared request/response evidence

`raw.tar.xz` contains the completed G/H raw evidence and all recorded failed and
successful check rounds. `summary.json` records the archive SHA256, byte counts,
and verification result. The full per-file SHA256/size/source manifest and exact
exclusion inventory are inside `MANIFEST.json`. No compiled product is included.

[portable-verification.json](portable-verification.json) records the separate
post-packaging extraction, all-member size/hash verification and successful
raw-only formal/diagnostic recomputation. It was produced after the archive and
is intentionally a sidecar, not an archived input.

Extract into an empty directory and run the retained independent auditor:

```sh
mkdir raw
tar -xJf raw.tar.xz -C raw
cd raw
python3 audit-gh.py --root . --raw-only --stage all --baseline-identity baseline-f-build/identity.json --candidate-identity candidate-gh-build/identity.json --freeze deepseek-setup/final-source-identity.json --expected-harness-revision 063ef7a9b713e76efad6dc4e6b7122454023c67f --expect-rust-sha256 cce30715eed4b1700c5b3c5fee2a93db1fb4ca49c8847cf36dfe995e4470e9b0 --output archive-gh-audit.json
```

F's identity and full Cargo build output are at `baseline-f-build/identity.json`
and `baseline-f-build/build.stdout`; G/H's are at `candidate-gh-build/`.
The pinned native source metadata is at
`deepseek-setup/final-source-identity.json` with `setup.json` beside it.
The auditor and both pinned helper scripts are at the archive root.

The only cohort CLI stdout exclusion is `final-cohorts/three-engine.stdout`,
enumerated in the manifest; its raw counters, commands, wire/process/provider
logs and stderr remain. The four direct/Pi CLI stdout files are retained because
the raw-only auditor uses them to verify actual execution order.
The separate confirmation cohort is retained in full, including its stdout,
commands, all 14 products, seven comparisons and independent audit result.
CPU `.cpu` data and native `.zstd` session logs are retained.
Build/check stdout is retained in full. Dependency caches, product-home files
outside native sessions, compiled formats, and old archives are excluded.

Raw-only recomputation does not execute products, decompress session logs, or
follow recorded old executable/Git paths. It checks retained identities and
arithmetic, while explicitly marking actual binary/current-source verification
false. CPU symbol reports remain diagnostic evidence; regenerating those symbols
would require the matching excluded executable. DeepSeek remains an additional
reference with its recorded durability boundary, not an equal-durability claim.
