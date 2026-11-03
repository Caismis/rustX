# Loaded-session retention evidence, 2026-11-03

[Report](../../2026-11-03-idle-retention.md) · [Complete tables](../../2026-11-03-idle-retention-tables.md).

The archives contain only this round's Rust products. Root paths are the f2e trial; `refinement/` is the final d501 production cohort. Heap/control diagnostics use the trial ELF and are not pooled with final ordinary timing. Pi/DeepSeek were not run; the prior Pi memory number is a stored authenticated reference.

| Archive | Bytes | SHA256 |
| --- | --- | --- |
| trial.tar.xz | 65899704 | `a7d3ddb1f8af1824613d5a8825a8e1139727f43d7e9462dd4b8a9e3a9cabcb1f` |
| final.tar.xz | 45544328 | `655d58667bf05661d6f7e7104118f679aa3032f47ef4404d0be863fe08af5316` |

`manifest.json` lists every selected path, byte count and digest. `replay-result.json` records the actual successful portable replay. `archive-selection.json` inside the trial archive lists exclusions. Failed/partial attempts are retained, not treated as accepted timing samples. Both stages have 3,256 unique passed tests /three existing ignored.

The archived tables reproduce byte for byte. The published Markdown removes the sole extra blank line at EOF; replay records both the archived and normalized publication digests.

Run from the repository root with Python 3 and its standard library:

```sh
python3 docs/pi-durable-cost-model/evidence/idle-retention-20261103/replay.py --directory docs/pi-durable-cost-model/evidence/idle-retention-20261103 --output /tmp/rustx-idle-retention-replay.json
```

The output path must not exist. Replay authenticates both archives and every selected member, independently reparses full HTTP/native histories and SQLite images, raw PID counters/cache rotations/Source reads, functional reopen results, complete heap sample denominators, allocator-return experiment, source/build/check maps and host observations, then reproduces every table byte. It executes only archived Python audits/arithmetic, never products, builds, git, symbolizers or network. It uses stored prearchive byte proofs for excluded executables/libraries/helpers, not fresh access to them.

Resource boundaries: anonymous PSS/RSS are ordinary-glibc process counters; sampled requested bytes and tcmalloc allocated/free bytes are separate diagnostic denominators. Inclusive stack categories overlap. At 32 s all 128 sessions remain at exactly 8 turns; at 130 s four have 14 turns and 124 have 8. Cold latency covers 122 predeclared eligible sessions; CPU/write windows cover all 128 calls. Mixed 98 s CPU includes idle maintenance. All tails/adverse paired cells remain.
