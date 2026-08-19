/**
 * One logical tool call is one visual entity.
 *
 * The contract under test: every fact rustX publishes about a call —
 * the assistant's `tool_call` block, the attempt's foreground execution
 * lifecycle, and the committed canonical result message — lands on the same
 * card, keyed by the runtime's own `ToolCallId` and by nothing else.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { reduce, replaceFromSnapshot } from "../src/presentation/projection.ts";
import { correlateTools } from "../src/presentation/tools.ts";
import { renderTranscript } from "../src/ui/components/transcript.ts";
import {
  assistantBlocks,
  attemptModel,
  attemptView,
  foreground,
  snapshot,
  toolCallBlock,
  toolMessage,
  toolResult,
} from "./support/fixtures.ts";
import { blockText, prefs, stateOf, transcriptString } from "./support/render.ts";

describe("correlation identity", () => {
  it("joins call block, execution, and canonical result into one entity", () => {
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          toolCallBlock("call-1", "tool-bash", "bash", { command: "ls" }),
        ]),
        toolMessage("m2", "call-1", "tool-bash"),
      ],
      attempt: attemptView({
        foreground: [
          foreground("call-1", "tool-bash", "bash", {
            type: "settled",
            arguments: '{"command":"ls"}',
            result: toolResult(),
          }),
        ],
      }),
    });
    const correlation = correlateTools(state);

    assert.equal(correlation.byCallId.size, 1, "three facts, one entity");
    const tool = correlation.byCallId.get("call-1");
    assert.equal(tool?.name, "bash");
    assert.equal(tool?.lifecycle.type, "settled");
    assert.equal(correlation.orphans.length, 0);
  });

  it("renders one card, not a call block plus a running card plus a result", () => {
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          toolCallBlock("call-1", "tool-bash", "bash", { command: "ls" }),
        ]),
        toolMessage("m2", "call-1", "tool-bash"),
      ],
      attempt: attemptView({
        foreground: [
          foreground("call-1", "tool-bash", "bash", {
            type: "settled",
            arguments: '{"command":"ls"}',
            result: toolResult(),
          }),
        ],
      }),
    });
    const blocks = renderTranscript(state, prefs());

    assert.equal(blocks.length, 1, "the committed result folds into the card");
    const card = blockText(blocks[0]!);
    assert.match(card, /Bash/);
    assert.match(card, /\$ ls/);
    assert.match(card, /ok/);
    // The raw argument object is gone from the default presentation.
    assert.ok(!card.includes('"command"'));
  });

  it("keeps two concurrent identical calls apart", () => {
    // Same tool, same arguments, same turn. Only the call id differs, which
    // is exactly why correlation may use nothing else.
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          toolCallBlock("call-a", "tool-bash", "bash", { command: "ls" }),
          toolCallBlock("call-b", "tool-bash", "bash", { command: "ls" }),
        ]),
      ],
      attempt: attemptView({
        foreground: [
          foreground("call-a", "tool-bash", "bash", {
            type: "running",
            arguments: '{"command":"ls"}',
          }),
          foreground("call-b", "tool-bash", "bash", {
            type: "settled",
            arguments: '{"command":"ls"}',
            result: toolResult({ status: { type: "failed", error: "boom" } }),
          }),
        ],
      }),
    });
    const correlation = correlateTools(state);

    assert.equal(correlation.byCallId.size, 2);
    assert.equal(correlation.byCallId.get("call-a")?.lifecycle.type, "running");
    assert.equal(correlation.byCallId.get("call-b")?.lifecycle.type, "settled");

    const cards = renderTranscript(state, prefs()).map(blockText);
    assert.equal(cards.length, 2);
    assert.match(cards[0] ?? "", /running/);
    assert.match(cards[1] ?? "", /failed/);
  });

  it("never regresses a settled entity to running", () => {
    // A stale foreground slot must not undo a committed settlement.
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          toolCallBlock("call-1", "tool-read", "read", { path: "a.rs" }),
        ]),
        toolMessage("m2", "call-1", "tool-read"),
      ],
      attempt: attemptView({
        foreground: [
          foreground("call-1", "tool-read", "read", {
            type: "running",
            arguments: '{"path":"a.rs"}',
          }),
        ],
      }),
    });
    assert.equal(
      correlateTools(state).byCallId.get("call-1")?.lifecycle.type,
      "settled",
    );
  });

  it("keeps an execution visible when its stream was dropped", () => {
    // The attempt settled without committing, so the streaming message is
    // gone. The execution the runtime still reports is not.
    const state = stateOf({
      attempt: attemptView({
        phase: { type: "settled", outcome: { type: "timed_out" } },
        foreground: [
          foreground("call-1", "tool-bash", "bash", {
            type: "settled",
            arguments: '{"command":"ls"}',
            result: toolResult(),
          }),
        ],
      }),
    });
    const correlation = correlateTools(state);

    assert.equal(correlation.orphans.length, 1);
    assert.equal(correlation.orphans[0]?.callId, "call-1");
  });
});

describe("tool-result chronology", () => {
  /**
   * The fold invariant, proven at the level a reader sees.
   *
   * > A committed tool result is folded into its call's card only when
   * > folding cannot move it across unrelated canonical content.
   *
   * rustX's canonical model permits `text, tool_call, text` — an
   * `AssistantMessageBlock` is a plain block vector, and `StructuralIndex`
   * rejects only duplicate calls, duplicate results, and orphan results — so
   * this is a shape the TUI must render, not a shape it may assume away.
   */
  it("keeps a later result after intervening text, as one split card", () => {
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          { type: "text", text: "A" },
          toolCallBlock("call-1", "tool-bash", "bash", { command: "cargo test" }),
          { type: "text", text: "B" },
        ]),
        toolMessage("m2", "call-1", "tool-bash", toolResult({
          content: [{ type: "text", text: "842 passed" }],
          exit_code: 0,
        })),
      ],
    });
    const rendered = renderTranscript(state, prefs()).map(blockText);

    // Canonical order: A, the call, B, the result. Exactly what is drawn.
    assert.equal(rendered.length, 4);
    assert.equal(rendered[0], "A");
    assert.match(rendered[1] ?? "", /Bash/);
    assert.match(rendered[1] ?? "", /\$ cargo test/);
    assert.match(rendered[1] ?? "", /result below/);
    assert.equal(rendered[2], "B");
    assert.match(rendered[3] ?? "", /↳/, "the result is the same card continuing");
    assert.match(rendered[3] ?? "", /Bash/);
    assert.match(rendered[3] ?? "", /842 passed/);

    // One identity, two fragments — never the pre-#79 duplication of a raw
    // call block, a separate running card, and a separate result block.
    assert.ok(!(rendered[1] ?? "").includes("842 passed"), "no reordering");
    assert.ok(!(rendered[3] ?? "").includes("result below"));
    assert.ok(!rendered.some((block) => block.includes('"command"')));
  });

  it("still folds when the call is the last block of its message", () => {
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          { type: "text", text: "A" },
          toolCallBlock("call-1", "tool-bash", "bash", { command: "cargo test" }),
        ]),
        toolMessage("m2", "call-1", "tool-bash"),
      ],
    });
    const rendered = renderTranscript(state, prefs()).map(blockText);

    assert.equal(rendered.length, 2, "one text block and one whole card");
    assert.equal(rendered[0], "A");
    assert.match(rendered[1] ?? "", /ok/);
    assert.ok(!(rendered[1] ?? "").includes("result below"));
    assert.ok(!(rendered[1] ?? "").includes("↳"));
  });

  it("folds a whole trailing batch of parallel calls", () => {
    // Calls and results of one batch are related content, so folding moves
    // nothing across anything unrelated and each call stays one card.
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          { type: "text", text: "A" },
          toolCallBlock("call-a", "tool-bash", "bash", { command: "one" }),
          toolCallBlock("call-b", "tool-bash", "bash", { command: "two" }),
        ]),
        toolMessage("m2", "call-a", "tool-bash"),
        toolMessage("m3", "call-b", "tool-bash"),
      ],
    });
    const rendered = renderTranscript(state, prefs()).map(blockText);

    assert.equal(rendered.length, 3);
    assert.match(rendered[1] ?? "", /\$ one/);
    assert.match(rendered[1] ?? "", /ok/);
    assert.match(rendered[2] ?? "", /\$ two/);
    assert.match(rendered[2] ?? "", /ok/);
    assert.ok(!rendered.some((block) => block.includes("↳")));
  });

  it("splits every call of a mixed message, never only the trailing ones", () => {
    // Folding just `call-b` would draw its result before `call-a`'s, which
    // canonical order puts first. The decision is per message for that reason.
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          toolCallBlock("call-a", "tool-bash", "bash", { command: "one" }),
          { type: "text", text: "B" },
          toolCallBlock("call-b", "tool-bash", "bash", { command: "two" }),
        ]),
        toolMessage("m2", "call-a", "tool-bash", toolResult({
          content: [{ type: "text", text: "first result" }],
        })),
        toolMessage("m3", "call-b", "tool-bash", toolResult({
          content: [{ type: "text", text: "second result" }],
        })),
      ],
    });
    const rendered = renderTranscript(state, prefs()).map(blockText);

    assert.equal(rendered.length, 5);
    assert.match(rendered[0] ?? "", /\$ one/);
    assert.equal(rendered[1], "B");
    assert.match(rendered[2] ?? "", /\$ two/);
    assert.match(rendered[3] ?? "", /first result/);
    assert.match(rendered[4] ?? "", /second result/);
  });

  it("draws a whole card when the result has no call anchor at all", () => {
    // The assistant message was never committed, so the result is the only
    // place this call is visible and it may not render as a fragment.
    const state = stateOf({
      messages: [toolMessage("m1", "call-1", "tool-bash")],
    });
    const rendered = renderTranscript(state, prefs()).map(blockText);

    assert.equal(rendered.length, 1);
    assert.ok(!(rendered[0] ?? "").includes("↳"));
    assert.match(rendered[0] ?? "", /ok/);
  });

  it("expands both fragments of a split card as one entity", () => {
    const body = Array.from({ length: 30 }, (_, i) => `line ${i}`).join("\n");
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          toolCallBlock("call-1", "tool-bash", "bash", {
            command: Array.from({ length: 30 }, (_, i) => `echo ${i}`).join("\n"),
          }),
          { type: "text", text: "B" },
        ]),
        toolMessage("m2", "call-1", "tool-bash", toolResult({
          content: [{ type: "text", text: body }],
        })),
      ],
    });
    const collapsed = renderTranscript(state, prefs()).map(blockText);
    assert.ok(!collapsed.some((block) => block.includes("echo 29")));
    assert.ok(!collapsed.some((block) => block.includes("line 29")));

    const opened = renderTranscript(
      state,
      prefs({ expandedToolCalls: new Set(["call-1"]) }),
    ).map(blockText);
    assert.ok(opened.some((block) => block.includes("echo 29")));
    assert.ok(opened.some((block) => block.includes("line 29")));
  });

  it("splits a streaming call the same way once its result commits", () => {
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          toolCallBlock("call-1", "tool-bash", "bash", { command: "ls" }),
          { type: "text", text: "B" },
        ]),
        toolMessage("m2", "call-1", "tool-bash"),
      ],
      attempt: attemptView({
        foreground: [
          foreground("call-1", "tool-bash", "bash", {
            type: "settled",
            arguments: '{"command":"ls"}',
            result: toolResult(),
          }),
        ],
      }),
    });
    const rendered = renderTranscript(state, prefs()).map(blockText);

    // The foreground projection agrees the call settled; that does not move
    // the result's canonical position.
    assert.equal(rendered.length, 3);
    assert.match(rendered[0] ?? "", /result below/);
    assert.ok(!(rendered[0] ?? "").includes("ok"));
    assert.equal(rendered[1], "B");
    assert.match(rendered[2] ?? "", /↳/);
  });
});

