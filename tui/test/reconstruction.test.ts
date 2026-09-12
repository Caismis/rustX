/**
 * A fresh snapshot reconstructs the whole semantic UI.
 *
 * This is the property that lets every Pi component be a disposable render
 * target: no component instance history, no hidden local conversation log,
 * and no incremental state that a `snapshot_get` cannot rebuild. Purely local
 * display preferences are the deliberate exception, and they are proven to be
 * non-semantic here rather than assumed to be.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { replaceFromSnapshot } from "../src/presentation/projection.ts";
import { correlateTools } from "../src/presentation/tools.ts";
import {
  renderBackgroundSection,
  renderInteractionSection,
  renderOrphanExecutions,
} from "../src/ui/components/activity.ts";
import { renderFooter, workingStatus } from "../src/ui/components/status.ts";
import { renderTranscript } from "../src/ui/components/transcript.ts";
import {
  withReasoningVisible,
  withExpandedBackgroundExecutions,
  withExpandedInteractions,
  withExpandedToolCalls,
  withToggledToolCall,
} from "../src/ui/preferences.ts";
import { plainText } from "../src/ui/theme.ts";
import type { RuntimeClientSnapshot } from "../src/protocol/types.ts";
import {
  approvalInteraction,
  assistantBlocks,
  attemptModel,
  attemptView,
  backgroundExecution,
  foreground,
  sessionModel,
  snapshot,
  toolCallBlock,
  toolMessage,
  toolResult,
  userMessage,
  runtimeCursor,
} from "./support/fixtures.ts";
import { blockText, prefs } from "./support/render.ts";

/**
 * One snapshot with a representative mix of everything the UI shows:
 * conversation history, a settled tool call, a streaming assistant message
 * with a live tool call, background work, a pending approval, model state,
 * usage, and pending inbound.
 */
function representative(): RuntimeClientSnapshot {
  return snapshot({
    messages: [
      userMessage("m1", "run the tests"),
      assistantBlocks("m2", [
        { type: "reasoning", text: "I should run cargo test" },
        { type: "text", text: "Running the suite." },
        toolCallBlock("call-1", "tool-bash", "bash", { command: "cargo test" }),
      ]),
      toolMessage("m3", "call-1", "tool-bash", toolResult({
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
      })),
    ],
    attempt: attemptView({
      attempt_id: "attempt-1",
      turn: 3,
      last_usage: { input_tokens: 12_500, output_tokens: 840, total_tokens: 13_340 },
      execution_settings: null,
      model: attemptModel("alpha/model-a"),
      in_flight: {
        message_id: "m4",
        blocks: [
          { type: "text", block_index: 0, text: "Now checking the lints." },
          {
            type: "tool_call",
            block_index: 1,
            call_id: "call-2",
            tool_id: "tool-grep",
            name: "grep",
            arguments: '{"pattern":"AttemptSettled","path":"src"}',
          },
        ],
      },
      foreground: [
        foreground("call-1", "tool-bash", "bash", {
          type: "settled",
          arguments: '{"command":"cargo test"}',
          result: toolResult({ exit_code: 0, duration_ms: 2_800 }),
        }),
        foreground("call-2", "tool-grep", "grep", {
          type: "running",
          arguments: '{"pattern":"AttemptSettled","path":"src"}',
          progress: { message: "scanning", completed: 40, total: 900 },
        }),
      ],
    }),
    background: [backgroundExecution("exec-1", "running")],
    pending_interactions: [approvalInteraction()],
    model: sessionModel("beta/model-b"),
    inbound: {
      pending: [
        {
          sequence: 7,
          message: {
            id: "m5",
            content: [{ type: "text", text: "queued" }],
            source: "human",
            kind: "message",
          },
        },
      ],
    },
  });
}

/** Everything the screen shows, as plain text, from one state. */
function visible(
  state: ReturnType<typeof replaceFromSnapshot>,
  preferences = prefs(),
): string {
  const correlation = correlateTools(state);
  return plainText(
    [
      ...renderTranscript(state, preferences).map(blockText),
      renderOrphanExecutions(correlation, preferences),
      renderBackgroundSection(state, preferences),
      renderInteractionSection(state, preferences),
      workingStatus(state) ?? "",
      renderFooter(state, "connected", 120),
    ].join("\n"),
  );
}

