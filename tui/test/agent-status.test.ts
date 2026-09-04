/**
 * Agent Status as a deterministic contextual annotation.
 *
 * The invariant under test: *every composed Agent Status has exactly one
 * deterministic presentation anchor, and once observable in conversation
 * history its placement does not change or disappear because a later attempt
 * starts.*
 *
 * Every case drives exact runtime facts — a `status_message_id`, a
 * `target_message_id`, a durable transcript cursor — and asserts the
 * resulting placement. Nothing here sleeps, reads a clock, or depends on the
 * order things happened to arrive in.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  reduce,
  replaceFromSnapshot,
} from "../src/presentation/projection.ts";
import {
  agentStatusAnchor,
  latestAgentStatus,
} from "../src/presentation/selectors.ts";
import type { PresentationState } from "../src/presentation/state.ts";
import type { RuntimeClientEvent } from "../src/protocol/types.ts";
import {
  agentStatusFacets,
  agentStatusSummary,
  formatStatusTime,
  renderAgentStatusDetail,
} from "../src/ui/components/agent-status.ts";
import {
  agentStatus,
  assistantBlocks,
  assistantMessage,
  backgroundExecution,
  backgroundSection,
  contextUserMessage,
  runtimeCursor,
  snapshot,
  temporalSection,
  todoSection,
  toolCallBlock,
  toolMessage,
  transcriptCursor,
  userMessage,
} from "./support/fixtures.ts";
import { stateOf, transcriptText } from "./support/render.ts";
import { renderTranscript } from "../src/ui/components/transcript.ts";
import { defaultPreferences } from "../src/ui/preferences.ts";

function renderBlocks(state: PresentationState) {
  return renderTranscript(state, defaultPreferences());
}

function fold(
  state: PresentationState,
  events: RuntimeClientEvent[],
): PresentationState {
  let current = state;
  let cursor = current.cursor;
  for (const event of events) {
    cursor = runtimeCursor(cursor + 1);
    current = reduce(current, { cursor, event });
  }
  return current;
}

/** The indexes of every rendered status annotation, in transcript order. */
function annotationIndexes(lines: string[]): number[] {
  return lines
    .map((line, index) => (line.startsWith("◇ status") ? index : -1))
    .filter((index) => index >= 0);
}

