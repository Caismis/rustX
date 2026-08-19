/**
 * Working status, footer, and the activity area.
 *
 * The rule under test: every state shown is proven by a projection fact. No
 * timer, no inactivity threshold, and no client guess about a phase rustX did
 * not publish.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { reduce } from "../src/presentation/projection.ts";
import { correlateTools } from "../src/presentation/tools.ts";
import {
  renderBackgroundSection,
  renderInteractionSection,
  renderOrphanExecutions,
} from "../src/ui/components/activity.ts";
import { renderFooter, workingStatus } from "../src/ui/components/status.ts";
import { plainText } from "../src/ui/theme.ts";
import {
  approvalInteraction,
  attemptModel,
  attemptView,
  backgroundExecution,
  foreground,
  sessionModel,
  toolResult,
} from "./support/fixtures.ts";
import { prefs, stateOf } from "./support/render.ts";

const footer = (...args: Parameters<typeof renderFooter>) =>
  plainText(renderFooter(...args));

describe("working status", () => {
  it("is absent when there is no attempt and when one has settled", () => {
    assert.equal(workingStatus(stateOf()), undefined);
    assert.equal(
      workingStatus(
        stateOf({
          attempt: attemptView({
            phase: { type: "settled", outcome: { type: "timed_out" } },
          }),
        }),
      ),
      undefined,
    );
  });

  it("reports admission before the loop starts", () => {
    assert.equal(
      workingStatus(stateOf({ attempt: attemptView({ phase: { type: "admitted" } }) })),
      "Admitted…",
    );
  });

  it("names the tool the runtime says is running", () => {
    assert.equal(
      workingStatus(
        stateOf({
          attempt: attemptView({
            foreground: [
              foreground("call-1", "tool-bash", "bash", {
                type: "running",
                arguments: "{}",
              }),
            ],
          }),
        }),
      ),
      "Running bash…",
    );
  });

  it("reports an assembled-but-not-started call as preparation", () => {
    assert.equal(
      workingStatus(
        stateOf({
          attempt: attemptView({
            foreground: [
              foreground("call-1", "tool-bash", "bash", {
                type: "assembled",
                arguments: "{}",
              }),
            ],
          }),
        }),
      ),
      "Preparing tool call…",
    );
  });

  it("distinguishes thinking from streaming by the latest published block", () => {
    let state = stateOf();
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
      type: "assistant_reasoning_delta",
      attempt_id: "a1",
      message_id: "m1",
      block_index: 0,
      delta: "hmm",
    });
    assert.equal(workingStatus(state), "Thinking…");

    push({
      type: "assistant_text_delta",
      attempt_id: "a1",
      message_id: "m1",
      block_index: 1,
      delta: "well",
    });
    assert.equal(workingStatus(state), "Streaming response…");
  });

  it("reports a pending approval for the active attempt", () => {
    const interaction = approvalInteraction();
    assert.equal(
      workingStatus(
        stateOf({
          attempt: attemptView({ attempt_id: "attempt-1" }),
          pending_interactions: [interaction],
        }),
      ),
      "Waiting for approval of bash…",
    );
  });

  it("does not attribute another attempt's approval to this one", () => {
    assert.equal(
      workingStatus(
        stateOf({
          attempt: attemptView({ attempt_id: "a-other" }),
          pending_interactions: [approvalInteraction()],
        }),
      ),
      "Working… (turn 1)",
    );
  });
});

describe("footer", () => {
  it("shows the session model, capability revision, and connection state", () => {
    const rendered = footer(
      stateOf({
        model: sessionModel("alpha/model-a"),
        capabilities: { revision: 3 },
      }),
      "connected",
    );
    assert.match(rendered, /alpha\/model-a/);
    assert.match(rendered, /cap r3/);
    assert.match(rendered, /connected/);
  });

  it("shows the attempt's frozen model when the session moved on", () => {
    const rendered = footer(
      stateOf({
        model: sessionModel("beta/model-b"),
        attempt: attemptView({ turn: 2, model: attemptModel("alpha/model-a") }),
      }),
      "connected",
    );
    assert.match(rendered, /beta\/model-b/, "the desired session model");
    assert.match(rendered, /attempt alpha\/model-a/, "the frozen attempt model");
    assert.match(rendered, /running · turn 2/);
  });

  it("collapses to one model when both agree", () => {
    const rendered = footer(
      stateOf({
        model: sessionModel("alpha/model-a"),
        attempt: attemptView({ model: attemptModel("alpha/model-a") }),
      }),
      "connected",
    );
    assert.ok(!rendered.includes("attempt alpha/model-a"));
  });

  it("shows a settled attempt's outcome rather than a phase", () => {
    const rendered = footer(
      stateOf({
        attempt: attemptView({
          phase: { type: "settled", outcome: { type: "cancelled", reason: "user_requested" } },
        }),
      }),
      "connected",
    );
    assert.match(rendered, /cancelled \(user_requested\)/);
  });

  it("distinguishes known usage from usage the runtime has not published", () => {
    assert.match(
      footer(
        stateOf({
          attempt: attemptView({
            last_usage: { input_tokens: 12_500, output_tokens: 840, total_tokens: 13_340 },
          }),
        }),
        "connected",
      ),
      /12\.5kin 840out/,
    );
    assert.match(
      footer(stateOf({ attempt: attemptView() }), "connected"),
      /usage pending/,
    );
  });

  it("counts pending inbound, background, and approvals", () => {
    const rendered = footer(
      stateOf({
        background: [backgroundExecution("exec-1", "running")],
        pending_interactions: [approvalInteraction()],
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
      }),
      "connected",
    );
    assert.match(rendered, /inbox 1/);
    assert.match(rendered, /bg 1/);
    assert.match(rendered, /approvals 1/);
  });

  it("reports drain and a closed transport without implying cancellation", () => {
    const rendered = footer(stateOf({ shutting_down: true }), "closed: input_eof");
    assert.match(rendered, /shutting down/);
    assert.match(rendered, /closed: input_eof/);
    assert.ok(!/cancelled/i.test(rendered));
  });

  it("degrades on a narrow terminal instead of writing one unbounded line", () => {
    const state = stateOf({
      model: sessionModel("alpha/model-a"),
      attempt: attemptView({
        model: attemptModel("beta/model-b"),
        last_usage: { input_tokens: 12_500, output_tokens: 840, total_tokens: 13_340 },
      }),
      background: [backgroundExecution("exec-1", "running")],
      capabilities: { revision: 9 },
    });

    const wide = footer(state, "connected", 200);
    assert.equal(wide.split("\n").length, 1);
    assert.match(wide, /cap r9/);

    const narrow = footer(state, "connected", 40);
    const rows = narrow.split("\n");
    assert.ok(rows.length <= 2, "never more than two rows");
    for (const row of rows) {
      assert.ok(row.length <= 40, `row within width: ${JSON.stringify(row)}`);
    }
    // The essential identities survive; the low-priority revision is dropped.
    assert.match(narrow, /alpha\/model-a/);
    assert.match(narrow, /attempt beta\/model-b/);
    assert.ok(!narrow.includes("cap r9"));
  });
});

describe("activity area", () => {
  it("renders nothing when the runtime knows of no background work", () => {
    assert.equal(renderBackgroundSection(stateOf(), prefs()), "");
    assert.equal(renderInteractionSection(stateOf()), "");
  });

  it("shows active and terminal background executions with runtime state", () => {
    const rendered = plainText(
      renderBackgroundSection(
        stateOf({
          background: [
            backgroundExecution("exec-1", "running", {
              progress: { message: "step 2" },
            }),
            backgroundExecution("exec-2", "succeeded", { result: toolResult() }),
          ],
        }),
        prefs(),
      ),
    );
    assert.match(rendered, /Background · 1 active of 2 known/);
    assert.match(rendered, /running exec-1/);
    assert.match(rendered, /succeeded exec-2/);
    assert.match(rendered, /step 2/);
  });

  it("shows runtime-owned approval facts and the typed response command", () => {
    const rendered = plainText(
      renderInteractionSection(
        stateOf({ pending_interactions: [approvalInteraction()] }),
      ),
    );
    assert.match(rendered, /Approval required · 1 pending/);
    assert.match(rendered, /bash/);
    assert.match(rendered, /attempt-1-interaction-1/);
    assert.match(rendered, /native policy requires approval/);
    assert.match(rendered, /printf original/);
    assert.match(rendered, /\/approve <interaction-id> <allow\|deny> \[reason\]/);
  });

  it("surfaces an execution whose stream was dropped", () => {
    const state = stateOf({
      attempt: attemptView({
        phase: { type: "settled", outcome: { type: "timed_out" } },
        foreground: [
          foreground("call-1", "tool-bash", "bash", {
            type: "settled",
            arguments: '{"command":"sleep 999"}',
            result: toolResult({ status: { type: "timed_out" } }),
          }),
        ],
      }),
    });
    const rendered = plainText(
      renderOrphanExecutions(correlateTools(state), prefs()),
    );
    assert.match(rendered, /Executions from an uncommitted turn/);
    assert.match(rendered, /Bash/);
    assert.match(rendered, /timed out/);
  });

  it("renders nothing when every execution has a transcript anchor", () => {
    const state = stateOf({
      messages: [
        {
          role: "assistant",
          id: "m1",
          content: [
            {
              type: "tool_call",
              id: "call-1",
              tool_id: "tool-bash",
              name: "bash",
              arguments: { command: "ls" },
            },
          ],
        },
      ],
      attempt: attemptView({
        foreground: [
          foreground("call-1", "tool-bash", "bash", {
            type: "running",
            arguments: '{"command":"ls"}',
          }),
        ],
      }),
    });
    assert.equal(renderOrphanExecutions(correlateTools(state), prefs()), "");
  });
});
