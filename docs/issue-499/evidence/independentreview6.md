# Independent review 6 — selected cold Session publication owner

Verdict: **approve the exact frozen source described below; no actionable source finding remains in this follow-up.** This is an independent read-only review. I did not author the production repair or tests, and did not run Cargo, Playwright or the full validation suite during this review. Source approval is separate from the final execution/CI verdict.

## Identity and method

Worktree: `/home/capi/.codex/worktrees/issue-499/rustX`; branch `codex/issue-499-cache-aware-mcp`. The inspected committed HEAD is `bfaca081c56cd17287379d431bc3fbaa1dac7f31`; main/base remains `00978bbe3072b211efe835950f7f99666e1cc02d`. This review covers the actual uncommitted four-file Web follow-up plus its two documentation paragraphs, and relies on the explicitly identified earlier independent reviews for unchanged Native/TUI work.

I independently regenerated the complete path inventories and each file SHA-256 from current bytes, recomputed the sorted `path + NUL + lowercase SHA-256 + LF` aggregate, checked inventories against Git diffs/untracked files, verified HEAD/base ancestry and origin/main, and ran `git diff --check` successfully. Every value matches `/tmp/issue499/review6-source.json`:

| Inventory | Count | SHA-256 |
| --- | ---: | --- |
| Native | 1012 | `ac3d1c568596d75dc6cc6982f6a4ed23d8097bac0cec08db711e7b4147244bf8` |
| This follow-up | 4 | `22db9a75023fc0729c6030f53006b588c12c32a698941c9e36db2bbbc1282344` |
| Cumulative repair from `5a86a5631019ea0c7c16e12ed07bd92aa3869198` | 60 | `32d5e3abd163415b638cc49e5801ffe30e00e9b3aaa28cad2533d2c470ccfc20` |
| Cumulative follow-up from `d2a5aff54e9e82df1680ad4887f6824cdb556976` | 28 | `ade51f2c274019f903c0b188a7d743a8325b7274aea26728eb6b7ee2d51fec94` |

The four frozen inputs are:

- `web-console/src/app/navigation/machine.ts`: `c7c268a07c47bc948c3c102dba9a8fe65b01c9c0424ee3b0898f79139cba6825`
- `web-console/test/navigation-ownership.test.ts`: `bdc4cfe6936d59f50d29e8623b92aef7172368b338454c985bc03a7f13b8ebb4`
- `web-console/test/e2e/shell.spec.ts`: `f9b6b2b168b5eff767eae893a8d82e9ecc6bf6350c2370f63cc775c578a28517`
- `web-console/test/e2e/mcp-cold-navigation.spec.ts`: `9ae1619a7e4f948c36ac179c4df2c16c7182dab4ed39729e57d14b9c3567f2da`

## Correction of the earlier owner assessment

The earlier review5 assumption that dismissing the drawer or opening Settings should prevent a late selected C publication was wrong. Cold selection has already selected C's local editor. The real narrow-screen red case showed that retiring its preparation could still allow C's explicit first Send to reach native dispatch, while permanently leaving its route as `session-draft`. That prevented the following input. This is a production ownership hole, not merely an old test expectation.

The preserved red receipt `review6-shell-dismiss-red.result.json` is exit 1, 22.205397751997225 seconds, with log SHA-256 `5455b864d35e74e295da7864c4127f0c2421c715e0708564e2ec2177803aad55`. Its held-input-ACK scenario proves exact C submission intent and first turn dispatch before the failed formal-route assertion; it does not prove completed native acceptance. My earlier approval is superseded for this selected-target/viewport distinction. Its unchanged Native, TUI and external-effect conclusions remain tied to their prior exact fingerprints.

## Production ownership and fences

The new guard requires the exact selected Session draft, its exact pending intent, a Session/restore destination, and the matching Session ID. Drawer dismissal/Settings then retains the preparation invoke and only records the concrete Pending whose viewport-close interest was withdrawn. Completion can promote that still-selected target, but its publication cannot close a newly reopened drawer. The assign callback compares the prior `dismissedSidebar` identity before clearing it. This is bounded state associated with one intent, rather than a general background navigation subsystem.

Newer selection, explicit Cancel, Workflows navigation and generation/Native authority replacement still exit/retire the prior invoke. The correlated pending identity, current authority and attachment proof guards remain the publication boundary. History/Fork preparation that has not selected a new Session keeps its existing cancellation behavior. Cleanup clears the dismissed reference on normal completion, failure, retirement and authority change.

Promotion itself issues no submission intent, input, upload or model request. No first-submit admission, Host authorization, physical MCP owner, timeout, pending authored configuration or generic adoption policy changes in these four files. A's native execution/attachment owners and its in-flight ACK responsibilities stay independent from C's local editor. The architecture and invariants additions accurately describe this distinction and do not claim #504 prewarming.

## Regression quality

The new unit tests retain exact Pending identity across both Settings kinds, preserve A's actual authority, require formal C publication and disabled old sidebar-close interest, and prove newer-B/explicit-Cancel fences with zero submission/turn requests.

The three 390px shell cases exercise drawer dismissal and both Settings surfaces. They hold Attach and input ACK separately, require exact C/node-C intent, zero turn before Attach, then formal route, usable editor and a second distinct C input. Exactly one C intent/Attach and unchanged A/B native facts are asserted. These are real-client/controlled-native-peer cases, not actual stdio/provider tests.

The actual stdio MCP tests now cover Session and Workflows origins at both 1440px and 390px. They retain A's exact target, Attempt and upload ownership, require zero provider/input/new upload before C's explicit submission and readiness, exact first-request tool schema, one listing and one provider request, and then formal route plus a usable subsequent draft. They do not send a second model request.

The nine sidebar and eight Settings cases preserve the native mutation/claim, replacement/uncertainty, competing-B and one-closure assertions. They now observe selected C rather than borrowing A's surface. Reopened drawers remain open after old completion. Final no-Send branches explicitly assert zero submission intents. Refused C positively requires the real error and disabled editor, instead of filling an intentionally disabled surface. No added sleep, timeout increase, ignored test, swallowed failure or filtering of missing authority was found.

## Execution evidence and limits

I independently checked the final focused receipt and actual log bytes: `review6-browser-focused-final.result.json` is exit 0, 71.26146513299318 seconds; log SHA-256 `227dba801c508c16cc3d8bfbe616e66f9133b2d6235dea5ceb20bf090cc620e0`; log ends **25 passed**. This is runner evidence inspected by me, not a claim that I launched the tests. Parent separately reports sequential full Web **3272 passed / 4 platform skips**, typecheck/i18n/provenance success; those remain their separately recorded receipts.

The earlier full browser **392 passed /18 failed**, the red **1 failed**, and the first review6 focused **23 passed /2 failed** must remain failures in the evidence record. The latter two focused failures attempted to fill correctly disabled refused-C fixtures; final assertions now prove disabled/error behavior. The first overlapping-build Web run also remains **3214 passes plus 6 Host import failures**, not a green run. No historical run is reclassified by this source approval.

Full final 415-browser execution and current final-head FULL CI/Merge verification remain required and pending at this review cut. The unchanged Native aggregate and its separately recorded **5475 passed /0 failed /16 ignored** do not make those unfinished jobs successful. No merge or auto-merge is approved by this report.
