# PR 517 pinned-browser screenshot repair — 2026-11-10

## Result

All 73 original Shell and Scheduler browser tests passed in 267.714 seconds with
zero skipped, unexpected, or flaky tests. Screenshot updates were disabled,
retries were zero, and the existing 120-second deadline and strict pixel/noise
policy were retained. Coverage includes both Workbench languages, pause command,
dark/mobile views, disconnected product states, and retirement, settings, and
attachment scenarios. The focused 147 screenshot-contract, shell, and schedules
unit tests also passed. TypeScript, 349 source provenance records, and i18n
checks passed. This is pinned-browser validation; the resulting revision requires
its own remote CI run.

The baseline correction addresses the screenshot failure at `8bc11ff28`. Shell
and Scheduler readiness/model assertions are in place, and 17 reviewed PNG
references now represent the intended footer controls. The ready Workspace
Policy / 策略 state, Scheduler fixture and model `fixture/native`, readiness
behavior, and strict comparison/noise policy are unchanged.

## CI finding and baseline correction

At head `8bc11ff28`, workflow run
[42743604185](https://github.com/Caismis/rustX/actions/runs/42743604185) had one
underlying shard-2 failure: nine screenshot comparisons. Coverage and merge
aggregate gates made three jobs red. The copied `main` references showed the
Permissions control disabled. The Shell fixture intentionally displays
`Choose model`; the Scheduler fixture is expected to show model
`fixture/native`. This PR configures the ready Workspace Policy / 策略 state and
that Scheduler fixture/model. The mismatch was in the references, not product
configuration.

The failed-run artifact [13731644747](https://github.com/Caismis/rustX/actions/runs/42743604185/artifacts/13731644747)
has SHA-256
`ea552653563e1e9ad3f42cc01e015df767cf0ffbd088ad7c32b6b5a9f44202d0`. Its nine
diff crops are limited to footer controls and each matches the prior PR
`b11e3c5c` crop with zero changed pixels. With the pinned environment, the
unchanged Scheduler calibration test passed against its existing strict
snapshots. The nine affected original screenshot tests then passed in 46.2
seconds after updating the references. All nine resulting captures match their
corresponding CI-stable captures pixel-for-pixel.

The pinned environment used Playwright 1.63.0 and Chrome for Testing
153.0.8010.12 from `mcr.microsoft.com/playwright` digest
`bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`; the exact
manifest SHA was verified. It ran from a Bubblewrap read-only root filesystem,
without host fonts; Helvetica resolved to Liberation Sans. The 17 changed PNGs,
bounds, pixel counts, hashes and image/artifact provenance are recorded in the
[reference diff manifest](ci-screenshot-reference-diffs-20261110.json), reviewed
alongside the full Scheduler Workbench screenshot and all diff crops.

## Changes and validation

- Added explicit readiness and expected-model assertions in `shell.spec.ts` and
  `scheduler.spec.ts` before screenshot comparison.
- Updated the 17 reviewed pinned PNG references. The unchanged Scheduler
  calibration test passed against existing strict snapshots. The nine affected
  screenshot tests passed in 46.2 seconds after the reference update; their
  captures match CI-stable images pixel-for-pixel.
- The full 73-test run passed with `RUSTX_SCREENSHOT_UPDATE` unset. It included
  both Workbench languages, pause command, dark/mobile views, disconnected
  product states, and retirement/settings/attachment scenarios. Final
  TypeScript, 349 source provenance records, and i18n validation passed.

The earlier full-head CI's native, Web, and TUI successes remain historical for
that head; the resulting revision requires its own remote CI run. PR #517 remains a
draft, and the separate performance gates remain open.
