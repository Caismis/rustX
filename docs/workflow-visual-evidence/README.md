# Native browser evidence

These PNGs are captures from the real App Server, Product Host and production Web
bundle in the repository's immutable Chromium authority, not design mockups.
The browser tests attach the same files to their reproducible traces. See the
[design, ownership and acceptance report](../workflow-visual-editor.md).

| State | Capture |
| --- | --- |
| Full shell, new file, no selection | [01](01-no-selection.png) |
| Start arguments and description | [02](02-start-schema.png) |
| Consumer-scoped variable popup with pending facts | [03](03-variable-picker.png) |
| Model and whole-text output | [04](04-model.png) |
| Tool argument mapping and projection | [05](05-tool.png) |
| Agent bindings and contract | [06](06-agent.png) |
| Review | [07](07-review.png) |
| Parallel lanes expanded / collapsed | [08](08-parallel-expanded.png), [09](09-parallel-collapsed.png) |
| Loop body expanded / collapsed | [10](10-loop-expanded.png), [11](11-loop-collapsed.png) |
| Branch IF/ELSE | [12](12-branch.png) |
| Root Return and aggregate references | [13](13-return.png) |
| Safe Markdown note and dimensions | [14](14-note.png) |
| Native Check | [15](15-checked.png) |
| Fresh browser profile reopens file-owned note | [16](16-fresh-profile.png) |
| Narrow window with usable canvas and Settings sheet | [17](17-narrow.png) |
| Visual nested structured schema | [18](18-structured-schema.png) |
| Repeated binding of one declared field | [19](19-structured-reuse.png) |
| Preserved invalid source | [20](20-preserved-invalid-file.png) |
| External source conflict, local edits retained | [21](21-source-conflict.png) |
| Exact retained input, Show/focus and copy refusal | [22](22-pending-input.png) |

[Measurements](bounded-scene-measurements.json) describe the real 256-executable-
node, three-Block fixture at 1440×1000. The predeclared render/organize budgets
are 10,000ms each. This local sample measured approximately 976ms to open/render
and 775ms to organize. It is one Linux x86_64 run with Node 24.19.0, pnpm 11.13.1,
Rust 1.98.1 and Playwright 1.63.0, not a cross-machine performance guarantee.
ELK's unmodified lazy production chunk is approximately 1,464kB / 452kB gzip;
the Workflow editor chunk is approximately 300kB / 91kB gzip. It is fetched only
for explicit Organize.

For a complete interaction recording, run `workflow-visual.spec.ts` with the
repository browser script and open the resulting `trace.zip` in Playwright's
trace viewer. Its complete-authoring case always records create → bind → nested
edit → Save → clean-profile reopen, including keyboard clipboard/history and
pending-field recovery. CI retains that trace in its merged browser report.
