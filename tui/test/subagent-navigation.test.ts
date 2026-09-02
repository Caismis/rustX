/**
 * Presentation-only subagent row navigation.
 *
 * These tests deliberately exercise identities from Runtime Client snapshots,
 * not a client-owned transcript/history collection.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  cycleSubagentSelection,
  hasSubagentSelection,
} from "../src/ui/subagent-navigation.ts";
import { subagent } from "./support/fixtures.ts";

const rows = [
  subagent("explore", "sha256:one", "running", {
    subagent_id: "subagent-one",
    child_conversation_id: "conversation-child-one",
  }),
  subagent("reviewer", "sha256:two", "succeeded", {
    subagent_id: "subagent-two",
    child_conversation_id: "conversation-child-two",
  }),
];

describe("subagent inspection navigation state", () => {
  it("cycles only through authoritative Runtime Client row identities", () => {
    assert.equal(cycleSubagentSelection(rows, undefined, 1), "subagent-one");
    assert.equal(cycleSubagentSelection(rows, "subagent-one", 1), "subagent-two");
    assert.equal(cycleSubagentSelection(rows, "subagent-two", 1), "subagent-one");
    assert.equal(cycleSubagentSelection(rows, undefined, -1), "subagent-two");
  });

  it("repairs a stale presentation selection when snapshot state replaces rows", () => {
    assert.equal(hasSubagentSelection(rows, "subagent-one"), true);
    assert.equal(hasSubagentSelection(rows.slice(1), "subagent-one"), false);
    assert.equal(cycleSubagentSelection(rows.slice(1), "subagent-one", 1), "subagent-two");
    assert.equal(cycleSubagentSelection([], "subagent-one", 1), undefined);
  });
});
