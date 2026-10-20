# AUX-02 implementation plan review

## Status and scope

This companion reviews [the original plan at `0c178e2460ea49186b2c7c6f13fd38d9188d2f53`](https://github.com/MiNisterPaX/rustX/blob/0c178e2460ea49186b2c7c6f13fd38d9188d2f53/docs/aux-02-implementation-plan.md) against [AUX-02 #474](https://github.com/Caismis/rustX/issues/474) and relevant source contracts. The original plan remains unchanged, byte for byte. Read the clarifications below alongside it; they refine the identified sections without replacing its remaining contracts.

No established blocker to the proposed direction was found. The single-request scope, narrow executor interface, Context-owned Summary semantics, and deferred AUX-03/AUX-04 features are appropriate. These are explicit plan clarifications, not claims of demonstrated runtime defects.

Authorization covers these documentation changes only. Runtime implementation still requires separate approval.

## 1. Make the resource-release condition explicit

Clarify the Call-permit lifetime in [the capacity table, lines 85–95](https://github.com/MiNisterPaX/rustX/blob/0c178e2460ea49186b2c7c6f13fd38d9188d2f53/docs/aux-02-implementation-plan.md#L85-L95):

> Retain total/general Call permits and reserved required-write responsibility until all owned execution, write and confirmation work has physically exited and its leases have returned, AND required obligations are confirmed or transferred through an explicit proven-exited durability-failure handoff.

A completed write alone does not release admission capacity while confirmation remains active. The original term “cleanup exit” may already include this condition; make it explicit rather than relying on that interpretation. Otherwise, an implementation could return general permits while retaining writer lanes, allowing later general admissions to consume capacity intended to remain usable by Compaction.

Add deterministic saturation coverage with writes already exited but confirmation delayed. Verify the local 4/3 and process 32/24 limits, protected write capacity, and exactly-once release. Transfers of still-live cleanup carry its permits. Historical uncertainty after proven-exited handoff must not retain physical capacity indefinitely.

## 2. Separate new ownership permission from existing settlement authority

Add to [the lock protocol, lines 99–115](https://github.com/MiNisterPaX/rustX/blob/0c178e2460ea49186b2c7c6f13fd38d9188d2f53/docs/aux-02-implementation-plan.md#L99-L115):

> Preserve one ordered durability frontier for new ownership, failure and dispatch authorization. Already-owned terminal settlement and authoritative exact confirmation do not reacquire healthy-state admission and are not refused because DurabilityFailed has been installed. They remain supervised by the existing native owner and subject to physical writer exclusion and Store integrity checks.

This preserves the [existing DurabilityGate contract](https://github.com/MiNisterPaX/rustX/blob/0c178e2460ea49186b2c7c6f13fd38d9188d2f53/src/runtime/types.rs#L596-L617), without creating another failure authority.

Step 2 must test both ownership/failure winners, already-owned auxiliary settlement after failure, and existing background/subagent settlement-survival contracts. Audit actual call chains and lock ownership; substituting an asynchronous mutex alone is not sufficient. Include constrained/current-thread runtime scheduling so progress does not depend on spare asynchronous worker threads.

## 3. Require safety evidence before production migration

Clarify [steps 4–6](https://github.com/MiNisterPaX/rustX/blob/0c178e2460ea49186b2c7c6f13fd38d9188d2f53/docs/aux-02-implementation-plan.md#L157-L193):

- Step 4 cannot enable automatic Summary until parent cancellation/deadline versus checkpoint-grant tests pass.
- Step 5 cannot enable transferred manual Summary until business-tail drain ownership, actual OperationLease release, shutdown cancellation, and writer exclusion with live blocking work are proven.
- Step 6 completes integrated verification; it is not the first establishment of those guarantees.

A coherent commit group may establish these together, provided every production-enabled intermediate state preserves them.

## Retained Step 2 proof gate

The original [Step 2 exit gate](https://github.com/MiNisterPaX/rustX/blob/0c178e2460ea49186b2c7c6f13fd38d9188d2f53/docs/aux-02-implementation-plan.md#L131-L141) remains mandatory before provider dispatch. Document and test actual worker/thread/task bounds, including join offloading and confirmation work. A bound of 32 lanes is not itself proof of a 32-thread total.

This review does not prescribe a replacement scheduler or general asynchronous Store framework. If cleanup progress or the ownership/lock protocol cannot be proven, return the revised design for review.

## Baseline update and evidence limits

[PR #486](https://github.com/Caismis/rustX/pull/486) merged on 2026-10-20 at 12:06:17 UTC as [`0f3d978e43e2174d5b26feff6bccefc1bf25f71d`](https://github.com/Caismis/rustX/commit/0f3d978e43e2174d5b26feff6bccefc1bf25f71d). Its accepted head was `4aedcda2c9415cec3cfd1b06d3539215009b2e09`. The original plan's open-PR statement describes its planning baseline, not current status.

Before implementation, verify actual main and accepted AUX-01 contracts. Integrate the accepted baseline before AUX-02 delivery, preserve existing branch history, and rerun affected checks. Do not rewrite ancestry to conceal prerequisite work.

This was a documentation and source-contract review. No runtime implementation, runtime tests, or AUX-02 CI success is claimed.
