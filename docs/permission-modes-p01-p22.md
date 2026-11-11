# Permission modes — P01–P22 evidence map (Issue #544)

Living map of acceptance proofs against tip tests. Gaps are explicit; do not
treat an empty cell as covered.

| ID | Requirement (short) | Evidence today | Gap |
| --- | --- | --- | --- |
| P01 | Built-in + external origin matrix | `src/runtime/permission/{matrix,classify,decision}.rs` unit tests; lifecycle Plan/Full Access Ask paths in `tests/scripted/agent/lifecycle.rs` | Not every external origin has a dedicated scripted matrix case |
| P02 | Mode change keeps tool set/schema | Mode reminders + schema decoration in `src/tools/schema.rs`; protocol `ExecutionPermissionMode` | Dedicated “same catalog across modes” conformance case thin |
| P03 | Deny/Ask/Allow/Default precedence | `src/runtime/permission/{resource,decision}.rs` unit tests | Full elevation-vs-hard-deny boundary suite incomplete |
| P04 | Workspace shadows Global resource | Runtime composition + MCP binding tests under `src/local_runtime/` | UI visibility covered in Web; TUI still mode-only |
| P05 | `PLAN.md` exact target | `classify::is_workspace_plan_file` unit tests | Symlink/ancestor replacement boundary cases thin |
| P06 | ZCode Bash AST/argv corpus | Conservative `src/runtime/permission/bash.rs` + regression tests | **Open:** brush-parser AST + pinned ZCode corpus port |
| P07 | Complex MCP schema / elevation strip | Schema elevation strip in `src/tools/schema.rs` | MCP composition/cardinality corpus incomplete |
| P08 | Reviewer allow/deny/timeout/cancel | `src/runtime/permission/review.rs` + lifecycle Automatic paths | Malformed/truncation already hardened; broader failure matrix thin |
| P09 | Human multi-client races | Interaction coordinator / lifecycle Ask tests | Cross-client race suite incomplete |
| P10 | (if present in issue table) | — | Confirm numbering against issue body when reporting |
| P11 | Workflow nodes + fixed-node elevation | Workflow permission gates in runtime workflow tests | Fixed-node elevation metadata channel needs explicit case |
| P12 | Goal/Scheduler admission policy | Goal tool surface Complete/Blocked only; schedule mutation Elevate/Review | Future-trigger admission policy case thin |
| P13–P15 | Capture/linearization/recovery | Attempt lifecycle + process_death Full Access fixtures | Map exact IDs when closing report |
| P16 | Background Bash ownership | Bash background dispatch + text_spill fixtures | Keep with Full Access lifecycle fixtures |
| P17 | Todo/Goal/Schedule races | Todo sequential barrier + goal revision tests | Conflicting revision stress suite thin |
| P18–P20 | Reminders / child / status independence | Reminder emission in agent execution; child inherits mode | Document reminder commit proofs in final report |
| P21 | WebUI/TUI / protocol | Web `PermissionSelect` + MCP `ResourcePermissionSet` editor; TUI `/settings … approval`; protocol v78 | TUI lacks interactive MCP three-mode editor (TOML/Web carry authoring) |
| P22 | Docs / breaking contracts | `docs/permission-modes.md` + architecture/invariants updates | Bash pin/license section pending AST port |

Update this file when P06 lands and when FULL CI on the final tip is green.
