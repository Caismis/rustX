# Issue #487 — layering refactor: requirement matrix

Starting base: `origin/main` at `6771928953a9b2815a27fd784f6d39c5ccde8b56`
(identical to the audit snapshot, so every audited finding applies).

Status legend: **done** (implemented, evidence below), **resolved on base**
(already true at the base, with source evidence), **open**.

Size numbers come from [`measure.py`](measure.py) (method in its docstring):
physical lines, comments and blanks included; Rust `#[cfg(test)]` items and
test-only module files counted as test; TypeScript test/generated files
separated. See [size-report.md](size-report.md) for before/after tables.

## A. Configuration and Runtime contracts

| Requirement | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| A1 separate authored/source/provenance from prepared bindings and admission/adoption | | | | | open |
| A2 `RuntimeConfiguration`, prepared configuration, adoption eligibility, resource loader owned natively | | | | | open |
| A3 one pure cache-impact policy | | | | | open |
| A4 configuration application split by capture/prepare/apply/publication | | | | | open |
| A5 stale-owner/revision, lock order, atomic adoption, model-selection ownership preserved | | | | | open |

## B. Native observation/read contracts and Runtime Client

| Requirement | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| B1 shared bounded contracts move to a native boundary | | | | | open |
| B2 no native calls into client display shaping (prepared status) | | | | | open |
| B3 presentation/wire shaping stays in the adapter | | | | | open |
| B4 projection/host split into read/subscription/projection/lifecycle | | | | | open |
| B5 bounded descriptors/caches, exact cuts, content identity preserved | | | | | open |

## C. SQLite and Store

| Requirement | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| C1 SQLite organized by schema/inbox/canonical/request/journal/publication/projection/observation | | | | | open |
| C2 one read-cut transaction helper | | | | | open |
| C3 narrower Store interfaces from real consumers | | | | | open |
| C4 one transaction owner per semantic operation | | | | | open |
| C5 domain tests relocated, rollback/corruption/cut/lineage/publication coverage kept | | | | | open |

## D. Native Runtime, execution, Subagent, Session

| Requirement | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| D1 ConversationRuntime split, single coordination owner | | | | | open |
| D2 AgentExecution inherent impl split (step/request/tools/publication) | | | | | open |
| D3 AgentExecution remains owner; state aggregates only on real lifetimes | | | | | open |
| D4 Subagent registry, Session, configuration, composition, manager modularized | | | | | open |
| D5 registration, drain, frozen input, settlement, ordering, terminal uniqueness intact | | | | | open |

## E. Shared Web/TUI semantics

| Requirement | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| E1 single native event fold; TUI duplicate folding removed | | | | | open |
| E2 shared exact numeric parsing/encoding/validation reasons | | | | | open |
| E3 one exhaustive method policy incl. parameter-sensitive cases | | | | | open |
| E4 transports keep their queue/backpressure | | | | | open |
| E5 shared Source repair rules and attachment identity | | | | | open |
| E6 `AppServerSession.updateState` removed | | | | | open |

## F. Web/TUI and App Server controllers

| Requirement | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| F1 Web client split (transport/session/reads/upload/submission) | | | | | open |
| F2 client ↔ first-submit cycle removed | | | | | open |
| F3 established seams reused, no forwarding context | | | | | open |
| F4 TUI app split | | | | | open |
| F5 dispatcher helpers and Session readers separated | | | | | open |
| F6 App Server handlers split by family, one admission/reply owner | | | | | open |

## G. MCP

| Requirement | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| G1 generation/lease/retirement module | | | | | open |
| G2 progress state/routing/subscriptions module | | | | | open |
| G3 invocation/outcome/materialization modules | | | | | open |
| G4 one tools-list-changed admission/invalidation/notification path | | | | | open |
| G5 common `dispatch_owned_request` preserved | | | | | open |

## H. Obsolete entry points and tests

| Requirement | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| H1 unused history façades removed | | | | | open |
| H2 `runtime/transcript_history.rs` removed | | | | | open |
| H3 App Server protocol/configuration tests split by contract | | | | | open |
| H4 inline tests moved with their domain | | | | | open |
| H5 repeated constructors/factories consolidated in existing support | | | | | open |
| H6 zero/one/multiple-session fixture controls used accurately | | | | | open |
| H7 Cargo discovery and module paths preserved | | | | | open |

## I. Documentation, structural checks, CI selection

| Requirement | Base finding | Old owner → final owner | Change / removed path | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| I1 documentation-contract selection | `DOCUMENTATION` paths had no surface, so a docs-only PR merged on FAST without running the `contracts` target that reads them | `verification.py` | `documentation` surface + FAST `documentation-contracts` job | `test_verification.py` (`test_contract_documentation_selects_only_the_documentation_contracts`) | done |
| I2 structural tests and lane rules updated | | | | | open |
| I3 ownership checks focus on contracts | | | | | open |
| I4 fast/full CI design preserved | | | | | open |
| I5 architecture/configuration/protocol docs updated | | | | | open |
| I6 generated protocol artifacts regenerated where sources changed | | | | | open |
