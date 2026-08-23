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
  renderBackground,
  renderBackgroundSection,
  renderInteractionSection,
  renderOrphanExecutions,
} from "../src/ui/components/activity.ts";
import {
  contextLabel,
  renderFooter,
  renderStartup,
  startupVisible,
  workingStatus,
} from "../src/ui/components/status.ts";
import { plainText } from "../src/ui/theme.ts";
import type { InteractionRequest } from "../src/protocol/types.ts";
import {
  DEFAULT_PREVIEW_CHARS,
  withExpandedBackgroundExecutions,
  withExpandedInteractions,
} from "../src/ui/preferences.ts";
import {
  approvalInteraction,
  attemptModel,
  attemptView,
  backgroundExecution,
  foreground,
  sessionModel,
  toolResult,
  userMessage,
  questionInteraction,
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

  it("reports a pending Question as an answer request", () => {
    assert.equal(
      workingStatus(
        stateOf({
          attempt: attemptView({ attempt_id: "attempt-1" }),
          pending_interactions: [questionInteraction()],
        }),
      ),
      "Waiting for an answer…",
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
      "Working…",
    );
  });
});

describe("footer", () => {
  it("shows the active model, provider, context, and connection state", () => {
    const rendered = footer(
      stateOf({
        model: sessionModel("alpha/model-a"),
        capabilities: { revision: 3 },
      }),
      "connected",
    );
    assert.match(rendered, /alpha\/model-a/);
    assert.match(rendered, /provider alpha/);
    assert.match(rendered, /context —\/128k/);
    assert.match(rendered, /online/);
    assert.doesNotMatch(rendered, /cap r3/);
  });

  it("shows the native active Session and node when published", () => {
    const rendered = footer(
      stateOf(),
      "connected",
      120,
      {
        id: "session-7",
        name: "review branch",
        created_at: "2026-08-21T00:00:00Z",
        updated_at: "2026-08-21T00:00:00Z",
        active_node: "node-3",
        active_conversation_id: "conv-3",
        node_count: 1,
      },
    );
    assert.match(rendered, /session review branch/);
    assert.match(rendered, /node node-3/);
  });

  it("surfaces unavailable optional capabilities without dying (Issue #81)", () => {
    const healthy = footer(
      stateOf({ capabilities: { revision: 3, sources: [] } }),
      "connected",
    );
    assert.doesNotMatch(healthy, /unavailable/);

    const degraded = footer(
      stateOf({
        capabilities: {
          revision: 3,
          sources: [
            { source: { type: "python" }, state: { type: "ready" } },
            {
              source: { type: "mcp", server_id: "exa" },
              state: { type: "unavailable", reason: "spawn failed" },
            },
            {
              source: { type: "mcp", server_id: "filesystem" },
              state: { type: "ready" },
            },
          ],
        },
      }),
      "connected",
    );
    assert.match(degraded, /1 optional capability unavailable/);
    assert.doesNotMatch(degraded, /cap r3/);
  });

  it("shows the attempt's frozen model when the session moved on", () => {
    const rendered = footer(
      stateOf({
        model: sessionModel("beta/model-b"),
        attempt: attemptView({ turn: 2, model: attemptModel("alpha/model-a") }),
      }),
      "connected",
    );
    assert.match(rendered, /cfg beta\/model-b/, "the desired session model");
    assert.match(rendered, /attempt alpha\/model-a/, "the frozen attempt model");
    assert.match(rendered, /Working…/);
  });

  it("collapses to one model when all of them agree", () => {
    const rendered = footer(
      stateOf({
        model: sessionModel("alpha/model-a"),
        attempt: attemptView({ model: attemptModel("alpha/model-a") }),
      }),
      "connected",
    );
    assert.ok(!rendered.includes("attempt alpha/model-a"));
    assert.ok(!rendered.includes("cfg "), "no labels are needed when nothing differs");
    assert.match(rendered, /alpha\/model-a/);
  });

  it("shows the effective model when the session cannot use what it configured", () => {
    // Configured A, effective B, no attempt: both are runtime facts and the
    // footer must not silently present one as the other.
    const rendered = footer(
      stateOf({
        model: {
          ...sessionModel("beta/model-b"),
          configured: { model: "alpha/model-a" },
        },
      }),
      "connected",
    );
    assert.match(rendered, /cfg alpha\/model-a/);
    assert.match(rendered, /eff beta\/model-b/);
  });

  it("keeps configured, effective, and attempt when all three differ", () => {
    const rendered = footer(
      stateOf({
        model: {
          ...sessionModel("beta/model-b"),
          configured: { model: "alpha/model-a" },
        },
        attempt: attemptView({ model: attemptModel("gamma/model-c") }),
      }),
      "connected",
    );
    assert.match(rendered, /cfg alpha\/model-a/);
    assert.match(rendered, /eff beta\/model-b/, "the effective model is never dropped");
    assert.match(rendered, /attempt gamma\/model-c/);
  });

  it("never drops a model identity to make a narrow terminal fit", () => {
    // The layout degrades by dropping optional segments and by wrapping. It
    // may not drop a model identity, and it may not truncate one into a
    // shorter identity that names a different model.
    const rendered = footer(
      stateOf({
        model: {
          ...sessionModel("beta/model-b"),
          configured: { model: "alpha/model-a" },
        },
        attempt: attemptView({ model: attemptModel("gamma/model-c") }),
        capabilities: { revision: 3 },
      }),
      "connected",
      24,
    );
    assert.match(rendered, /cfg alpha\/model-a/);
    assert.match(rendered, /eff beta\/model-b/);
    assert.match(rendered, /attempt gamma\/model-c/);
    assert.ok(
      !rendered.includes("cfg alpha/model-…") &&
        !rendered.includes("eff beta/model-…") &&
        !rendered.includes("attempt gamma/model-…"),
      "an identity is never elided into a prefix",
    );
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
    assert.match(rendered, /cancelled/);
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
      /↑12\.5k ↓840/,
    );
    assert.match(
      footer(stateOf({ attempt: attemptView() }), "connected"),
      /tokens pending/,
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
    assert.match(rendered, /queued 1/);
    assert.match(rendered, /background 1/);
    assert.match(rendered, /human input 1/);
  });

  it("reports drain and a closed transport without implying cancellation", () => {
    const rendered = footer(stateOf({ shutting_down: true }), "closed: input_eof");
    assert.match(rendered, /draining/);
    assert.match(rendered, /offline/);
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
    assert.match(wide, /context 10%\/128k/);

    const narrow = footer(state, "connected", 40);
    const rows = narrow.split("\n");
    assert.ok(rows.length <= 2, "never more than two rows");
    for (const row of rows) {
      assert.ok(row.length <= 40, `row within width: ${JSON.stringify(row)}`);
    }
    // The essential identities survive; optional status and hint content may
    // be dropped or wrapped for a narrow terminal.
    assert.match(narrow, /alpha\/model-a/);
    assert.match(narrow, /attempt beta\/model-b/);
    assert.ok(!narrow.includes("Ctrl+L model"));
  });
});

