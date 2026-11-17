# Joint runtime optimization evidence

`raw.tar.xz` contains the completed joint-runtime round and every retained failed
and successful correctness queue. The formal matrices contain 72 direct revision
runs, 72 fresh Pi runs, and 36 three-engine runs. Logical-column and CPU/control
runs are separate diagnostics. An optional confirmation cohort is included in
full when present; its existence is never assumed.

`summary.json` records the archive SHA256, sizes and member verification result.
The complete per-file hash/size/source manifest and exact exclusion inventory
are inside `MANIFEST.json`. No compiled product is included.

Extract into an empty directory and run both retained independent auditors:

```sh
mkdir raw
tar -xJf raw.tar.xz -C raw
cd raw
python3 audit-joint.py --root . --raw-only --stage all --baseline-identity baseline-build/identity.json --candidate-identity candidate-joint-build/identity.json --freeze deepseek-setup/final-source-identity.json --expected-harness-revision 123accb38fd8cf1e8f88583c1858d514cb453870 --expect-rust-sha256 6899c8fad1fc85e8ffd36a33b92e85cec877cda3689bb26a03c6f07c658a3d43 --output ../archive-joint-audit.json
python3 audit-diagnostics.py --root . --raw-only --baseline-identity baseline-build/identity.json --candidate-identity candidate-joint-build/identity.json --formal-audit joint-audit.json --expected-harness-revision 123accb38fd8cf1e8f88583c1858d514cb453870 --expect-rust-sha256 6899c8fad1fc85e8ffd36a33b92e85cec877cda3689bb26a03c6f07c658a3d43 --output ../diagnostic-raw-audit.json
```

The previous G/H baseline identity and complete Cargo output are at
`baseline-build/identity.json` and `baseline-build/build.stdout`; the joint
candidate's are at `candidate-joint-build/`. Pinned native source metadata is at
`deepseek-setup/final-source-identity.json`, with `setup.json` beside it. The
joint auditor, pinned arithmetic helpers and dependency hashes are at the root.
`audit-diagnostics.py` independently checks all four logical-column products
and all four CPU/control products against their captured identities and counts.
The strict reports are `joint-audit.json` and `diagnostic-audit.json`.

The only cohort CLI stdout exclusion is `final-cohorts/three-engine.stdout`;
its raw counters, commands, wire/process/provider logs and stderr remain.
All four direct/Pi CLI stdout files remain because the portable auditor checks
actual execution order from them. Optional confirmation stdout also remains.
Failed check logs, `.cpu` profiles, symbols, and native `.zstd` sessions remain.
Dependency caches, home content outside native sessions, compiled formats,
symlinks and old archives are excluded and enumerated in the manifest.

Raw-only recomputation does not execute products, decompress native session
logs, or follow recorded old executable/Git paths. It checks retained identities
and arithmetic, while marking actual binary/current-source verification false.
The archived compressed native logs support separate external reinspection;
the auditor uses retained decoded events and verifies their file identities.
CPU symbol reports are diagnostic evidence; regenerating them needs the matching
excluded executable. CPU files attest phase names but do not have separate
wall-clock boundary timestamps; logical-column runs retain their phases.json.
The measured DeepSeek SDK idle completion does not await final JSONL flush/fsync;
its shipped headless entry point explicitly awaits session flush after idle.
The SDK cohort is not an equal-durability performance claim.

The delivered archive includes the separate seven-pair confirmation cohort,
for **194 formal products** in total, plus eight diagnostic products.
[Portable verification](portable-verification.json) passed with every payload
member checked, both raw-only audits accepted and all recomputed numerical
results equal to the retained strict reports. No executable was rerun.
