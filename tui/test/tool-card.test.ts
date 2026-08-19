/**
 * The tool card: runtime facts decide semantics, tool identity decides looks.
 *
 * The two halves are tested separately on purpose. The status half must be
 * identical for every tool given the same runtime result; the presentation
 * half may differ per tool and must always degrade to the generic renderer
 * rather than fail.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { CorrelatedTool } from "../src/presentation/tools.ts";
import type { ToolExecutionResult } from "../src/protocol/types.ts";
import { renderToolCard } from "../src/ui/components/tool-card.ts";
import { hasSpecializedRenderer } from "../src/ui/components/tool-renderers.ts";
import { DEFAULT_PREVIEW_LINES } from "../src/ui/preferences.ts";
import { plainText } from "../src/ui/theme.ts";
import { toolResult } from "./support/fixtures.ts";

const context = { expanded: false, previewLines: DEFAULT_PREVIEW_LINES };
const expanded = { expanded: true, previewLines: DEFAULT_PREVIEW_LINES };

function tool(overrides: Partial<CorrelatedTool> = {}): CorrelatedTool {
  return {
    callId: "call-1",
    toolId: "tool-unknown",
    name: "unknown_tool",
    argumentsText: "{}",
    lifecycle: { type: "assembled" },
    committed: false,
    ...overrides,
  };
}

function card(overrides: Partial<CorrelatedTool> = {}, ctx = context): string {
  return plainText(renderToolCard(tool(overrides), ctx));
}

function settled(result: Partial<ToolExecutionResult> = {}) {
  return { type: "settled" as const, result: toolResult(result) };
}

describe("runtime-owned outcomes", () => {
  it("reports every settlement exactly as the runtime published it", () => {
    const cases: Array<[Partial<ToolExecutionResult>, RegExp]> = [
      [{}, /ok/],
      [{ status: { type: "failed", error: "boom" } }, /failed/],
      [{ status: { type: "denied", reason: "human denied" } }, /denied \(human denied\)/],
      [
        { status: { type: "cancelled", reason: "user_requested" } },
        /cancelled \(user_requested\)/,
      ],
      [{ status: { type: "timed_out" } }, /timed out/],
      [{ status: { type: "interrupted" } }, /interrupted \(outcome unknown\)/],
    ];
    for (const [result, expected] of cases) {
      assert.match(card({ lifecycle: settled(result) }), expected);
    }
  });

  it("never reads a status out of the output text", () => {
    // Output that says "ok" under an interrupted settlement is still
    // interrupted; output that says "error" under a success is still ok.
    const interrupted = card({
      lifecycle: settled({
        status: { type: "interrupted" },
        content: [{ type: "text", text: "test result: ok. 842 passed" }],
      }),
    });
    assert.match(interrupted, /interrupted \(outcome unknown\)/);
    assert.ok(!/(^|\s)ok(\s|$)/.test(interrupted.split("\n")[0] ?? ""));

    const success = card({
      lifecycle: settled({
        content: [{ type: "text", text: "error: everything is on fire" }],
      }),
    });
    assert.match(success.split("\n")[0] ?? "", /ok/);
  });

  it("does not infer running from an absent result", () => {
    assert.match(card({ lifecycle: { type: "assembled" } }), /preparing/);
    assert.ok(!/running/.test(card({ lifecycle: { type: "assembled" } })));
  });

  it("shows the runtime's duration and exit code, and never invents them", () => {
    assert.match(
      card({ lifecycle: settled({ duration_ms: 2_800, exit_code: 0 }) }),
      /2\.8s.*exit 0/,
    );
    assert.ok(!/exit/.test(card({ lifecycle: settled({ duration_ms: 12 }) })));
  });

  it("reports runtime truncation separately from the visual collapse", () => {
    const rendered = card({
      lifecycle: settled({
        truncation: { truncated: true, original_bytes: 9_001 },
      }),
    });
    assert.match(rendered, /runtime-truncated result \(from 9001 bytes\)/);
    // Expanding the card is a client act and does not undo the runtime's own
    // truncation, which stays reported.
    assert.match(
      card(
        {
          lifecycle: settled({
            truncation: { truncated: true, original_bytes: 9_001 },
          }),
        },
        expanded,
      ),
      /runtime-truncated result/,
    );
  });

  it("keeps the lifecycle identical across tools with different renderers", () => {
    const lifecycle = settled({ status: { type: "failed", error: "boom" } });
    const specialized = card({
      toolId: "tool-bash",
      name: "bash",
      argumentsText: '{"command":"ls"}',
      lifecycle,
    });
    const generic = card({
      toolId: "tool-python-thing",
      name: "thing",
      argumentsText: "{}",
      lifecycle,
    });

    assert.ok(hasSpecializedRenderer("tool-bash"));
    assert.ok(!hasSpecializedRenderer("tool-python-thing"));
    for (const rendered of [specialized, generic]) {
      assert.match(rendered.split("\n")[0] ?? "", /failed/);
      assert.match(rendered, /boom/);
    }
  });
});

describe("progressive disclosure", () => {
  const long = Array.from({ length: 30 }, (_, index) => `line ${index}`).join("\n");

  it("bounds a long body and says how much is hidden", () => {
    const rendered = card({
      lifecycle: settled({ content: [{ type: "text", text: long }] }),
    });
    assert.match(rendered, /line 0/);
    assert.match(rendered, /line 7/);
    assert.ok(!rendered.includes("line 8"));
    assert.match(rendered, /… 22 more lines · ctrl\+o to expand/);
  });

  it("shows everything when expanded, deterministically", () => {
    const rendered = card(
      { lifecycle: settled({ content: [{ type: "text", text: long }] }) },
      expanded,
    );
    assert.match(rendered, /line 29/);
    assert.ok(!rendered.includes("more lines"));
  });

  it("renders an empty result without inventing content", () => {
    const rendered = card({ lifecycle: settled({ content: [] }) });
    assert.match(rendered.split("\n")[0] ?? "", /ok/);
    assert.equal(rendered.split("\n").length, 1);
  });
});

describe("the generic fallback", () => {
  it("renders an unknown tool by its published name", () => {
    const rendered = card({
      toolId: "tool-mcp-corpus-search",
      name: "search",
      argumentsText: '{"query":"AttemptSettled","limit":5}',
      lifecycle: { type: "running" },
    });
    assert.match(rendered, /search/);
    assert.match(rendered, /"query": "AttemptSettled"/);
    assert.match(rendered, /running/);
  });

  it("falls back to the tool id when no name was published", () => {
    assert.match(card({ name: "", toolId: "tool-x" }), /tool-x/);
  });

  it("shows a partially streamed argument fragment without crashing", () => {
    const rendered = card({ argumentsText: '{"command":' });
    assert.match(rendered, /\{"command":/);
  });

  it("renders published JSON result content", () => {
    const rendered = card({
      lifecycle: settled({
        content: [{ type: "json", value: { total: 3, items: ["a"] } }],
      }),
    });
    assert.match(rendered, /"total": 3/);
  });

  it("keeps file and image content explicit", () => {
    const rendered = card({
      lifecycle: settled({
        content: [
          { type: "file", path: "out.txt" },
          { type: "image", url: "x" },
        ],
      }),
    });
    assert.match(rendered, /\(file\)/);
    assert.match(rendered, /\(image\)/);
  });

  it("shows a failure reason even when the result carries no content", () => {
    const rendered = card({
      lifecycle: settled({
        status: { type: "failed", error: "cannot read a.rs" },
        content: [],
      }),
    });
    assert.match(rendered, /cannot read a\.rs/);
  });
});

describe("specialized native renderers", () => {
  it("renders Bash as a command line, not argument JSON", () => {
    const rendered = card({
      toolId: "tool-bash",
      name: "bash",
      argumentsText: '{"command":"cargo test --all","timeout":600}',
      lifecycle: settled({
        content: [
          {
            type: "json",
            value: {
              exit_code: 0,
              stdout: "test result: ok. 842 passed\n",
              stderr: "",
              combined: "test result: ok. 842 passed\n",
            },
          },
        ],
        exit_code: 0,
        duration_ms: 2_800,
      }),
    });
    assert.match(rendered, /^✓ Bash · ok · 2\.8s · exit 0$/m);
    assert.match(rendered, /\$ cargo test --all \(timeout 600s\)/);
    assert.match(rendered, /test result: ok\. 842 passed/);
    assert.ok(!rendered.includes('"command"'));
  });

  it("reports a Bash call with no output as having none", () => {
    const rendered = card({
      toolId: "tool-bash",
      name: "bash",
      argumentsText: '{"command":"true"}',
      lifecycle: settled({
        content: [
          { type: "json", value: { exit_code: 0, stdout: "", stderr: "", combined: "" } },
        ],
        exit_code: 0,
      }),
    });
    assert.match(rendered, /\(no output\)/);
  });

  it("renders Read as a path and the window that was asked for", () => {
    assert.match(
      card({
        toolId: "tool-read",
        name: "read",
        argumentsText: '{"path":"src/runtime/agent_loop.rs","offset":120,"limit":121}',
      }),
      /Read[\s\S]*src\/runtime\/agent_loop\.rs[\s\S]*lines 120–240/,
    );
    assert.match(
      card({
        toolId: "tool-read",
        name: "read",
        argumentsText: '{"path":"a.rs","offset":10}',
      }),
      /from line 10/,
    );
    // With no window published, none is stated: the runtime's default is the
    // runtime's business.
    const bare = card({
      toolId: "tool-read",
      name: "read",
      argumentsText: '{"path":"a.rs"}',
    });
    assert.ok(!/lines |from line |first /.test(bare));
  });

  it("renders Grep as a query, a scope, and a match summary", () => {
    const rendered = card({
      toolId: "tool-grep",
      name: "grep",
      argumentsText:
        '{"pattern":"AttemptSettled","path":"src/runtime","literal":true}',
      lifecycle: settled({
        content: [
          {
            type: "json",
            value: {
              matches: [
                { path: "src/runtime/a.rs", line: 12, column: 4, text: "  AttemptSettled," },
                { path: "src/runtime/b.rs", line: 40, column: 1, text: "AttemptSettled {" },
              ],
              context: [],
              truncated: false,
            },
          },
        ],
      }),
    });
    assert.match(rendered, /Grep/);
    assert.match(rendered, /"AttemptSettled"/);
    assert.match(rendered, /src\/runtime · literal/);
    assert.match(rendered, /2 matches/);
    assert.match(rendered, /src\/runtime\/a\.rs:12 AttemptSettled,/);
  });

  it("renders Glob as a pattern and a path count", () => {
    const rendered = card({
      toolId: "tool-glob",
      name: "glob",
      argumentsText: '{"pattern":"**/*.rs"}',
      lifecycle: settled({
        content: [
          { type: "json", value: { results: ["a.rs", "b.rs", "c.rs"], truncated: false } },
        ],
      }),
    });
    assert.match(rendered, /Glob/);
    assert.match(rendered, /3 paths/);
    assert.match(rendered, /b\.rs/);
  });

  it("renders Edit as a path plus a diff derived only from the arguments", () => {
    const rendered = card({
      toolId: "tool-edit",
      name: "edit",
      argumentsText: JSON.stringify({
        path: "src/lib.rs",
        edits: [{ oldText: "let x = 1;", newText: "let x = 2;" }],
      }),
      lifecycle: settled({
        content: [{ type: "json", value: { path: "src/lib.rs", replacements: 1 } }],
      }),
    });
    assert.match(rendered, /Edit/);
    assert.match(rendered, /src\/lib\.rs/);
    assert.match(rendered, /^\s*- let x = 1;$/m);
    assert.match(rendered, /^\s*\+ let x = 2;$/m);
    assert.match(rendered, /applied 1 replacement/);
  });

  it("renders Write as a path plus the runtime's own byte count", () => {
    const rendered = card({
      toolId: "tool-write",
      name: "write",
      argumentsText: JSON.stringify({ path: "notes.md", content: "a\nb\nc" }),
      lifecycle: settled({
        content: [
          { type: "json", value: { path: "notes.md", bytes_written: 5 } },
        ],
      }),
    });
    assert.match(rendered, /Write/);
    assert.match(rendered, /notes\.md/);
    assert.match(rendered, /3 lines/);
    assert.match(rendered, /wrote 5 bytes to notes\.md/);
  });

  it("degrades to the generic presentation on an unexpected argument shape", () => {
    // Every specialized renderer is handed a shape it cannot read. None may
    // crash, and each must still show the call.
    const shapes = [
      ["tool-bash", "bash", '{"cmd":"ls"}'],
      ["tool-read", "read", '{"file":"a.rs"}'],
      ["tool-grep", "grep", '{"regex":"x"}'],
      ["tool-glob", "glob", "[1,2,3]"],
      ["tool-edit", "edit", '{"path":"a.rs","edits":"not-a-list"}'],
      ["tool-write", "write", '{"path":"a.rs"}'],
    ] as const;
    for (const [toolId, name, argumentsText] of shapes) {
      const rendered = card({ toolId, name, argumentsText });
      assert.match(rendered, new RegExp(name), toolId);
    }
  });

  it("degrades to the generic result body on an unexpected result shape", () => {
    const rendered = card({
      toolId: "tool-bash",
      name: "bash",
      argumentsText: '{"command":"ls"}',
      lifecycle: settled({ content: [{ type: "text", text: "plain output" }] }),
    });
    assert.match(rendered, /\$ ls/);
    assert.match(rendered, /plain output/);
  });
});
