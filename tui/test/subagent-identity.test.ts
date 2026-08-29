/**
 * Issue #144: the TUI's protocol mirror carries the named-agent subagent
 * identity, and the obsolete profile-shaped contract is gone.
 *
 * The mirror is a compile-time contract, so these cases are deliberately a
 * mix: `tsc --noEmit` proves the shape (a `profile` field would not compile
 * against `RuntimeClientSubagent`), and the runtime assertions prove the
 * reducer actually carries both identity fields through.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { reduce, replaceFromSnapshot } from "../src/presentation/projection.ts";
import {
  RUNTIME_CLIENT_PROTOCOL_VERSION,
  type RuntimeClientSubagent,
} from "../src/protocol/types.ts";
import { runtimeCursor, snapshot } from "./support/fixtures.ts";

function subagent(
  agent: string,
  definitionDigest: string,
  state: RuntimeClientSubagent["state"] = "running",
): RuntimeClientSubagent {
  return {
    subagent_id: "conv-1-subagent-1",
    child_agent_id: "agent-child",
    child_conversation_id: "conv-1-subagent-1",
    agent,
    definition_digest: definitionDigest,
    state,
  };
}

describe("subagent identity", () => {
  it("negotiates v9, which carries named identity and interrupted state", () => {
    assert.equal(RUNTIME_CLIENT_PROTOCOL_VERSION, 9);
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
        subagent: { ...subagent("explore", "sha256:d1"), state: "succeeded" },
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
        subagent: {
          ...subagent("worker", "sha256:d1", "interrupted"),
          detail: "child outcome unknown",
        },
      },
    });
    assert.equal(state.subagents[0]?.state, "interrupted");
    assert.equal(state.subagents[0]?.detail, "child outcome unknown");
  });
});
