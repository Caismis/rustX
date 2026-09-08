import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { it } from "node:test";
import type { InteractionRequest } from "../src/protocol/types.ts";

it("Review v21 shares the complete immutable Rust wire fixture", () => {
  const actual: unknown = JSON.parse(readFileSync(new URL("../../tests/fixtures/runtime-client/review-v21.json", import.meta.url), "utf8"));
  const expected: InteractionRequest = {
    id: "review-interaction", conversation_id: "review-conversation", attempt_id: "review-attempt", turn: 1,
    kind: { type: "review", subject_digest: "5700e24e6e7571534adc95185bda7a852e76d1a443a67b0df61560d477facf70", review: {
      instance: { block: { run: { conversation_id: "review-conversation", attempt_id: "review-attempt", invocation: 1 }, definition: { workflow_id: "review_plan", blocks: [] }, invocations: [0] }, node: "human", visit: 0 },
      subject: { type: "plan", content: { steps: ["inspect", "implement"] } }, context: [],
    } },
  };
  assert.deepEqual(actual, expected);
});