describe("Agent Status placement", () => {
  it("renders a FreshInbound status once, beneath the referenced turn", () => {
    const state = stateOf({
      messages: [userMessage("m1", "Help me analyze this issue")],
      statuses: [
        agentStatus({
          status_message_id: "status-1",
          opportunities: { fresh_inbound: { target_message_id: "m1" } },
          sections: [temporalSection("2026-08-14T15:42:00Z"), todoSection({ active_count: 3 })],
        }),
      ],
    });

    const lines = transcriptText(state);
    const anchors = annotationIndexes(lines);
    assert.equal(anchors.length, 1, "one composition renders once");
    const [annotation] = anchors;
    assert.ok(annotation !== undefined);
    assert.match(lines[annotation - 1] ?? "", /Help me analyze this issue/);
    assert.equal(lines[annotation], "◇ status · 15:42 UTC · todo 3");
  });

  it("does not give the annotation the user turn's background band", () => {
    const state = stateOf({
      messages: [userMessage("m1", "hello")],
      statuses: [
        agentStatus({
          status_message_id: "status-1",
          opportunities: { fresh_inbound: { target_message_id: "m1" } },
        }),
      ],
    });

    const blocks = renderBlocks(state);
    const user = blocks.find((block) => block.background === "user");
    assert.ok(user !== undefined, "the human turn keeps its band");
    const annotation = blocks.find((block) =>
      block.key.startsWith("agent-status:"),
    );
    assert.ok(annotation !== undefined);
    assert.equal(
      annotation.background,
      undefined,
      "the annotation is runtime metadata, not part of what the human said",
    );
  });

  it("anchors a PostToolBatch-only status to its published transcript position", () => {
    const state = stateOf({
      messages: [
        userMessage("m1", "go"),
        assistantBlocks("m2", [
          { type: "text", text: "I will inspect the implementation." },
          toolCallBlock("call-1", "tool-read", "read", { path: "src/foo.rs" }),
        ]),
        toolMessage("m3", "call-1", "tool-read"),
        assistantMessage("m4", "The boundary behaves as follows."),
      ],
      statuses: [
        agentStatus({
          status_message_id: "status-batch",
          turn: 2,
          opportunities: { post_tool_batch: { transcript_anchor: transcriptCursor(3) } },
          sections: [todoSection({ active_count: 2 }), backgroundSection([
            backgroundExecution("exec-1", "running"),
          ])],
        }),
      ],
    });

    const lines = transcriptText(state);
    const anchors = annotationIndexes(lines);
    assert.deepEqual(
      anchors.map((index) => lines[index]),
      ["◇ status update · todo 2 · background 1"],
    );
    const [annotation] = anchors;
    assert.ok(annotation !== undefined);
    // After the settled tool card, before the assistant text that follows it.
    assert.match(lines[annotation + 1] ?? "", /The boundary behaves as follows/);
  });

  it("keeps a PostToolBatch status above an unrelated inbound accepted after it", () => {
    // The runtime published the tool batch's own position (cursor 3). The
    // conversation then durably accepted an unrelated inbound turn at cursor
    // 4 before the composition was even observed. Placement follows the
    // published fact, so the annotation stays with the tool batch that
    // caused it.
    const state = stateOf({
      messages: [
        userMessage("m1", "go"),
        assistantBlocks("m2", [
          { type: "text", text: "I will inspect the implementation." },
          toolCallBlock("call-1", "tool-read", "read", { path: "src/foo.rs" }),
        ]),
        toolMessage("m3", "call-1", "tool-read"),
        userMessage("m4", "and another thing"),
      ],
      statuses: [
        agentStatus({
          status_message_id: "status-batch",
          turn: 2,
          opportunities: {
            post_tool_batch: { transcript_anchor: transcriptCursor(3) },
          },
          sections: [todoSection({ active_count: 2 })],
        }),
      ],
    });

    const lines = transcriptText(state);
    const annotation = annotationIndexes(lines)[0];
    assert.ok(annotation !== undefined, "the composition is drawn once");
    const unrelated = lines.findIndex((line) =>
      line.includes("and another thing"),
    );
    assert.ok(unrelated >= 0, "the unrelated inbound turn is on screen");
    assert.ok(
      annotation < unrelated,
      "the annotation belongs to the tool batch, not to the later inbound turn",
    );
  });

  it("renders a doubly-eligible composition once, at the FreshInbound anchor", () => {
    const status = agentStatus({
      status_message_id: "status-both",
      opportunities: {
        fresh_inbound: { target_message_id: "m1" },
        post_tool_batch: { transcript_anchor: transcriptCursor(3) },
      },
      sections: [todoSection({ active_count: 1 })],
    });
    const state = stateOf({
      messages: [
        userMessage("m1", "first"),
        assistantMessage("m2", "answer"),
        userMessage("m3", "second"),
      ],
      statuses: [status],
    });

    const lines = transcriptText(state);
    const anchors = annotationIndexes(lines);
    assert.equal(anchors.length, 1, "one composition, one annotation");
    const [annotation] = anchors;
    assert.ok(annotation !== undefined);
    assert.match(lines[annotation - 1] ?? "", /first/);
    assert.deepEqual(agentStatusAnchor(status), {
      kind: "inbound_message",
      messageId: "m1",
    });
  });

  it("keeps an earlier annotation where it was when a later attempt starts", () => {
    const first = agentStatus({
      status_message_id: "status-1",
      opportunities: { fresh_inbound: { target_message_id: "m1" } },
      sections: [todoSection({ active_count: 1 })],
    });
    const base = stateOf({
      messages: [userMessage("m1", "first"), userMessage("m2", "second")],
      statuses: [first],
    });
    const before = transcriptText(base);

    const after = fold(base, [
      {
        type: "attempt_started",
        attempt_id: "a2",
        model: base.sessionModel.effective as never,
      },
    ]);
    assert.deepEqual(transcriptText(after), before);
    assert.deepEqual(after.statuses, [first]);
  });

  it("adds a later status at its own anchor without moving the earlier one", () => {
    const first = agentStatus({
      status_message_id: "status-1",
      opportunities: { fresh_inbound: { target_message_id: "m1" } },
      sections: [todoSection({ active_count: 1 })],
    });
    const second = agentStatus({
      status_message_id: "status-2",
      attempt_id: "a2",
      opportunities: { fresh_inbound: { target_message_id: "m2" } },
      sections: [todoSection({ active_count: 4 })],
    });
    const state = fold(
      stateOf({
        messages: [userMessage("m1", "first"), userMessage("m2", "second")],
        statuses: [first],
      }),
      [{ type: "agent_status_composed", attempt_id: "a2", turn: 1, status: second }],
    );

    const lines = transcriptText(state);
    const anchors = annotationIndexes(lines);
    assert.deepEqual(
      anchors.map((index) => lines[index]),
      ["◇ status · todo 1", "◇ status · todo 4"],
    );
    assert.match(lines[anchors[0]! - 1] ?? "", /first/);
    assert.match(lines[anchors[1]! - 1] ?? "", /second/);
  });

  it("draws nothing for a status whose anchor is not in the loaded transcript", () => {
    const state = stateOf({
      messages: [userMessage("m2", "second")],
      statuses: [
        agentStatus({
          status_message_id: "status-old",
          opportunities: { fresh_inbound: { target_message_id: "m1" } },
          sections: [todoSection({ active_count: 1 })],
        }),
      ],
    });

    assert.deepEqual(annotationIndexes(transcriptText(state)), []);
  });

  it("does not duplicate a replayed composition", () => {
    const status = agentStatus({
      status_message_id: "status-1",
      opportunities: { fresh_inbound: { target_message_id: "m1" } },
      sections: [todoSection({ active_count: 1 })],
    });
    const state = fold(stateOf({ messages: [userMessage("m1", "hi")] }), [
      { type: "agent_status_composed", attempt_id: "a1", turn: 1, status },
      { type: "agent_status_composed", attempt_id: "a1", turn: 1, status },
    ]);

    assert.equal(state.statuses.length, 1);
    assert.equal(annotationIndexes(transcriptText(state)).length, 1);
  });

  it("keeps the canonical Context(AgentStatus) message out of ordinary rows", () => {
    const hidden = contextUserMessage(
      "status-1",
      "<system-reminder>\nTimezone: UTC\n</system-reminder>",
    );
    // The canonical Context message is on the Surface, as model history. It
    // carries no durable transcript cursor and is not a transcript item, so
    // the loaded page holds only the visible turn.
    const state = stateOf({
      messages: [userMessage("m1", "hello"), hidden],
      transcript: {
        entries: [
          {
            cursor: transcriptCursor(1),
            item: { type: "message", message: userMessage("m1", "hello") },
          },
        ],
      },
      statuses: [
        agentStatus({
          status_message_id: "status-1",
          opportunities: { fresh_inbound: { target_message_id: "m1" } },
          sections: [temporalSection("2026-08-14T15:42:00Z")],
        }),
      ],
    });

    // The live path agrees: a hidden Context commit produces no row either.
    const live = fold(state, [
      { type: "message_committed", attempt_id: "a1", message: hidden },
    ]);

    for (const lines of [transcriptText(state), transcriptText(live)]) {
      assert.ok(
        !lines.some((line) => line.includes("system-reminder")),
        "the model-facing Context body never becomes a transcript row",
      );
      assert.equal(annotationIndexes(lines).length, 1);
    }
  });
});

