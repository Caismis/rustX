# Independent implementation review

The user requested an independent sub-agent review before PR creation. The
read-only `review_issue_504` agent reviewed the complete implementation and the
issue requirements, with follow-up passes after repairs. It did not author or
modify implementation code, run heavy builds, commit, push, or create the PR.

## Findings resolved before the final review

| Finding | Resolution and regression |
| --- | --- |
| Submitted create could overwrite a concurrent visible Session attachment. | Register the unique loading flight and convert its creation reservation under the catalog publication lock. The existing multi-client visibility regression now exercises submitted and inert creation, including durability uncertainty, and checks one composition/incarnation. |
| A committed creation carrying a durability diagnostic could discard its original owner/clock. | Retain the committed Session's unique initialization owner; the Web continuation still stops before attach/input on that diagnostic. A real MCP regression checks one handshake and the same deadline. |
| Initialization failure before attach could lose the Send clock. | Retain a bounded terminal result whenever the shared initialization has accepted submission intent. Automatic attach/reconnect returns the same result; a retry's own failure is retained too. Native regression exercises repeated automatic attach and two failing explicit retries. |
| Physical Workspace identity could become stale during capacity/readiness waiting. | Compare device/inode and captured bindings after capacity waiting and recapture current inputs after readiness waiting, before transfer. Ready and queued replacement regressions prove stale physical ownership is rejected. |
| Definite rejection/discard left an editable draft sealed. | Observe the real first-submission owner; release and retire only after authoritative rejection or explicit discard. Unknown outcomes remain sealed across reconnect. |
| Retained failure had no reachable client recovery. | Explicit initialization recovery replaces only the known failed native owner. Existing Web Open Session and TUI reselection invoke it without replaying creation or input; prepared attachment observation failure still reuses that attachment. |
| Delayed Web recovery could overwrite newer navigation. | One captured XState route/control fence gates final recovery dispatch, post-ACK navigation and error publication. Delayed authorization and delayed reply regressions preserve the newer Workspace route. |
| Repeated close after completed drain could restart an empty supervisor. | Make native scope close idempotent and schedule retirement only for an existing current slot. The existing drain regression now closes twice and drops the connection before synchronous completion, without yielding. |
| The post-visibility test hook could starve the submitted loading flight. | Run that blocking, test-only hook on the blocking pool at the same boundary. Production ownership/spawn ordering and the 30-second test bound are unchanged. |

## Final reviewer conclusion

> No remaining actionable findings or code-level blockers in the reviewed
> implementation. The previously reported ownership, deadline, physical-Workspace,
> recovery, and navigation issues are addressed.
>
> Final review was read-only; no builds or tests were rerun. Final-head FULL
> verification and completion of the issue's evidence report remain required.

The primary agent separately owns executed validation and the evidence report.

The final delta review also confirmed that the committed-creation guard removal
preserves the original preparation/deadline while the Web honours its diagnostic.
It found no new actionable issues. The additional credential/model regression
proves invalidation and unchanged-input reuse without changing implementation.

After #505 landed, the same independent reviewer checked the rebase onto
`1395faf0d00f474f5a79d809b7a91d6fc99c679b`. It found no actionable
integration issues: the ownership/recovery changes and upstream preview behavior
are preserved, all code readers use v58, and the updated provenance matches the
integrated source and preparation dependency. This pass was also read-only;
integrated FULL verification remains the primary agent's responsibility.

Full local verification subsequently exposed the repeated-close defect and the
blocking-hook scheduling issue above. The independent reviewer confirmed both
causes, then reviewed the applied repairs and stronger drain regression. Its
conclusion was: "No remaining actionable findings in these deltas." Focused and
full executed revalidation are reported separately in the evidence report.
