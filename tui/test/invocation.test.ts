import assert from "node:assert/strict";
import { test } from "node:test";
import { invocationLabel } from "../src/presentation/invocation.ts";

test("Agent approval retains its real canonical call label", () => {
  assert.equal(invocationLabel({ caller: "agent", call_id: "call-7" }), "call call-7");
});

test("Workflow approval labels its concrete node without inventing a call id", () => {
  assert.equal(invocationLabel({
    caller: "workflow",
    node: {
      block: {
        run: { conversation_id: "conv", attempt_id: "attempt", invocation: 4 },
        definition: { workflow_id: "check", blocks: ["fanout", "alpha"] },
        invocations: [],
      },
      node: "verify",
      visit: 0,
    },
  }), "workflow check · fanout/alpha/verify · 4:0");
});
