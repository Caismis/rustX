import { strict as assert } from "node:assert";
import { test } from "node:test";
import { configurationCommand, forwardConfigurationCommand } from "../src/configuration-command.ts";

test("configuration commands forward opaque Rust arguments without interpreting configuration", () => {
  for (const args of [["init", "--template", "custom"], ["config", "show", "--sources", "--json"], ["doctor", "--probe", "--prepare"]]) {
    assert.deepEqual(configurationCommand(["--binary", "/rustx", ...args]), { binary: "/rustx", arguments: args });
  }
  assert.equal(configurationCommand(["--binary", "/rustx", "--workspace", "/project"]), undefined);
});

test("forwarded commands preserve Rust exit status and release signal listeners", async () => {
  const before = [process.listenerCount("SIGINT"), process.listenerCount("SIGTERM")];
  assert.equal(await forwardConfigurationCommand({ binary: process.execPath, arguments: ["-e", "process.exit(3)"] }), 3);
  assert.deepEqual([process.listenerCount("SIGINT"), process.listenerCount("SIGTERM")], before);
});
