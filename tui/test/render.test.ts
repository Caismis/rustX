/**
 * The renderers, tested as pure functions.
 *
 * Rendering is proven without a terminal, which is the point: Pi sits at the
 * outermost layer and every correctness question below it is answerable from
 * strings.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { Markdown } from "@earendil-works/pi-tui";

import { replaceFromSnapshot, reduce } from "../src/presentation/projection.ts";
import {
  renderBackgroundSection,
  renderEntry,
  renderEntryBlocks,
  renderFooter,
  renderForegroundTool,
} from "../src/ui/render.ts";
import type { PresentationState } from "../src/presentation/state.ts";
import type { ForegroundToolExecution } from "../src/protocol/types.ts";
import { markdownTheme } from "../src/ui/theme.ts";
import {
  assistantMessage,
  attemptModel,
  backgroundExecution,
  runtimeInbound,
  sessionModel,
  snapshot,
  toolResult,
  userMessage,
} from "./support/fixtures.ts";

/** Strips SGR sequences so assertions read as plain text. */
function plain(text: string): string {
  // eslint-disable-next-line no-control-regex
  return text.replace(/\[[0-9;]*m/g, "");
}

const base = (overrides = {}) =>
  replaceFromSnapshot(snapshot(overrides), 0) as PresentationState;

describe("transcript rendering", () => {
  it("labels human and runtime-originated inbound differently", () => {
    const state = base({
      messages: [
        userMessage("m1", "a human turn"),
        runtimeInbound("m2", "a runtime turn"),
      ],
    });
    const [human, runtime] = state.transcript.map((entry) =>
      plain(renderEntry(entry)),
    );

    assert.match(human ?? "", /▌ you/);
    assert.match(human ?? "", /a human turn/);
    assert.match(runtime ?? "", /▌ runtime/);
    assert.match(runtime ?? "", /a runtime turn/);
  });

  it("keeps reasoning and refusal visually distinct from assistant text", () => {
    const state = base({
      messages: [
        {
          role: "assistant",
          id: "m1",
          content: [
            { type: "reasoning", text: "internal" },
            { type: "text", text: "the answer" },
            { type: "refusal", text: "I cannot help with that" },
          ],
        },
      ],
    });
    const rendered = plain(renderEntry(state.transcript[0]!));

    assert.match(rendered, /▌ reasoning\ninternal/);
    assert.match(rendered, /▌ answer\nthe answer/);
    assert.match(rendered, /▌ refusal\nI cannot help with that/);
    const [reasoning, answer] = renderEntryBlocks(state.transcript[0]!);
    assert.equal(
      reasoning?.defaultTextStyle?.color?.("internal"),
      "\u001b[90minternal\u001b[0m",
    );
    assert.equal(answer?.defaultTextStyle, undefined);
  });

  it("reapplies reasoning style after nested Markdown resets ANSI", () => {
    const state = base({
      messages: [
        {
          role: "assistant",
          id: "m1",
          content: [
            {
              type: "reasoning",
              text: "before **bold** after `code` tail",
            },
          ],
        },
      ],
    });
    const [reasoning] = renderEntryBlocks(state.transcript[0]!);
    assert.ok(reasoning);

    const rendered = new Markdown(
      reasoning.markdown,
      0,
      0,
      markdownTheme,
      reasoning.defaultTextStyle,
    )
      .render(100)
      .join("\n");

    assert.match(rendered, /\u001b\[90m after/);
    assert.match(rendered, /\u001b\[90m tail/);
  });

  it("renders reasoning the provider did not expose without inventing text", () => {
    const state = base({
      messages: [
        { role: "assistant", id: "m1", content: [{ type: "reasoning" }] },
      ],
    });
    assert.match(
      plain(renderEntry(state.transcript[0]!)),
      /reasoning \(not exposed by the provider\)/,
    );
  });

  it("renders streaming blocks as they accumulate", () => {
    let state = base();
    for (const event of [
      { type: "attempt_started" as const, attempt_id: "a1", model: attemptModel("alpha/model-a") },
      { type: "assistant_message_started" as const, attempt_id: "a1", message_id: "m1" },
      {
        type: "assistant_text_delta" as const,
        attempt_id: "a1",
        message_id: "m1",
        block_index: 0,
        delta: "partial",
      },
    ]) {
      state = reduce(state, { cursor: state.cursor + 1, event });
    }
    assert.equal(plain(renderEntry(state.transcript[0]!)), "▌ answer\npartial");
  });

  it("renders a committed assistant message after the stream", () => {
    const state = base({ messages: [assistantMessage("m1", "final answer")] });
    assert.equal(
      plain(renderEntry(state.transcript[0]!)),
      "▌ answer\nfinal answer",
    );
  });
});

describe("tool rendering", () => {
  const execution = (
    state: ForegroundToolExecution["state"],
    name = "bash",
  ): ForegroundToolExecution => ({
    call_id: "c1",
    tool_id: "tool-1",
    name,
    state,
  });

  it("renders the generic lifecycle, not a per-tool branch", () => {
    // Two different tools with identical lifecycle state render identically
    // apart from their name: there is no per-tool semantic branch.
    const running = execution(
      { type: "running", arguments: '{"x":1}' },
      "bash",
    );
    const other = execution(
      { type: "running", arguments: '{"x":1}' },
      "totally_unknown_tool",
    );
    assert.equal(
      plain(renderForegroundTool(running)).replace("bash", "T"),
      plain(renderForegroundTool(other)).replace("totally_unknown_tool", "T"),
    );
  });

  it("shows assembled, running with progress, and settled states", () => {
    assert.match(
      plain(renderForegroundTool(execution({ type: "assembled", arguments: "{}" }))),
      /assembled/,
    );
    assert.match(
      plain(
        renderForegroundTool(
          execution({
            type: "running",
            arguments: "{}",
            progress: { message: "working", completed: 1, total: 4 },
          }),
        ),
      ),
      /running working · 1\/4/,
    );
    assert.match(
      plain(
        renderForegroundTool(
          execution({ type: "settled", arguments: "{}", result: toolResult() }),
        ),
      ),
      /ok/,
    );
  });

  it("never collapses a non-success settlement into success", () => {
    const cases: Array<[ForegroundToolExecution["state"], RegExp]> = [
      [
        {
          type: "settled",
          arguments: "{}",
          result: toolResult({ status: { type: "failed", error: "boom" } }),
        },
        /failed/,
      ],
      [
        {
          type: "settled",
          arguments: "{}",
          result: toolResult({
            status: { type: "cancelled", reason: "user_requested" },
          }),
        },
        /cancelled \(user_requested\)/,
      ],
      [
        {
          type: "settled",
          arguments: "{}",
          result: toolResult({ status: { type: "timed_out" } }),
        },
        /timed out/,
      ],
      [
        {
          type: "settled",
          arguments: "{}",
          result: toolResult({ status: { type: "interrupted" } }),
        },
        /interrupted \(outcome unknown\)/,
      ],
    ];
    for (const [state, expected] of cases) {
      assert.match(plain(renderForegroundTool(execution(state))), expected);
    }
  });

  it("reports truncation rather than hiding it", () => {
    const rendered = plain(
      renderForegroundTool(
        execution({
          type: "settled",
          arguments: "{}",
          result: toolResult({
            truncation: { truncated: true, original_bytes: 9_001 },
          }),
        }),
      ),
    );
    assert.match(rendered, /truncated from 9001 bytes/);
  });
});

describe("background rendering", () => {
  it("shows active and terminal executions with runtime state", () => {
    const state = base({
      background: [
        backgroundExecution("exec-1", "running", {
          progress: { message: "step 2" },
        }),
        backgroundExecution("exec-2", "succeeded", { result: toolResult() }),
      ],
    });
    const rendered = plain(renderBackgroundSection(state));

    assert.match(rendered, /Background — 1 active of 2 known/);
    assert.match(rendered, /exec-1 · running/);
    assert.match(rendered, /exec-2 · succeeded/);
    assert.match(rendered, /step 2/);
  });

  it("renders nothing when the runtime knows of no background work", () => {
    assert.equal(renderBackgroundSection(base()), "");
  });
});

describe("footer", () => {
  it("is written entirely from rustX presentation data", () => {
    const state = base({
      model: sessionModel("alpha/model-a"),
      capabilities: { revision: 3 },
      background: [backgroundExecution("exec-1", "running")],
      inbound: {
        pending: [
          {
            sequence: 1,
            message: {
              id: "m1",
              content: [{ type: "text", text: "queued" }],
              source: "human",
              kind: "message",
            },
          },
        ],
      },
    });
    const rendered = plain(renderFooter(state, "connected"));

    assert.match(rendered, /alpha\/model-a/);
    assert.match(rendered, /cap r3/);
    assert.match(rendered, /inbox 1/);
    assert.match(rendered, /bg 1/);
    assert.match(rendered, /connected/);
  });

  it("shows the attempt model separately when it differs from the session's", () => {
    const state = base({
      model: sessionModel("beta/model-b"),
      attempt: {
        attempt_id: "a1",
        phase: { type: "running" },
        turn: 2,
        model: attemptModel("alpha/model-a"),
      },
    });
    const rendered = plain(renderFooter(state, "connected"));

    assert.match(rendered, /beta\/model-b/, "the desired session model");
    assert.match(rendered, /attempt: alpha\/model-a/, "the frozen attempt model");
    assert.match(rendered, /running · turn 2/);
  });

  it("collapses to one model when both agree", () => {
    const state = base({
      model: sessionModel("alpha/model-a"),
      attempt: {
        attempt_id: "a1",
        phase: { type: "running" },
        turn: 1,
        model: attemptModel("alpha/model-a"),
      },
    });
    assert.ok(!plain(renderFooter(state, "connected")).includes("attempt:"));
  });

  it("reports a closed transport without implying cancellation", () => {
    const rendered = plain(renderFooter(base(), "closed: input_eof"));
    assert.match(rendered, /closed: input_eof/);
    assert.ok(!/cancelled/i.test(rendered));
  });
});
