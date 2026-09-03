/**
 * Issue #178: the subagent section renders one compact live-activity row
 * per non-terminal child, driven entirely by the runtime-published
 * observation projection.
 *
 * The clock is injected, so elapsed and last-activity labels are proven
 * against fixed instants; the terminal filtering proves the section never
 * impersonates the lifecycle authority.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { RuntimeClientSubagent } from "../src/protocol/types.ts";
import { renderSubagentSection } from "../src/ui/components/activity.ts";
import {
  prefs,
  stateOf,
  plain,
} from "./support/render.ts";
import { subagent, subagentObservation } from "./support/fixtures.ts";

const NOW = new Date("2026-09-02T10:03:00Z");

function render(
  children: RuntimeClientSubagent[],
  selectedSubagentId?: string,
): string {
  return plain(
    renderSubagentSection(
      stateOf({ subagents: children }),
      prefs(),
      NOW,
      selectedSubagentId,
    ),
  );
}

function child(
  activity: RuntimeClientSubagent["observation"]["activity"],
  overrides: Partial<RuntimeClientSubagent> = {},
): RuntimeClientSubagent {
  return subagent("explore", "sha256:d1", "running", {
    observation: subagentObservation(activity, {
      revision: 1,
      last_activity_at: "2026-09-02T10:02:55Z",
    }),
    ...overrides,
  });
}

describe("subagent activity section", () => {
  it("renders nothing when the runtime knows of no subagents", () => {
    assert.equal(render([]), "");
  });

  it("keeps terminal children as durable conversation navigation targets", () => {
    const rendered = render([
      subagent("explore", "sha256:d1", "succeeded"),
    ]);
    assert.match(rendered, /Subagents · 0 active of 1 known/);
    assert.match(rendered, /explore · succeeded · conv-1-subagent-1/);
    assert.doesNotMatch(rendered, /awaiting activity/);
  });

  it("shows the header count and the lifecycle of active children only", () => {
    const rendered = render([
      child({ type: "awaiting_activity" }),
      subagent("worker", "sha256:d2", "succeeded", {
        subagent_id: "conv-1-subagent-2",
        child_conversation_id: "conv-1-subagent-2",
      }),
    ]);
    assert.match(rendered, /Subagents · 1 active of 2 known/);
    assert.match(rendered, /explore · running/);
    assert.match(rendered, /worker/);
    assert.match(rendered, /conv-1-subagent-2/);
  });

  it("marks only the selected row without changing the observation payload", () => {
    const rendered = render([
      child({ type: "awaiting_activity" }),
      subagent("worker", "sha256:d2", "failed", {
        subagent_id: "conv-1-subagent-2",
        child_conversation_id: "conv-1-subagent-2",
      }),
    ], "conv-1-subagent-2");
    assert.match(rendered, /▸ .*worker · failed · conv-1-subagent-2/);
    assert.doesNotMatch(rendered, /detail/);
  });

  it("derives elapsed time from started_at at render time", () => {
    const rendered = render([child({ type: "awaiting_activity" })]);
    // started_at 10:00:00Z, rendered at 10:03:00Z.
    assert.match(rendered, /running · 3m/);
  });

  it("shows the time since the last projected activity", () => {
    const rendered = render([child({ type: "awaiting_activity" })]);
    // last_activity_at 10:02:55Z, rendered at 10:03:00Z.
    assert.match(rendered, /last activity 5s ago/);
  });

  it("omits the last-activity line before the child reports any", () => {
    const rendered = render([
      subagent("explore", "sha256:d1", "running", {
        observation: subagentObservation({ type: "awaiting_activity" }),
      }),
    ]);
    assert.match(rendered, /awaiting activity/);
    assert.doesNotMatch(rendered, /last activity/);
  });

  it("renders the neutral state as the idle label", () => {
    assert.match(render([child({ type: "awaiting_activity" })]), /awaiting activity/);
  });

  it("renders an in-flight model request", () => {
    assert.match(
      render([child({ type: "model", request_id: "req-1", retry: 0 })]),
      /model request/,
    );
  });

  it("marks a retried model request", () => {
    assert.match(
      render([child({ type: "model", request_id: "req-2", retry: 1 })]),
      /model request · retry 1/,
    );
  });

  it("renders a scheduled retry", () => {
    assert.match(
      render([child({ type: "retrying_model", retry: 2 })]),
      /retrying · attempt 2/,
    );
  });

  it("renders an in-flight tool execution without progress", () => {
    assert.match(
      render([
        child({ type: "tool", tool_call_id: "call-1", tool_id: "tool-grep" }),
      ]),
      /tool-grep/,
    );
  });

  it("renders the latest bounded tool progress", () => {
    const rendered = render([
      child({
        type: "tool",
        tool_call_id: "call-1",
        tool_id: "tool-grep",
        progress: { message: "scanning src", completed: 1, total: 2 },
      }),
    ]);
    assert.match(rendered, /tool-grep · scanning src · 1\/2/);
  });

  it("bounds externally derived activity text to one finite line", () => {
    const huge = `${"x".repeat(50_000)}\nsecond line`;
    const rendered = render([
      child({
        type: "tool",
        tool_call_id: "call-1",
        tool_id: "tool-grep",
        progress: { message: huge },
      }),
      subagent(`agent-${"y".repeat(50_000)}`, "sha256:d1", "running", {
        subagent_id: "conv-1-subagent-2",
        observation: subagentObservation({ type: "awaiting_activity" }),
      }),
    ]);
    assert.ok(!rendered.includes("x".repeat(200)), "progress text is clipped");
    assert.ok(!rendered.includes("y".repeat(200)), "agent name is clipped");
    assert.ok(!rendered.includes("second line"), "no embedded newline survives");
  });

  it("renders an in-flight compaction", () => {
    assert.match(render([child({ type: "compacting" })]), /compacting context/);
  });

  it("renders a pending approval wait", () => {
    assert.match(
      render([
        child({ type: "waiting", on: { type: "approval", tool_id: "tool-bash" } }),
      ]),
      /waiting · approval \(tool-bash\)/,
    );
  });

  it("renders a pending questionnaire wait", () => {
    assert.match(
      render([child({ type: "waiting", on: { type: "questionnaire" } })]),
      /waiting · questionnaire/,
    );
  });

  it("shows the frozen execution profile when the runtime published one", () => {
    const rendered = render([
      child({ type: "awaiting_activity" }, {
        execution_profile: {
          model: "alpha/model-a",
          reasoning_profile: "reasoning:high",
          reasoning_enabled: true,
        },
      }),
    ]);
    assert.match(rendered, /alpha\/model-a · reasoning:high/);
  });

  it("shows the model alone when no reasoning profile was selected", () => {
    const rendered = render([
      child({ type: "awaiting_activity" }, {
        execution_profile: {
          model: "alpha/model-a",
          reasoning_enabled: false,
        },
      }),
    ]);
    assert.match(rendered, /alpha\/model-a/);
    assert.doesNotMatch(rendered, /undefined/);
  });

  it("shows no profile line for a recovery-projected record", () => {
    const rendered = render([child({ type: "awaiting_activity" })]);
    assert.doesNotMatch(rendered, /model-a/);
  });

  it("never renders the diagnostics-only detail as a payload", () => {
    const rendered = render([
      child({ type: "awaiting_activity" }, { detail: "x".repeat(50_000) }),
    ]);
    assert.ok(!rendered.includes("x".repeat(200)));
  });
});
