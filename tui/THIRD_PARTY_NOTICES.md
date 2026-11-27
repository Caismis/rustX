# Third-party notices

## `@earendil-works/pi-tui`

- **Package**: [`@earendil-works/pi-tui`](https://www.npmjs.com/package/@earendil-works/pi-tui)
- **Version used**: `0.82.1` (pinned exactly; not a range)
- **Upstream project**: Pi — <https://github.com/earendil-works/pi>, package
  directory `packages/tui`
- **License**: MIT

### Scope of use

`@earendil-works/pi-tui@0.82.1` supplies the published rendering primitives.

The committed-summary component in
`src/ui/components/compaction-summary.ts` adapts Pi Coding Agent's
[`compaction-summary-message.ts`](https://github.com/earendil-works/pi/blob/850ea2eae2f39eb16ae14e1f09448eab6da2188a/packages/coding-agent/src/modes/interactive/components/compaction-summary-message.ts)
at immutable commit `850ea2eae2f39eb16ae14e1f09448eab6da2188a`.
Its content and identity come from native rustX checkpoints, and disclosure is
held in attachment-local presentation preferences. The MIT notice below also
applies to this adaptation.

Compaction-specific colors in `src/ui/theme.ts` reproduce that commit's
`packages/coding-agent/src/modes/interactive/theme/dark.json`: background
`#3a3055`, label `#a798d7` and text `#9da5a9`. The pinned upstream pi-tui
1.5.0 color converter produced these sRGB values from the reference OKHSL
tokens. The reference renderer is isolated from the product dependency graph.

In particular, none of the following Pi coding-agent components were copied,
ported, or adapted: `InteractiveMode`, `FooterComponent`, the
`AgentSession`-bound assistant renderer, `ToolExecutionComponent`, the model
resolver, the session selector, the session tree, compaction controls,
authentication/login code, the extension infrastructure, the Skills runtime,
or shell shortcuts. The compaction-summary adaptation is the exception
described above; status reason/cancellation grammar was studied at the same pin.

### Primitives imported

Only terminal input/output primitives are imported, all from the package's
public entry point:

| Import | Role in rustX |
| --- | --- |
| `TUI` | differential renderer and focus/overlay host |
| `ProcessTerminal` | the `process.stdin`/`process.stdout` terminal |
| `Container` | grouping of child components |
| `Box` | terminal-width background bands, including committed summaries |
| `Editor` | the multiline input editor |
| `Markdown` | Markdown layout of rustX-rendered text |
| `Text` | plain text layout (footer, background section) |
| `Spacer` | vertical spacing |
| `Loader` | the working indicator |
| `SelectList` | the model-selection overlay |
| `matchesKey` | key matching for the interrupt binding |
| `AutocompleteItem`, `AutocompleteProvider`, `AutocompleteSuggestions` (types) | the interface implemented by rustX's `SlashCommandAutocompleteProvider` |
| `fuzzyFilter` | ranking of rustX slash-command completions |
| `Component`, `OverlayHandle` (types) | component and overlay handle typing |

`CombinedAutocompleteProvider` is **deliberately not used**: it performs
Node-side filesystem traversal and may invoke `fd`. rustX implements its own
`SlashCommandAutocompleteProvider`, which completes only rustX TUI commands
and never touches the filesystem.

### Transitive dependencies

`@earendil-works/pi-tui@0.82.1` depends on:

- `marked@18.0.5` — MIT
- `get-east-asian-width@1.6.0` — MIT

Exact resolutions are recorded in `tui/pnpm-lock.yaml`.

### MIT license text

The Pi project is distributed under the MIT License. The published
`@earendil-works/pi-tui` tarball declares `"license": "MIT"` in its
`package.json` but does not ship a `LICENSE` file, so the notice below is
reproduced from the upstream repository's `LICENSE`
(<https://github.com/earendil-works/pi/blob/main/LICENSE>):

```
MIT License

Copyright (c) 2025 Mario Zechner

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

The notice covers the published dependency and the identified Coding Agent
summary adaptation.

## Development dependencies

| Package | Version | License |
| --- | --- | --- |
| `typescript` | 5.9.3 | Apache-2.0 |
| `@types/node` | 22.19.2 | MIT |