describe("Agent Status projection convergence", () => {
  const first = agentStatus({
    status_message_id: "status-1",
    opportunities: { fresh_inbound: { target_message_id: "m1" } },
    sections: [temporalSection("2026-08-14T15:42:00Z"), todoSection({ active_count: 2 })],
  });
  const second = agentStatus({
    status_message_id: "status-2",
    turn: 2,
    opportunities: { post_tool_batch: { transcript_anchor: transcriptCursor(2) } },
    sections: [backgroundSection([backgroundExecution("exec-1", "running")])],
  });
  const messages = [userMessage("m1", "hello"), assistantMessage("m2", "answer")];

  it("places statuses identically whether folded or repaired", () => {
    const folded = fold(stateOf({ messages }), [
      { type: "agent_status_composed", attempt_id: "a1", turn: 1, status: first },
      { type: "agent_status_composed", attempt_id: "a1", turn: 2, status: second },
    ]);
    const repaired = stateOf({ messages, statuses: [first, second] });

    assert.deepEqual(repaired.statuses, folded.statuses);
    assert.deepEqual(transcriptText(repaired), transcriptText(folded));
  });

  it("repairs from the snapshot alone, never from the state it replaces", () => {
    const populated = stateOf({ messages, statuses: [first, second] });
    assert.equal(populated.statuses.length, 2);

    // A runtime that has projected only the newer composition produces a
    // client that shows only the newer composition. Nothing survives from
    // the state being replaced.
    const repaired = replaceFromSnapshot(
      snapshot({ messages, statuses: [second] }),
      runtimeCursor(5),
    );
    assert.deepEqual(repaired.statuses, [second]);
    assert.deepEqual(
      transcriptText(repaired),
      transcriptText(stateOf({ messages, statuses: [second] })),
    );
  });

  it("derives the latest status from composition order, not a second field", () => {
    const state = stateOf({ messages, statuses: [first, second] });
    assert.equal(latestAgentStatus(state), second);
    assert.equal(latestAgentStatus(stateOf({ messages })), undefined);
  });
});

