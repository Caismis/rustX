import assert from "node:assert/strict";
import { it } from "node:test";

import { fixtures } from "../../protocol/app-server/fixtures.ts";
import { AppServerClient, UncertainOutcomeError } from "../src/app-server/client.ts";
import { StdioTransport } from "../src/app-server/stdio-transport.ts";
import { TransportClosedError } from "../src/app-server/transport.ts";
import { JsonlFramingError } from "../src/protocol/jsonl.ts";
import { ScriptedAppServer } from "./support/app-server-peer.ts";

it("partial stdout EOF before process exit retains an uncertain mutation", async () => {
  const peer = new ScriptedAppServer();
  const transport = new StdioTransport({ input: peer.serverOutput, output: peer.clientOutput });
  const initializing = AppServerClient.initialize({ transport });
  const [initialize] = await peer.awaitMethod("initialize");
  assert.ok(initialize);
  const initialized = fixtures.find((record) => "result" in record && record.result?.type === "initialized");
  assert.ok(initialized && "result" in initialized && initialized.result?.type === "initialized");
  peer.respond(initialize.id, initialized.result);
  const client = await initializing;

  const pending = client.call("session/attach", {
    session_id: "ses_01900000-0000-7000-8000-000000001001", node_id: null,
  }, "attached");
  const settled = pending.then(() => assert.fail("a partial response cannot settle the mutation"), (error: unknown) => error);
  const [attach] = await peer.awaitMethod("session/attach");
  assert.ok(attach);
  const partial = `{"jsonrpc":"2.0","id":${JSON.stringify(attach.id)},"result":`;
  peer.writeRaw(partial);
  peer.endOutput();

  const failure = await settled;
  assert.ok(failure instanceof UncertainOutcomeError);
  assert.ok(failure.transportFailure instanceof TransportClosedError);
  assert.equal(failure.transportFailure.reason, "framing_error");
  assert.ok(failure.transportFailure.cause instanceof JsonlFramingError);
  assert.equal(failure.transportFailure.cause.kind, "truncated_record");
  assert.equal(failure.transportFailure.cause.bytes, Buffer.byteLength(partial));
  const first = transport.closed;
  transport.reportProcessExit(null, "SIGKILL");
  assert.equal(transport.closed, first);
  assert.equal(client.closed, first);
  assert.equal(peer.log.count("session/attach"), 1, "a lost mutation is never replayed");
});