describe("startup and context", () => {
  it("is shown before a real turn and reclaimed after transcript content", () => {
    const initial = stateOf();
    assert.equal(startupVisible(initial), true);
    assert.equal(
      startupVisible(stateOf({ messages: [userMessage("m1", "hello")] })),
      false,
    );
  });

  it("shows authoritative welcome facts without client internals", () => {
    const state = stateOf({
      model: sessionModel("alpha/model-a", {
        protocol: "openai_responses",
        contextWindow: 256_000,
        capabilities: {
          inputModalities: ["text"],
          outputModalities: ["text"],
          toolCalls: true,
          reasoning: true,
        },
        reasoningEnabled: true,
        reasoningProfile: "medium",
      }),
    });
    const rendered = plainText(
      renderStartup(state, {
        id: "session-7",
        name: "review branch",
        created_at: "2026-08-21T00:00:00Z",
        updated_at: "2026-08-21T00:00:00Z",
        active_node: "node-3",
        active_conversation_id: "conv-3",
        node_count: 1,
      }),
    );

    assert.match(rendered, /rustX/);
    assert.match(rendered, /model alpha\/model-a/);
    assert.match(rendered, /provider alpha · Responses/);
    assert.match(rendered, /context —\/256k/);
    assert.match(rendered, /reasoning on \(profile medium\)/);
    assert.match(rendered, /session review branch · node node-3/);
    assert.match(rendered, /Ctrl\+L model/);
    assert.match(rendered, /\/help commands/);
    assert.doesNotMatch(rendered, /attachment|conversation|cursor|cap r/i);
  });

  it("uses only the latest runtime-published request usage", () => {
    const state = stateOf({
      attempt: attemptView({
        model: attemptModel("alpha/model-a", { contextWindow: 256_000 }),
        last_usage: { input_tokens: 25_600, output_tokens: 512, total_tokens: 26_112 },
      }),
    });
    assert.equal(contextLabel(state), "context 10%/256k");
  });
});