describe("lifecycle progression", () => {
  /** Drives the reducer through one call's full event sequence. */
  function driveToRunning() {
    let state = replaceFromSnapshot(snapshot(), 0);
    const events = [
      {
        type: "attempt_started" as const,
        attempt_id: "a1",
        model: attemptModel("alpha/model-a"),
      },
      {
        type: "assistant_message_started" as const,
        attempt_id: "a1",
        message_id: "m1",
      },
      {
        type: "tool_call_started" as const,
        attempt_id: "a1",
        message_id: "m1",
        block_index: 0,
        call: { id: "call-1", tool_id: "tool-bash", name: "bash" },
      },
      {
        type: "tool_call_arguments_delta" as const,
        attempt_id: "a1",
        message_id: "m1",
        block_index: 0,
        call_id: "call-1",
        arguments_delta: '{"command":',
      },
      {
        type: "tool_call_arguments_delta" as const,
        attempt_id: "a1",
        message_id: "m1",
        block_index: 0,
        call_id: "call-1",
        arguments_delta: '"cargo test"}',
      },
      {
        type: "tool_call_assembled" as const,
        attempt_id: "a1",
        message_id: "m1",
        block_index: 0,
        call: {
          id: "call-1",
          tool_id: "tool-bash",
          name: "bash",
          arguments: { command: "cargo test" },
        },
      },
      {
        type: "tool_execution_started" as const,
        attempt_id: "a1",
        tool_call_id: "call-1",
        tool_id: "tool-bash",
      },
    ];
    for (const event of events) {
      state = reduce(state, { cursor: state.cursor + 1, event });
    }
    return state;
  }

  it("moves assembled -> running -> settled as the same entity", () => {
    let state = replaceFromSnapshot(snapshot(), 0);
    const push = (event: Parameters<typeof reduce>[1]["event"]) => {
      state = reduce(state, { cursor: state.cursor + 1, event });
    };

    push({
      type: "attempt_started",
      attempt_id: "a1",
      model: attemptModel("alpha/model-a"),
    });
    push({ type: "assistant_message_started", attempt_id: "a1", message_id: "m1" });
    push({
      type: "tool_call_started",
      attempt_id: "a1",
      message_id: "m1",
      block_index: 0,
      call: { id: "call-1", tool_id: "tool-bash", name: "bash" },
    });
    assert.equal(
      correlateTools(state).byCallId.get("call-1")?.lifecycle.type,
      "assembled",
    );

    push({
      type: "tool_execution_started",
      attempt_id: "a1",
      tool_call_id: "call-1",
      tool_id: "tool-bash",
    });
    assert.equal(
      correlateTools(state).byCallId.get("call-1")?.lifecycle.type,
      "running",
    );

    push({
      type: "tool_execution_progress",
      attempt_id: "a1",
      tool_call_id: "call-1",
      tool_id: "tool-bash",
      progress: { message: "compiling", completed: 3, total: 9 },
    });
    const progressing = correlateTools(state).byCallId.get("call-1");
    assert.equal(progressing?.lifecycle.type, "running");
    assert.deepEqual(
      progressing?.lifecycle.type === "running"
        ? progressing.lifecycle.progress
        : undefined,
      { message: "compiling", completed: 3, total: 9 },
    );
    // A progress update changes the same entity, never adds a second one.
    assert.equal(correlateTools(state).byCallId.size, 1);

    push({
      type: "tool_execution_settled",
      attempt_id: "a1",
      tool_call_id: "call-1",
      tool_id: "tool-bash",
      result: toolResult(),
    });
    assert.equal(
      correlateTools(state).byCallId.get("call-1")?.lifecycle.type,
      "settled",
    );
    assert.equal(correlateTools(state).byCallId.size, 1);
  });

  it("carries the runtime-published name and arguments into the execution", () => {
    // `tool_execution_started` repeats neither, so the reducer must have
    // created the slot at `tool_call_started`, exactly as Rust does.
    const state = driveToRunning();
    const slot = state.attempt?.foreground[0];

    assert.equal(slot?.name, "bash");
    assert.equal(slot?.state.arguments, '{"command":"cargo test"}');
  });

  it("folds incrementally to the same state a fresh snapshot rebuilds", () => {
    const incremental = driveToRunning();
    const resynced = replaceFromSnapshot(
      snapshot({
        messages: [],
        attempt: attemptView({
          in_flight: {
            message_id: "m1",
            blocks: [
              {
                type: "tool_call",
                block_index: 0,
                call_id: "call-1",
                tool_id: "tool-bash",
                name: "bash",
                arguments: '{"command":"cargo test"}',
              },
            ],
          },
          foreground: [
            foreground("call-1", "tool-bash", "bash", {
              type: "running",
              arguments: '{"command":"cargo test"}',
            }),
          ],
        }),
      }),
      incremental.cursor,
    );

    const left = correlateTools(incremental).byCallId.get("call-1");
    const right = correlateTools(resynced).byCallId.get("call-1");
    assert.deepEqual(left, right);
    assert.equal(
      transcriptString(incremental),
      transcriptString(resynced),
      "a resync reconstructs the identical card",
    );
  });
});
