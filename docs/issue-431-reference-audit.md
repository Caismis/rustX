# Issue #431 reference audit

Read-only reference: `deepseek-ai/deepseek-harness` at
`639ed015397290b3745d163aafe02ffee4aa3f84`, inspected in the separate detached
worktree `/home/caismis/Documents/codes/deepseek-harness-ref-431`. The original
reference checkout was clean at `477b4f420553e8a52c2fbccc464d7561b239c443` and was
not modified.

Files inspected at the approved pin:

- `packages/client/ui-deliverables/README.md`
- `packages/deliverables/tool-present/src/index.ts`, `src/types.ts`
- `packages/client/ui-deliverables/src/index.ts`, `src/presented.ts`, `src/present-open.ts`
- `packages/client/ui-deliverables/src/client/PresentRow.tsx`, `PresentedFileCard.tsx`, `file-actions.ts`, `turn-deliverables.ts`
- `packages/api/workspace-files/src/index.ts`
- `packages/client/ui-sidebar-documentpreview/src/index.ts`
- `packages/client/ui-sidebar-documentpreview/src/client/index.ts`, `rpc.ts`, `definition.ts`
- `packages/client/ui-sidebar-documentpreview/src/client/markdown/MarkdownBody.tsx`
- `packages/client/ui-primitives/src/markdown/render.tsx`

Concepts adapted: explicit `present` declaration, structured canonical coordinates
for file actions, a shared right-panel seat, and independent bounded original-byte
reads for rendering and Download. rustX resolves original native Conversation/root
identity and successful canonical Tool metadata instead of Harness Session events.

No new Harness source was copied or mechanically adapted at this pin. Code reused
is rustX's existing attributed safe Markdown renderer, RightPanel, and native
Tool/Session/App Server owners. Source inventory changes refresh hashes and imports
of those existing descendants; they do not claim new upstream provenance for
conceptual inspiration.

Excluded: Cordis/plugin/runtime ownership; separate delivery events/database;
mutation-derived “produced” files; guessed Markdown/file mentions; generic resource
registry; changed-files/diff/review systems; desktop opener; full document tabs,
Office/HTML viewers; unrestricted reads outside cwd; cwd fallback; embedded local
or remote Markdown images. The approved reference's outside-workspace read policy
is deliberately replaced by rustX's exact original-root descriptor containment.