describe("typed Agent Status sections", () => {
  it("renders the temporal section in the runtime's own timezone", () => {
    assert.equal(formatStatusTime("2026-08-14T15:42:00Z"), "15:42 UTC");
    assert.equal(
      formatStatusTime("2026-01-14T16:42:00Z", "America/Chicago"),
      "10:42 CST",
    );
    // A shape this client cannot parse is shown, not guessed at.
    assert.equal(formatStatusTime("not-a-timestamp"), "not-a-timestamp");
  });

  it("renders the todo section from committed counts", () => {
    const status = agentStatus({
      status_message_id: "s",
      sections: [
        todoSection({
          current: {
            id: 7,
            subject: "Review cancellation boundary",
            active_form: "Reviewing cancellation boundary",
            status: "in_progress",
            blocked: false,
          },
          active_count: 3,
          blocked_count: 1,
          completed_count: 2,
        }),
      ],
    });

    assert.equal(agentStatusSummary(status, "status"), "◇ status · todo 3");
    assert.deepEqual(renderAgentStatusDetail(status), [
      "- **todo** — Reviewing cancellation boundary",
      "  - 3 active · 1 blocked · 2 completed",
    ]);
  });

  it("renders the background section including the runtime's omission count", () => {
    const status = agentStatus({
      status_message_id: "s",
      sections: [
        backgroundSection(
          [
            backgroundExecution("exec-1", "running"),
            backgroundExecution("exec-2", "running", { tool_name: "explore" }),
          ],
          3,
        ),
      ],
    });

    assert.equal(agentStatusSummary(status, "status"), "◇ status · background 5");
    assert.deepEqual(renderAgentStatusDetail(status), [
      "- **background** — bash · running",
      "  - explore · running",
      "  - … and 3 more",
    ]);
  });

  it("omits a section the runtime published with nothing to say", () => {
    const status = agentStatus({
      status_message_id: "s",
      sections: [todoSection(), backgroundSection([])],
    });
    assert.deepEqual(agentStatusFacets(status), []);
    assert.equal(agentStatusSummary(status, "status"), "◇ status");
  });

  it("stays one bounded line however long a published value is", () => {
    const long = "x".repeat(4_000);
    const status = agentStatus({
      status_message_id: "status-long",
      opportunities: { fresh_inbound: { target_message_id: "m1" } },
      sections: [
        todoSection({
          current: {
            id: 1,
            subject: `${long}\n${long}`,
            status: "pending",
            blocked: false,
          },
          active_count: 1,
        }),
        backgroundSection([
          backgroundExecution("exec-1", "running", { tool_name: long }),
        ]),
      ],
    });
    const state = stateOf({
      messages: [userMessage("m1", "hi"), assistantMessage("m2", "answer")],
      statuses: [status],
    });

    const lines = transcriptText(state);
    const anchors = annotationIndexes(lines);
    assert.equal(anchors.length, 1);
    const annotation = lines[anchors[0]!]!;
    assert.equal(annotation.split("\n").length, 1, "the annotation is one row");
    assert.ok(annotation.length <= 80, `annotation was ${annotation.length} wide`);
    // Ordering is unaffected: the annotation still sits between its own turn
    // and the assistant answer that follows.
    assert.match(lines[anchors[0]! - 1] ?? "", /hi/);
    assert.match(lines[anchors[0]! + 1] ?? "", /answer/);

    // The detail form bounds each externally derived value too.
    for (const line of renderAgentStatusDetail(status)) {
      assert.equal(line.split("\n").length, 1);
      assert.ok(line.length <= 200, `detail line was ${line.length} wide`);
    }
  });
});
