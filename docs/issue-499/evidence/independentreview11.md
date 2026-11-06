# Review11 independent wire-probe source review

Verdict: approved for the stated fixture contract; no actionable findings in the reviewed files.

Reviewer `/root/final_architecture_review` did not author either file and made no repository edits. Reviewed from worktree `/home/capi/.codex/worktrees/issue-499/rustX`, based on committed head `8752fbb48518d6837fbbc3ec9df74873ea8160f3` plus the following frozen fixture changes:

| File | SHA256 |
| --- | --- |
| `web-console/test/e2e/wire-probe.ts` | `1996207162e2965657202fed9219ff64dbc7be79f4b9c373cca29bd17cdabfc9` |
| `web-console/test/wire-probe.test.ts` | `9483f6f77ccf3f9eb8abb0465b0b671d76e77c977573afa160b5dbb8311703ce` |

Two-file identity digest: `684ce8a24c6219856f12bc41be4bba3c3045d48b85842d72f511b71ee82fa407`. Recipe: SHA256 of UTF-8 concatenation of sorted `path + NUL + file_sha256 + newline` records. The unchanged `web-console/test/e2e/commands.spec.ts` remains `45d94c559c40098031bb6619b441036d8e79d2cd1f96f157158050a2a9f38252`.

Source review covered separate Browser-owned outstanding versus actually forwarded ordinary requests; one active probe and six total pending probes; withholding only the sixth Browser request displaced by a probe; forwarding the Browser's own seventh request with or without a probe so Native refusal remains visible; raw Native result/error releasing occupancy even when the Browser ACK is held; Browser backlog priority before another probe; shared-policy classification and unknown-method pass-through; per-physical-route retirement, rejected probe owners, dropped unforwarded work and late-frame/held-ACK fencing; original error propagation and absence of retry or synthetic semantic results. No production or Native capacity changes are part of this repair.

Independent execution: `pnpm --dir web-console exec vitest run test/wire-probe.test.ts`, exit 0, 1 file / 19 tests passed. Started `2026-10-24T15:22:41.038562+00:00`, completed `2026-10-24T15:22:42.973040+00:00`; wrapper wall time 1.934s, Vitest reported 426ms. Reviewed file hashes and unchanged commands fixture were identical before and after execution. Log: `/tmp/issue499/review11-independent-wire-probe-unit.log`, SHA256 `9c58580445e7ca13a5f832ff8e5b6141ce15413c30629b8b50df000d38c90620`. Machine receipt: `/tmp/issue499/review11-independent-wire-probe-unit.result.json`.

The tests invoke the actual helper and fake only Page/WebSocket-route callbacks, with manually driven outgoing frames and Native responses; they do not implement a second admission algorithm. They use no sleeps or timers as ordering proof.

Scope and limits: temporarily withholding the extra ordinary Browser frame can change its arrival order relative to directly forwarded control/metadata/attachment/observation frames. For example, `delivery/cancel` or `turn/cancel` can reach Native before that temporarily withheld ordinary request. This helper is not evidence that Native observed a request before cancellation, nor proof of physical initialization/process cleanup. The unit suite proves fixture forwarding, slot ownership, bounded queues, retirement and unchanged raw-result handling only. Native admission/cancellation/physical settlement remain covered by their independent native owner/boundary suites. This reviewer did not run Cargo, full Web validation, or real-browser acceptance for this repair; those remain the root's validation responsibility. Historical CI1527 failed and is not superseded by the unit result.
