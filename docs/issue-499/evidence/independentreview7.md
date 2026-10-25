# Independent review 7 — chat upload fixture admission

Verdict: **approve the exact one-file test-only follow-up; no actionable finding.** I reviewed the actual diff and relevant production/wire-probe paths read-only. I authored none of this patch and did not launch its browser/typecheck commands. This source approval is separate from complete acceptance and final-head CI.

## Exact identity

Worktree `/home/capi/.codex/worktrees/issue-499/rustX`, branch `codex/issue-499-cache-aware-mcp`; previous committed HEAD `5aece5c6bfc81d3df6fd8afb9bc71f4a5d6338b0`; base `00978bbe3072b211efe835950f7f99666e1cc02d`. The only changed executable/test input is `web-console/test/e2e/chat.spec.ts`, SHA-256 `5ba9fb0c9c6a2cd7264647f11b488cd8c944b985b0187758456853b7089efcab`.

I independently recomputed all current bytes, sorted path inventories and aggregate hashes against Git anchors and `/tmp/issue499/review7-source.json`; HEAD/base ancestry, origin/main and `git diff --check` pass:

| Inventory | Count | SHA-256 |
| --- | ---: | --- |
| Native, unchanged | 1012 | `ac3d1c568596d75dc6cc6982f6a4ed23d8097bac0cec08db711e7b4147244bf8` |
| Current delta | 1 | `941896f53bcfd54b8abf0784ea980244ccc2bc154a09b2607b5bb0399433d82e` |
| Cumulative repair from 5a86a563 | 61 | `31856536d7c7badbc695a3a01cd23a2b5c2c7cd1934b79152cf3a5e7c3016055` |
| Cumulative follow-up from d2a5aff5 | 29 | `42fa7ef7e5953c3f22f0873b961dd4c080dc08a8d84cf7df16b18d9a8be90fc7` |

## Failure diagnosis and correction

The preceding complete run remains **414 passed /1 failed**. My separate trace diagnosis, `/tmp/issue499/review6-chat-failure-independent.md`, binds original trace SHA-256 `94c3522c8072f6c72e28b1f9a868a09f0ad67827f0a51b70df263dc9e65d18b7`. The test selected note.txt during replacement attachment, with the hidden picker disabled at the action snapshot; no upload RPC/chip appeared, and the subsequent native input contained text only. Connected and retained historical messages were insufficient proof of current attachment controls. This evidence does not establish accepted files being discarded by the production intake, and does not make that failed complete command flaky or green.

The patch uses wire.attachedTarget(id), whose state belongs to the current physical browser socket and resolves only after the real newest Attach ACK is delivered. It waits for Subscribe with the same full target and then the picker enabled before injecting files. This scenario holds/loses no Subscribe reply, so observing that exact response is a valid barrier here. A ready note.txt chip and native uploadStatus ready outcome with the expected Session/file are then required before Send. The dispatched turn's upload block must equal that exact native receipt. No target authority or upload result is manufactured.

The original provider response, canonical uploaded note, reconnect persistence, image/tool rendering, decode/lightbox, URL ownership/lifetime and cleanup assertions remain in place. The change adds no helper, production path, sleep, timeout extension, skip or swallowed failure. Existing cold-target tests continue to cover pre-Attach local input; this history/image test now accurately admits its already-attached upload scenario.

## Execution scope

I inspected the actual final standalone receipt/log: `review7-chat-browser.result.json`, command `pnpm --dir web-console test:e2e chat.spec.ts`, exit 0, 49.22350119496696 seconds wrapper wall time, log SHA-256 `0e2f1188d296d38736f968a83887b7264b466c130fdd40ecfbdbc5dd325b9a99`. Actual log ends **1 passed (41.4s)**, with the individual case 33.2s. These distinct timing scopes must not be conflated. I verified the log bytes against the receipt, rather than running it myself.

The passing standalone case does not replace the failed **414/1** complete run. A new exact-source complete **415-case** browser run and final-head/base/merge FULL CI/Merge verification remain pending. Native/TUI/measurement and prior review6 Web production source are unchanged and retain only their exact-source earlier evidence. No merge/auto-merge approval is implied.