describe("activity area", () => {
  it("renders nothing when the runtime knows of no background work", () => {
    assert.equal(renderBackgroundSection(stateOf(), prefs()), "");
    assert.equal(renderInteractionSection(stateOf(), prefs()), "");
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
        prefs(),
      ),
    );
    assert.match(rendered, /Approval required · 1 pending/);
    assert.match(rendered, /bash/);
    assert.match(rendered, /attempt-1-interaction-1/);
    assert.match(rendered, /native policy requires approval/);
    assert.match(rendered, /printf original/);
    assert.match(rendered, /\/approve <interaction-id> <allow\|deny> \[reason\]/);
  });

  it("marks the deterministic focused interaction for ordinary editor input", () => {
    const later = questionInteraction("attempt-1-interaction-z");
    const focused = questionInteraction("attempt-1-interaction-a");
    const rendered = plainText(
      renderInteractionSection(
        stateOf({ pending_interactions: [later, focused] }),
        prefs(),
      ),
    );
    assert.match(rendered, /Focused interaction: attempt-1-interaction-a/);
    assert.match(rendered, /ordinary input answers this request/);
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

describe("client collapse is finite and reversible", () => {
  /**
   * The final disclosure contract, at the two places it was not yet held.
   *
   * ```text
   * client collapse    finite, and reversible from facts already held
   * runtime truncation authoritative, and irreversible
   * ```
   *
   * A background settlement and a pending approval both carry runtime prose
   * that can be arbitrarily long, and both are decision-relevant: one explains
   * why work failed, the other is what a reader says allow or deny to. Bounding
   * them is right; bounding them *irreversibly* is not, because the hidden
   * remainder is already in `PresentationState` and can be shown for free.
   *
   * Every case below asserts both halves. A test that only proved the last
   * token was absent when collapsed would pass on a card that could never be
   * opened again.
   */
  const HUGE = "x".repeat(50_000);
  /** Comfortably above one collapsed band, three orders below the input. */
  const CEILING = 4_000;

  function background(
    result: Parameters<typeof toolResult>[0],
    expanded: boolean,
  ): string {
    const preferences = expanded
      ? withExpandedBackgroundExecutions(prefs(), ["exec-1"])
      : prefs();
    return plainText(
      renderBackground(
        backgroundExecution("exec-1", "failed", { result: toolResult(result) }),
        preferences,
      ),
    );
  }

  function interaction(
    kind: Partial<Extract<InteractionRequest["kind"], { type: "approval" }>>,
    expanded: boolean,
  ): string {
    const request = approvalInteraction();
    if (request.kind.type !== "approval") {
      throw new Error("fixture must be an approval interaction");
    }
    const preferences = expanded
      ? withExpandedInteractions(prefs(), [request.id])
      : prefs();
    return plainText(
      renderInteractionSection(
        stateOf({
          pending_interactions: [
            { ...request, kind: { ...request.kind, ...kind, type: "approval" } },
          ],
        }),
        preferences,
      ),
    );
  }

  it("bounds a background failure reason and restores it on expansion", () => {
    const collapsed = background({ status: { type: "failed", error: HUGE } }, false);
    assert.ok(
      collapsed.length < CEILING,
      `collapsed background card was ${collapsed.length} characters`,
    );
    assert.match(collapsed, /failed/, "the runtime settlement stays visible");
    assert.match(collapsed, /exec-1/, "the identity stays visible");
    assert.ok(!collapsed.includes(HUGE));

    // The whole explanation, from the same already-held result.
    assert.ok(background({ status: { type: "failed", error: HUGE } }, true).includes(HUGE));
  });

  it("bounds a background denial reason and restores it on expansion", () => {
    const status = { type: "denied" as const, reason: HUGE };
    const collapsed = background({ status }, false);
    assert.ok(collapsed.length < CEILING);
    assert.match(collapsed, /denied/);
    assert.ok(
      !(collapsed.split("\n")[0] ?? "").includes("xx"),
      "prose never reaches the header",
    );
    assert.ok(!collapsed.includes(HUGE));

    assert.ok(background({ status }, true).includes(HUGE));
  });

  it("expands a background reason and body under one expansion state", () => {
    // The bug this replaces: the body honoured the execution's expansion
    // preference while the reason was rendered through a permanently
    // collapsed context, so `/expand background <id>` revealed half the card.
    const result = {
      status: { type: "failed" as const, error: HUGE },
      content: [{ type: "text" as const, text: `${HUGE}TAIL` }],
    };
    const collapsed = background(result, false);
    assert.ok(collapsed.length < CEILING);
    assert.ok(!collapsed.includes("TAIL"));

    const expanded = background(result, true);
    assert.ok(expanded.includes(HUGE), "the reason is complete");
    assert.ok(expanded.includes("TAIL"), "the body is complete");
  });

  it("keeps the runtime's own truncation irreversible", () => {
    // Expanding restores what the client hid. It cannot restore bytes the
    // runtime never sent, and it must not pretend otherwise.
    const result = {
      status: { type: "failed" as const, error: "boom" },
      truncation: { truncated: true, original_bytes: 9_001 },
    };
    for (const rendered of [background(result, false), background(result, true)]) {
      assert.match(rendered, /runtime-truncated result \(from 9001 bytes\)/);
    }
  });

  it("bounds a pending approval reason and restores it on expansion", () => {
    const collapsed = interaction({ reason: HUGE }, false);
    assert.ok(
      collapsed.length < CEILING,
      `collapsed approval card was ${collapsed.length} characters`,
    );
    assert.match(collapsed, /bash/, "the tool identity stays visible");
    assert.match(collapsed, /attempt-1-interaction-1/, "the id stays visible");
    assert.match(collapsed, /^\s*x+/m, "the beginning of the reason is on screen");
    assert.ok(!collapsed.includes(HUGE));

    assert.ok(interaction({ reason: HUGE }, true).includes(HUGE));
  });

  it("bounds pending approval arguments and restores them on expansion", () => {
    // A `Write` approval carrying 50 kB of content is exactly the case a
    // reader must be able to inspect *before* answering allow or deny.
    const args = { path: "large.txt", content: HUGE };
    const collapsed = interaction({ arguments: args }, false);
    assert.ok(collapsed.length < CEILING);
    assert.match(collapsed, /large\.txt/, "the identifying argument survives");
    assert.ok(!collapsed.includes(HUGE));

    const expanded = interaction({ arguments: args }, true);
    assert.ok(expanded.includes(HUGE), "the validated arguments are complete");
    // Exactly what the runtime published, never a client-normalized rewrite.
    assert.ok(expanded.includes(JSON.stringify(args, null, 2).split("\n")[1] ?? ""));
  });

  it("bounds a huge reason and huge arguments independently", () => {
    const fields = { reason: `${HUGE}REASONTAIL`, arguments: { content: `${HUGE}ARGTAIL` } };
    const collapsed = interaction(fields, false);
    assert.ok(
      collapsed.length < CEILING,
      `collapsed approval card was ${collapsed.length} characters`,
    );
    assert.ok(!collapsed.includes("REASONTAIL"));
    assert.ok(!collapsed.includes("ARGTAIL"));
    // Neither band starved the other: both reported their own elision.
    assert.equal(
      collapsed.split("\n").filter((line) => line.includes("more characters")).length,
      2,
    );
    assert.ok(
      collapsed.split("\n").every((line) => line.length < DEFAULT_PREVIEW_CHARS + 200),
    );

    const expanded = interaction(fields, true);
    assert.ok(expanded.includes("REASONTAIL"));
    assert.ok(expanded.includes("ARGTAIL"));
  });

  it("tells the reader how to see the rest", () => {
    const collapsed = interaction({ reason: HUGE }, false);
    assert.match(collapsed, /\/expand interaction <interaction-id>/);
    assert.match(collapsed, /\/approve <interaction-id> <allow\|deny> \[reason\]/);
  });
});
