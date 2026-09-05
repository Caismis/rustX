/**
 * Issue #144: the TUI's protocol mirror carries the named-agent subagent
 * identity, and the obsolete profile-shaped contract is gone. Issue #178
 * adds the live activity projection (`observation`), the redacted
 * `execution_profile`, and `started_at` to the same mirrored shape.
 *
 * The mirror is a compile-time contract, so these cases are deliberately a
 * mix: `tsc --noEmit` proves the shape (a `profile` field would not compile
 * against `RuntimeClientSubagent`), and the runtime assertions prove the
 * reducer actually carries both identity fields through.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { reduce, replaceFromSnapshot } from "../src/presentation/projection.ts";
import { RUNTIME_CLIENT_PROTOCOL_VERSION } from "../src/protocol/types.ts";
import {
  runtimeCursor,
  snapshot,
  subagent,
  subagentObservation,
} from "./support/fixtures.ts";

describe("subagent identity", () => {
  it("negotiates v15, which carries workspace resource settlement and the Agent Status projection", () => {
    assert.equal(RUNTIME_CLIENT_PROTOCOL_VERSION, 15);
  });

  it("carries agent and definition_digest from the snapshot", () => {
    const state = replaceFromSnapshot(
      {
        ...snapshot(),
        subagents: [subagent("explore", "sha256:d1")],
      },
      runtimeCursor(1),
    );
    assert.equal(state.subagents.length, 1);
    const [child] = state.subagents;
    assert.ok(child);
    assert.equal(child.agent, "explore");
    assert.equal(child.definition_digest, "sha256:d1");
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
        revision: 7,
        last_activity_at: "2026-09-02T10:02:00Z",
        counters: { model_requests: 2, model_retries: 1, tool_executions: 3 },
      },
    );
    const state = replaceFromSnapshot(
      {
        ...snapshot(),
        subagents: [
          subagent("explore", "sha256:d1", "running", {
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
    const [child] = state.subagents;
    assert.ok(child);
    assert.deepEqual(child.observation, observation);
    assert.equal(child.started_at, "2026-09-02T10:00:00Z");
    assert.deepEqual(child.execution_profile, {
      model: "alpha/model-a",
      reasoning_profile: "reasoning:high",
      reasoning_enabled: true,
    });
  });

  it("carries the whole observation through a subagent_updated upsert", () => {
    let state = replaceFromSnapshot(
      { ...snapshot(), subagents: [subagent("explore", "sha256:d1")] },
      runtimeCursor(1),
    );
    const observation = subagentObservation(
      { type: "retrying_model", retry: 2 },
      { revision: 3, last_activity_at: "2026-09-02T10:01:00Z" },
    );
    state = reduce(state, {
      cursor: runtimeCursor(2),
      event: {
        type: "subagent_updated",
        subagent: subagent("explore", "sha256:d1", "running", { observation }),
      },
    });
    assert.equal(state.subagents.length, 1);
    assert.deepEqual(state.subagents[0]?.observation, observation);
  });

  it("keeps a running child bound to the digest it started with", () => {
    // A later generation may redefine the same agent name. A live update
    // about *this* child still carries its own digest, so a client can never
    // conclude the running child now has the new definition.
    let state = replaceFromSnapshot(
      { ...snapshot(), subagents: [subagent("explore", "sha256:d1")] },
      runtimeCursor(1),
    );
    state = reduce(state, {
      cursor: runtimeCursor(2),
      event: {
        type: "subagent_updated",
        subagent: subagent("explore", "sha256:d1", "succeeded"),
      },
    });
    assert.equal(state.subagents.length, 1);
    const [child] = state.subagents;
    assert.ok(child);
    assert.equal(child.state, "succeeded");
    assert.equal(child.definition_digest, "sha256:d1");
  });

  it("carries an interrupted child through the ordinary Runtime Client projection", () => {
    let state = replaceFromSnapshot(
      {
        ...snapshot(),
        subagents: [subagent("worker", "sha256:d1", "interrupted")],
      },
      runtimeCursor(1),
    );
    assert.equal(state.subagents[0]?.state, "interrupted");

    state = reduce(state, {
      cursor: runtimeCursor(2),
      event: {
        type: "subagent_updated",
        subagent: subagent("worker", "sha256:d1", "interrupted", {
          detail: "child outcome unknown",
        }),
      },
    });
    assert.equal(state.subagents[0]?.state, "interrupted");
    assert.equal(state.subagents[0]?.detail, "child outcome unknown");
  });
});
