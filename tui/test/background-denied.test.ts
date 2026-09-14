import assert from "node:assert/strict";
import { it } from "node:test";
import { BACKGROUND_TERMINAL_STATES } from "../src/protocol/app-server.ts";

it("protocol 19 preserves denied as an absorbing background terminal", () => {
  assert.ok(BACKGROUND_TERMINAL_STATES.has("denied"));
  assert.ok(!BACKGROUND_TERMINAL_STATES.has("cancelling"));
});
