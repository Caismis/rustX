# Permission modes — P01–P22 evidence map (Issue #544)

Living map of acceptance proofs against tip tests. Gaps are explicit; do not
treat an empty cell as covered. Tip under verification: `b012890f5` (#550 /
#546).

| ID | Requirement (short) | Evidence today | Gap |
| --- | --- | --- | --- |
| P01 | Built-in + external origin matrix | `src/runtime/permission/{matrix,classify,decision}.rs` unit tests; lifecycle Plan / Automatic / Full Access paths in `tests/scripted/agent/lifecycle.rs` | Not every external origin has a dedicated scripted matrix case |
| P02 | Mode change keeps tool set/schema | Mode reminders + schema decoration in `src/tools/schema.rs`; protocol `ExecutionPermissionMode` | Dedicated “same catalog across modes” conformance case thin |
| P03 | Deny/Ask/Allow/Default precedence | `src/runtime/permission/{resource,decision}.rs` (hard deny survives Full Access + elevation) | Cross-origin Ask/Allow matrix suite incomplete |
| P04 | Workspace shadows Global resource | Runtime composition + MCP binding tests under `src/local_runtime/` | UI visibility covered in Web; TUI still mode-only |
| P05 | `PLAN.md` exact target | `classify::is_workspace_plan_file` unit tests | Symlink/ancestor replacement boundary cases thin |
| P06 | ZCode Bash AST/argv corpus | `src/runtime/permission/bash/` (brush-parser AST + ZCode@b95ebaffe flag/callback tables); unit tests in `bash/mod.rs`; pin/license in `docs/permission-modes.md` | Corpus breadth vs upstream still thinner than ZCode's full suite |
| P07 | Complex MCP schema / elevation strip | Schema elevation strip + decoration in `src/tools/schema.rs`; executor prepare strips before business validate | MCP composition/cardinality corpus incomplete |
| P08 | Reviewer allow/deny/timeout/cancel | `src/runtime/permission/review.rs` + lifecycle Automatic paths | Broader malformed/truncation matrix thin |
| P09 | Human multi-client races | Interaction coordinator / lifecycle Ask tests | Cross-client race suite incomplete |
| P10 | Child create / send / resume / old activation | Subagent start `approval_mode` on `SubagentStartSpec`; conformance Ask proofs use Plan + `elevation_reason` (`tests/boundary/subagent/conformance.rs`) | Inactive-resume vs new-activation policy contrast case thin |
| P11 | Workflow nodes + fixed-node elevation | Workflow permission gates in `src/runtime/workflow/` tests | Fixed-node elevation metadata channel needs explicit case |
| P12 | Goal / Scheduler admission policy | Goal Complete/Blocked Allow; create/schedule mutation Elevate/Review in matrix + tool surface | Future-trigger admission policy case thin |
| P13 | Mode edit vs new Attempt / old background | Attempt admission freezes mode; process_death Plan Ask fixtures (`tests/boundary/durable/process_death/`) | Explicit mode-edit-while-background linearization case thin |
| P14 | Concurrent complete C/A/B → results A/B/C | `parallel_tool_results_commit_in_canonical_call_order` in `tests/scripted/agent/lifecycle.rs` | — |
| P15 | Allowed work while sibling awaits Ask | `parallel_batch_allows_independent_work_while_sibling_awaits_ask` in `tests/scripted/agent/lifecycle.rs` | — |
| P16 | Background Bash ownership | Bash background dispatch + text_spill fixtures; Full Access lifecycle fixtures | — |
| P17 | Todo / Goal / Schedule races | Todo sequential barrier + goal revision tests | Conflicting revision stress suite thin |
| P18 | Sparse first / change / compaction reminder | `src/runtime/permission/reminder.rs`; agent-loop permission reminder placement | Compaction-trigger reminder proof thin |
| P19 | No Agent Status dependence | Reminder path independent of status extension; child resume inherits mode | Explicit Status-disabled reminder case thin |
| P20 | Process death / reconstruction | `tests/boundary/durable/process_death/` Plan Ask + Full Access fixtures | Archive/reconstruction under broader grant denial case thin |
| P21 | WebUI / TUI / protocol | Web `PermissionSelect` + MCP `ResourcePermissionSet` editor; TUI `/settings … approval`; protocol v78+ | TUI lacks interactive MCP three-mode editor (TOML/Web carry authoring) |
| P22 | Self-edit / injection / credentials / unattended Ask | Hard deny non-elevatable; review treats tool args as untrusted; unattended Ask fails closed (no silent Allow) | Dedicated self-authorization / credential non-disclosure suite thin |

Update again only if FULL CI on the final tip changes the evidence surface (including Merge verification).
