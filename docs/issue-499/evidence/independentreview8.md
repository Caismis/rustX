# Independent review: catalog recovery synchronization

Decision: approved, no blocking findings in the reviewed test-only change.

Reviewed base HEAD: `866f6024e1d995e305cb91a23609a62406163bb2`.
Reviewed file: `tests/scripted/app_server/configuration/catalog_recovery.rs`.
File SHA-256: `1a64e54007bcbbdb7e61bce5ed5b2e9680854d747608d404a6e88c9d9aaaf677`.
Diff SHA-256 (`git diff -- <file>`): `fe5828ce133880a54a822a7d47b15cd1ea6ea4df261e96f984c651f3463facc3`.
Native inventory: independently rehashed all 1012 inputs and reproduced aggregate `ea1c7076d0b16fae90bdcc40d51f4fcec2385b2840b42c202bcad4071fc41177` from `review8-native-source.json` using each `path + NUL + sha256 + newline` row.

## Scope and diagnosis

CI 1522 at the reviewed HEAD reported `Busy` at the removed-profile mixed-capability test's final adoption. The old eligibility subscription began before source edits and model-selection recapture. It could still contain a preparation's Busy→Eligible pair; the provider gate was released before the intended Attempt even started. Thus the event pair was not a receipt for that Attempt's idle boundary.

The change fixes both occurrences of this same synchronization error: removed model with pending Instructions, and removed profile with pending Capabilities. The actual repository diff is +70/-8 in this file only. No production code, generated artifacts, test timeouts, request parameters, fixtures, retry behavior or generic adoption semantics were changed.

## Owner and ordering review

1. `settled()` still proves the authored candidate is prepared; the existing native `wait_for_configuration_admissions()` additionally waits for actual counted nonresident operations to leave. Its implementation enables the Notify waiter before reading the level, so no dropped wakeup is substituted for actual lifecycle ownership.
2. A fresh `EligibilityStream` captures the runtime checkpoint and subscribes from that exact cursor only after those owners settle. Its Eligible result is checked against the current native `idle_epoch()`.
3. The settlement ordinal is captured before `submit_inbound`. The real provider `HeaderGate` remains closed and its watch-based entered level proves the request reached the response frontier. The test then consumes Busy and directly asserts a current Attempt plus `IdleBusyReason::LifecycleOwner`.
4. While that real Attempt is held, explicit authored adoption must return Busy and retain the exact same recaptured candidate. This strengthens the original intent-isolation proof; it does not retry until adoption succeeds.
5. Only then is the provider released. Exact next settlement ordinal, exactly one provider request, the fresh stream's later Eligible, no current Attempt and a current idle epoch establish the final ordinary adoption boundary.

## Assertions preserved

- Model/catalog availability and stale-candidate Conflict checks remain.
- The first recovered request retains adopted Instructions/tools while using the newly selected model/profile and exact request parameters.
- Pending Instructions are absent before explicit adoption and present afterward.
- `job_status` remains advertised before Capability adoption and is removed afterward; desired `reviewer` is not admitted early.
- Selected `local/c`/`deep` survives authored adoption; adopted binding/component revisions and capabilities are checked.
- Old registry ownership is still proved retired by the exact remaining strong count after native resource settlement.
- No schema assertion was removed or relaxed. This test-only synchronization change does not alter MCP schema/epoch/dispatch authority.

No blind retry loop, sleep or increased timeout was added. The only looped waits are existing level/event-owned synchronization helpers; the unchanged outer 30-second timeout remains a deadlock watchdog.

## Validation boundary

This was an independent source/call-chain review, including comparison to the existing owner-correct `catalog_agents` sequence and the actual CI failure excerpt. I ran no Cargo command and edited no repository file. Independent `git diff --check` passed. Author-reported formatting and the focused four-case run must be assessed through their own final receipts; the focused run was still compiling when this review was written. This report does not claim final local/full CI success or a PR merge verdict.
