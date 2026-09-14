# DeepSeek Harness source provenance

Invariant: **Reuse the UI; keep runtime and protocol authority in rustX.**

Upstream: https://github.com/deepseek-ai/deepseek-harness

Exact baseline: `c291e7961a515f6d7af9304e7fd1d257929aef26` (unchanged from #289).
The upstream checkout was inspected outside rustX. No upstream fetch is part of
installation, build, startup, or runtime. Updates are ordinary reviewed source edits.

`source-inventory.json` records **every imported file**, its exact upstream path,
rustX destination, treatment, and SHA-256 of the original pinned source. The hashes
identify originals, not edited local bytes. All paths below are relative to
`packages/client/` upstream and `web-console/src/presentation/` locally.

| Upstream source | Destination | Treatment and retained presentation |
| --- | --- | --- |
| `ui-primitives/src/{Button,Input,Pill,StateDot,DisclosureRow}.{tsx,module.css}` | `primitives/` with the same names | Mostly intact React controls, capsule geometry, selectable pills, animated state matrix, keyboard/mouse disclosure chrome. Imports stay local. |
| `ui-primitives/src/icons/{index.tsx,props.ts}` | `primitives/icons/` | Four generic check/chevron glyphs extracted, props retained; no official product glyphs. |
| `ui-theme/src/styles/base.css` | `theme/base.css` | Font stacks and motion variables retained; system fonts only, no downloaded fonts. |
| `ui-theme/src/styles/design-platform.css` | `theme/tokens.css` | Generic token sheet retained; `deepseek` palette name renamed `accent`. rustX overrides the bubble/business accent in `app/console.css`. |
| `ui-layout/src/client/AppFrame.{tsx,module.css}` | `AppFrame.{tsx,module.css}` | Substantially refactored three-column shell. Center/sidebar/inspector presentation uses plain React children. Host slots, stores, resize service, document-title bindings and boot graph removed. |
| `ui-sidebar/src/client/SidebarRoot.{tsx,module.css}` | `Sidebar.{tsx,module.css}` | Extracted column/logo/region/footer presentation. Native Session rows replace workspace services. Branding is rustX text. |
| `ui-chat/src/client/chat/MessageItem.{tsx,module.css}` | `MessageItem.{tsx,module.css}` | Extracted `UserStyleBubble` layout and styling, with a generic assistant body. Removed event nodes, retry countdowns, attachment services, fork/file actions and submission echoes. |
| `ui-conversation/src/client/skeleton/InputBar.{tsx,module.css}` | `InputBar.{tsx,module.css}` | Extracted composer card, scroll/grow surface, toolbar and trailing actions. A native textarea uses Enter/Shift+Enter/IME handling; rustX operations replace Lexical, command/queue/upload machines and Session Controller. |
| `ui-tool/src/client/tool/components/ToolRow.{tsx,module.css}` | `ToolRow.{tsx,module.css}` | Extracted disclosure header and IN/OUT sections. Native state labels and values replace toolview/Remote models. File opening, Tool-specific Harness interpretations, image slots and trajectory navigation removed. |
| `ui-approval/src/client/ApprovalPanel.{tsx,module.css}` | `ApprovalPanel.{tsx,module.css}` | Retained ApprovalFlow card, waiting strip, scrollable details, reject/allow actions. Remote waterfall and component-owned `answered` lifetime removed; rustX authoritative binding controls status/disabled state. |
| `ui-user-questions/src/client/QuestionComposer.{tsx,module.css}` | `QuestionComposer.{tsx,module.css}` | Substantially refactored option rows, recommendation-suffix parsing, mirror-growing text field, draft selection and pager. Native question indices and scalar wire encodings replace label-based selection, Remote promises, plan-intent routing and Session slot stores. |
| Repository `LICENSE` | `web-console/public/LICENSE-DeepSeek-Harness.txt` | Verbatim MIT notice, included in production builds. Copyright (c) 2026 DeepSeek. |

The source-derived TSX files identify their origins at the top. Extracted CSS keeps
only rules used by the selected presentation, plus small local layout adaptations.
This is substantive source reuse: controls and their behavior, disclosure/card
structure, questionnaire form interaction and the shared token/style system are
checked in. It does not rely on screenshots, visual imitation, or dormant upstream
packages to claim reuse.

## Dependency closure inspected before extraction

The pure primitives require React, CSS modules and `clsx`; their generic icon
imports can be reduced to four glyphs. Higher-level TSX receives Session Controller
state through ui-session/ui-slots and crosses into connection/Remote, locale,
file/attachment, workspace and toolview models. The upstream `web` package loads a
Host-authored module and Cordis plugin graph. Importing its entry would pull the
wrong authority boundary into rustX, so none of that graph is retained.

Retained third-party production dependencies: **React 19.3.0, React DOM 19.3.0,
clsx 2.1.1**, plus React DOM's pinned **scheduler** dependency. CSS modules are
compiled by Vite; there is no CSS runtime or component framework. Lockfile versions
are authoritative. Upstream used React 18; the pure prop-driven extraction works
with React 19 without retaining an upstream runtime service.

Additional build/test-only tools: TypeScript, Vite, its React plugin, Vitest,
Testing Library, jsdom, Playwright and type declarations. Their exact versions and
transitive closure are committed in `pnpm-lock.yaml`. These are conventional tooling,
not a Node semantic backend. No runtime dependency uses an upstream git branch.

## Replaced and excluded areas

- **Replaced:** Harness connection, RPC/Remote, ui-session, Session Controller,
  event assembler, durable V3 log, boot/module graph, stores and action wiring.
  `src/client/` and `src/bindings/` use generated native rustX DTOs directly.
- **Excluded:** Cordis and slots entirely; the selected closure does not benefit
  from a plugin system. Also excluded: Host/Agent, provider settings/onboarding,
  official branding/marks, workspace manager, file/editor/terminal/PTY/Git features,
  uploads, attachment fetching, commands, input suggestions and browser persistence
  of runtime facts.
- **Inspected but excluded:** specialized `ui-subagent`, `ui-workflow-run` and
  `ui-goal` components. Their tree/log/continuation semantics do not match rustX.
  Native Subagent, Workflow, Goal, Todo and background facts use generic read-only
  disclosure views. No semantic compatibility layer was introduced to activate them.
- **Unsupported controls removed:** Harness queue/retry/deduplication, plan mode,
  policy presets, provider editing, open-file and workspace actions, Workflow/Goal
  continuation controls, image upload and plugin installation. Native start/steer,
  cancel, interaction settlement, attach/detach/unload remain explicit RPC gestures.

## Licenses and assets

All imported Harness source is under the retained MIT license. Four generic SVG
paths originate in that source's Figma-derived icon set and carry the same notice;
no separate asset license was supplied for those paths. No official whale/fish logo,
third-party photographs, fonts, Markdown math assets or syntax-highlighting assets
were imported. System font names are references to installed fonts, not vendored
font binaries.

`public/THIRD-PARTY-NOTICES.txt` retains license text for every production JavaScript
dependency included in the bundle. Both notice files are copied into `dist/`.
Build/test dependency licenses remain in their installed packages; React, clsx,
Vite, Vitest, Testing Library, jsdom and DefinitelyTyped use MIT, while TypeScript
and Playwright use Apache-2.0. See the lockfile and upstream package notices for
transitive toolchain licensing. rustX's repository MIT license remains applicable
to local changes.
