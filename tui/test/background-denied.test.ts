import assert from "node:assert/strict";
import { it } from "node:test";
import { BACKGROUND_TERMINAL_STATES, RUNTIME_CLIENT_PROTOCOL_VERSION } from "../src/protocol/types.ts";

it("protocol 17 preserves denied as an absorbing background terminal", () => {
  assert.equal(RUNTIME_CLIENT_PROTOCOL_VERSION, 17);
  assert.ok(BACKGROUND_TERMINAL_STATES.has("denied"));
  assert.ok(!BACKGROUND_TERMINAL_STATES.has("cancelling"));
});
