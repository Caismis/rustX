/**
 * Issue #144: the TUI's protocol mirror carries the named-agent subagent
 * identity, and the obsolete profile-shaped contract is gone. Issue #178
 * adds the live activity projection (`observation`), the redacted
 * `execution_profile`, and `started_at` to the same mirrored shape.
 *
 * The mirror is a compile-time contract, so these cases are deliberately a
 * mix: `tsc --noEmit` proves the shape (a `profile` field would not compile
 * against `RuntimeClientAgent`), and the runtime assertions prove the
 * reducer actually carries both identity fields through.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { reduce, replaceFromSnapshot } from "../src/presentation/projection.ts";
import {
  runtimeCursor,
  snapshot,
  subagent,
  subagentObservation,
} from "./support/fixtures.ts";

describe("subagent identity", () => {
  it("carries both identity digests from the snapshot", () => {
    const state = replaceFromSnapshot(
      {
        ...snapshot(),
        agents: [subagent("explore", "sha256:d1")],
      },
      runtimeCursor(1),
    );
    assert.equal(state.agents.length, 1);
    const [child] = state.agents;
    assert.ok(child);
    assert.equal(child.agent, "explore");
    // Two separate identities (Issue #258): the source definition the child
    // started with, and the effective execution profile it committed with.
    assert.equal(child.definition_digest, "sha256:d1");
    assert.equal(child.profile_digest, "sha256:d1-profile");
    assert.equal(
      (child as unknown as Record<string, unknown>).profile,
      undefined,
      "the obsolete profile identity is absent from the mirrored shape",
    );
  });

  it("carries the observation and started_at through the snapshot", () => {
    const observation = subagentObservation(
      { type: "tool", tool_call_id: "call-1", tool_id: "tool-grep" },
      {
        revision: "7",
        last_activity_at: "2026-09-02T10:02:00Z",
        counters: { model_requests: 2, model_retries: 1, tool_executions: 3 },
      },
    );
    const state = replaceFromSnapshot(
      {
        ...snapshot(),
        agents: [
          subagent("explore", "sha256:d1", "active", {
            observation,
            execution_profile: {
              model: "alpha/model-a",
              reasoning_profile: "reasoning:high",
              reasoning_enabled: true,
            },
          }),
        ],
      },
      runtimeCursor(1),
    );
    const [child] = state.agents;
    assert.ok(child);
    assert.deepEqual(child.observation, observation);
    assert.equal(child.started_at, "2026-09-02T10:00:00Z");
    assert.deepEqual(child.execution_profile, {
      model: "alpha/model-a",
      reasoning_profile: "reasoning:high",
      reasoning_enabled: true,
    });
  });

  it("carries the whole observation through a agent_updated upsert", () => {
    let state = replaceFromSnapshot(
      { ...snapshot(), agents: [subagent("explore", "sha256:d1")] },
      runtimeCursor(1),
    );
    const observation = subagentObservation(
      { type: "retrying_model", retry: 2 },
      { revision: "3", last_activity_at: "2026-09-02T10:01:00Z" },
    );
    state = reduce(state, {
      cursor: runtimeCursor(2),
      event: {
        type: "agent_updated",
        agent: subagent("explore", "sha256:d1", "active", { observation }),
      },
    });
    assert.equal(state.agents.length, 1);
    assert.deepEqual(state.agents[0]?.observation, observation);
  });

  it("keeps a running child bound to the digest it started with", () => {
    // A later generation may redefine the same agent name. A live update
    // about *this* child still carries its own digest, so a client can never
    // conclude the running child now has the new definition.
    let state = replaceFromSnapshot(
      { ...snapshot(), agents: [subagent("explore", "sha256:d1")] },
      runtimeCursor(1),
    );
    state = reduce(state, {
      cursor: runtimeCursor(2),
      event: {
        type: "agent_updated",
        agent: subagent("explore", "sha256:d1", "inactive"),
      },
    });
    assert.equal(state.agents.length, 1);
    const [child] = state.agents;
    assert.ok(child);
    assert.equal(child.state, "inactive");
    assert.equal(child.definition_digest, "sha256:d1");
  });

  it("carries an interrupted child through the ordinary Runtime Client projection", () => {
    let state = replaceFromSnapshot(
      {
        ...snapshot(),
        agents: [subagent("worker", "sha256:d1", "inactive", { activation_state: "interrupted" })],
      },
      runtimeCursor(1),
    );
    assert.equal(state.agents[0]?.state, "inactive");
    assert.equal(state.agents[0]?.activation_state, "interrupted");

    state = reduce(state, {
      cursor: runtimeCursor(2),
      event: {
        type: "agent_updated",
        agent: subagent("worker", "sha256:d1", "inactive", {
          detail: "child outcome unknown",
          activation_state: "interrupted",
        }),
      },
    });
    assert.equal(state.agents[0]?.state, "inactive");
    assert.equal(state.agents[0]?.activation_state, "interrupted");
    assert.equal(state.agents[0]?.detail, "child outcome unknown");
  });
});