describe("snapshot reconstruction", () => {
  it("rebuilds every semantic region from one fresh snapshot", () => {
    const screen = visible(replaceFromSnapshot(representative(), runtimeCursor(42)));

    // Conversation
    assert.match(screen, /run the tests/, "the user turn");
    assert.match(screen, /I should run cargo test/, "reasoning");
    assert.match(screen, /Running the suite\./, "the answer");
    assert.match(screen, /Now checking the lints\./, "the streaming answer");

    // The settled tool card, joined from three separate runtime facts
    assert.match(screen, /✓ Bash \$ cargo test · ok · 2\.8s · exit 0/);
    assert.match(screen, /test result: ok\. 842 passed/, "the committed result body");

    // The live tool card, with runtime progress
    assert.match(screen, /◐ Grep "AttemptSettled" · running · scanning · 40\/900/);

    // Activity
    assert.match(screen, /Background · 1 active of 1 known/);
    assert.match(screen, /Human input required · 1 pending/);

    // Working status: the pending approval for this attempt outranks the
    // running execution, because it is what the runtime is actually waiting on.
    assert.match(screen, /Waiting for approval of bash…/);

    // Footer: stable model/policy/usage; activity remains on the working surface.
    assert.match(screen, /beta\/model-b/);
    assert.match(screen, /attempt alpha\/model-a/);
    assert.match(screen, /Waiting for approval of bash…/);
    assert.match(screen, /↑12\.5k ↓840/);
    assert.doesNotMatch(screen, /queued 1|background 1/);
    assert.equal(screen.match(/Waiting for approval of bash…/g)?.length, 1);
  });

  it("is identical whether the state is fresh or replaced in place", () => {
    // A resync replaces the projection wholesale. The second render must not
    // depend on anything the first render left behind.
    const first = visible(replaceFromSnapshot(representative(), runtimeCursor(42)));
    const second = visible(replaceFromSnapshot(representative(), runtimeCursor(99)));
    assert.equal(first, second);
  });

  it("reconstructs the same screen with every domain expanded", () => {
    // Expansion is client state, so it is not in the snapshot — but the facts
    // it reveals are, and two independent rebuilds must reveal the same ones.
    const state = replaceFromSnapshot(representative(), runtimeCursor(42));
    const preferences = withExpandedInteractions(
      withExpandedBackgroundExecutions(
        withExpandedToolCalls(prefs(), ["call-1", "call-2"]),
        ["exec-1"],
      ),
      [approvalInteraction().interaction],
    );
    const first = visible(state, preferences);
    const second = visible(
      replaceFromSnapshot(representative(), runtimeCursor(99)),
      preferences,
    );
    assert.equal(first, second);

    // The expanded approval shows the complete published request, and nothing
    // was written back into the projection to produce it.
    assert.match(first, /native policy requires approval/);
    assert.match(first, /printf original/);
    assert.deepEqual(
      state,
      replaceFromSnapshot(representative(), runtimeCursor(42)),
      "expanding never writes back into runtime state",
    );
  });

  it("CFG238 reasoning visibility changes rendering without touching model, tools or canonical history", () => {
    const native = representative();
    const state = replaceFromSnapshot(native, runtimeCursor(42));
    const before = structuredClone(state);
    const visible = renderTranscript(state, withReasoningVisible(prefs(), true)).map(blockText).join("\n");
    const hidden = renderTranscript(state, withReasoningVisible(prefs(), false)).map(blockText).join("\n");
    assert.notEqual(visible, hidden);
    assert.match(visible, /I should run cargo test/);
    assert.ok(!hidden.includes("I should run cargo test"));
    assert.deepEqual(state, before);
    assert.deepEqual(state, replaceFromSnapshot(native, runtimeCursor(42)));
  });

  it("carries no semantic state in a display preference", () => {
    const state = replaceFromSnapshot(representative(), runtimeCursor(42));
    const collapsed = renderTranscript(state, prefs()).map(blockText);
    const expanded = renderTranscript(
      state,
      withToggledToolCall(prefs(), "call-1"),
    ).map(blockText);

    // Expanding changes how much of a result is drawn and nothing else: the
    // same blocks, the same order, the same runtime facts.
    assert.equal(collapsed.length, expanded.length);
    for (const [index, block] of collapsed.entries()) {
      const other = expanded[index] ?? "";
      assert.equal(
        block.split("\n")[0],
        other.split("\n")[0],
        `block ${index} keeps its identity line`,
      );
    }
    // And the projection itself is untouched by either render.
    assert.deepEqual(
      state,
      replaceFromSnapshot(representative(), runtimeCursor(42)),
      "rendering never writes back into runtime state",
    );
  });
});

it("pure Tool/footer/approval rendering performs no filesystem, process, or native control effects", async (t) => {
  const fs = await import("node:fs");
  const childProcess = await import("node:child_process");
  const net = await import("node:net");
  const { ApprovalSelector } = await import("../src/ui/components/approval-selector.ts");
  const { renderToolCard } = await import("../src/ui/components/tool-card.ts");
  const { rendererFor } = await import("../src/ui/components/tool-renderers.ts");
  const state = replaceFromSnapshot(representative(), runtimeCursor(42));
  const tools = correlateTools(state);
  const nativeControls: unknown[] = [];
  const picker = new ApprovalSelector({ state: () => state,
    submit: async (mode) => { nativeControls.push(mode); }, close: () => {}, change: () => {},
  });
  // Imports/fixtures are complete before effect tripwires are installed.
  t.mock.method(globalThis, "fetch", () => assert.fail("render called a provider/network service"));
  t.mock.method(net.Socket.prototype, "connect", () => assert.fail("render opened a provider/runtime socket"));
  for (const key of ["readFileSync", "readFile", "readdirSync", "statSync"] as const) {
    t.mock.method(fs.default, key, () => assert.fail(`render performed filesystem ${key}`));
  }
  for (const key of ["spawn", "spawnSync", "exec", "execSync", "execFile", "execFileSync"] as const) {
    t.mock.method(childProcess.default, key, () => assert.fail(`render invoked a Tool/Git/process through ${key}`));
  }
  const adapter = rendererFor("tool-bash");
  const original = adapter.renderResult!;
  t.mock.method(adapter as Required<typeof adapter>, "renderResult", (content: Parameters<typeof original>[0], args: unknown) => {
    assert.deepEqual(Object.keys(content), ["content"], "specialized renderers receive no lifecycle authority");
    return original(content, args);
  });
  const rendered = renderFooter(state, "connected");
  assert.match(rendered, /approval POLICY/);
  for (const tool of tools.byCallId.values()) renderToolCard(tool, { expanded: true, budget: prefs().previewBudget });
  picker.render(100); picker.handleInput("\x1b[B"); picker.render(100);
  picker.handleInput("\r"); picker.render(100); picker.handleInput("\x1b");
  assert.deepEqual(nativeControls, []);
});
