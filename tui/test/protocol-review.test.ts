import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { it } from "node:test";
import type { InteractionRequest } from "../src/protocol/types.ts";

it("Review v21 shares the complete immutable Rust wire fixture", () => {
  const actual: unknown = JSON.parse(readFileSync(new URL("../../tests/fixtures/runtime-client/review-v21.json", import.meta.url), "utf8"));
  const expected: InteractionRequest = {
    id: "review-interaction", conversation_id: "review-conversation", attempt_id: "review-attempt", turn: 1,
    kind: { type: "review", subject_digest: "880840e722fdde30d73da69d79c8d8ff58f897e271394f03083b4ff1ca4692ba", review: {
      instance: { block: { run: { conversation_id: "review-conversation", attempt_id: "review-attempt", invocation: 1 }, definition: { workflow_id: "review_plan", blocks: [] }, invocations: [0] }, node: "human", visit: 0 },
      subject: { type: "plan", candidate: null, content: { steps: ["inspect", "implement"] } }, context: [{ value: { passed: true }, candidate: { run: { conversation_id: "review-conversation", attempt_id: "review-attempt", invocation: 1 }, version: 1, content: "b".repeat(64) } }],
    } },
  };
  assert.deepEqual(actual, expected);
});
