import assert from "node:assert/strict";
import { test } from "node:test";
import { harness, nextRequest } from "./support/app-server-harness.ts";
import { snapshot, assistantMessage, userMessage } from "./support/fixtures.ts";
import { reduce, replaceFromSnapshot } from "../src/presentation/projection.ts";
import type { RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage } from "../src/protocol/app-server.ts";
const entry = (cursor: string, assistant = false): RuntimeClientTranscriptEntry => ({ cursor, item: { type: "message", message: assistant ? assistantMessage(cursor, "answer") : userMessage(cursor, "older") } });
const window = (page: RuntimeClientTranscriptPage) => ({ page, cut: { conversation_id: snapshot().conversation_id, journal: "0", transcript: "0", surface_revision: "0", pending_count: "0", pending_revision: "0" }, newer_cursor: null, target: null, target_cursor: null });
const publish = (h: Awaited<ReturnType<typeof harness>>, cursor: string, transcript: RuntimeClientTranscriptPage) => h.session.applyNotification({ jsonrpc: "2.0", method: "session/event", params: { target: h.target, cursor, event: { type: "read_domains_updated", transcript, occupancy: null, todos: null } } });

test("native read-domain events preserve joined history, totals and presentation ownership without snapshots", async () => {
  const h = await harness(snapshot({ transcript: { entries: [entry("4", true)], next_cursor: "4" } }));
  const loading = h.session.loadOlderTranscript(); const request = await nextRequest(h, "session/transcript", 0);
  h.transport.respond(request.id, { type: "transcript_window", window: window({ entries: [entry("2"), entry("3")], next_cursor: "2" }) }); await loading;
  const statistics = { turns: "1", steps: "1", completed_responses: "50", model_requests: "100", requests_with_usage: "99" };
  let replacements = 0; h.session.onSnapshot(() => replacements++);
  publish(h, "1", { entries: [entry("4", true)], next_cursor: "4", statistics });
  assert.deepEqual(h.session.state.transcript.map(e => e.key), ["committed:2", "committed:3", "committed:4"]);
  assert.equal(h.session.state.transcriptNextCursor, "2"); assert.deepEqual(h.session.state.statistics, statistics);
  assert.equal(replacements, 0); assert.equal(h.transport.transportCount("session/snapshot"), 0); h.client.close();
});
test("gaps coalesce while snapshot is gated, then replay joins the returned cursor", async () => {
  const h = await harness(); publish(h, "3", { entries: [entry("3")] });
  const request = await nextRequest(h, "session/snapshot", 0);
  publish(h, "4", { entries: [entry("4")] }); publish(h, "5", { entries: [entry("5")] });
  const repair = h.session.resync();
  assert.equal(h.transport.transportCount("session/snapshot"), 1);
  h.transport.respond(request.id, { type: "snapshot", snapshot: snapshot(), cursor: "4" });
  const subscribe = await nextRequest(h, "session/subscribe", 0);
  h.transport.respond(subscribe.id, { type: "subscribed", after_cursor: "4" }); await repair;
  publish(h, "5", { entries: [entry("5")] }); publish(h, "5", { entries: [entry("99")] });
  assert.deepEqual(h.session.state.transcript.map(e => e.key), ["committed:5"]); h.client.close();
});
test("released attachment cannot install a gated repair", async () => {
  const h = await harness(); const repair = h.session.resync(); const read = await nextRequest(h, "session/snapshot", 0);
  const release = h.session.detach(); const detach = await nextRequest(h, "session/detach", 0); h.transport.respond(detach.id, { type: "detached" }); await release;
  h.transport.respond(read.id, { type: "snapshot", snapshot: snapshot({ shutting_down: true }), cursor: "8" }); await repair;
  assert.equal(h.session.state.runtimeShutdown, false); assert.equal(h.transport.transportCount("session/subscribe"), 0); h.client.close();
});
test("a gated older page joins an intervening native read-domain event", async () => {
  const h = await harness(snapshot({ transcript: { entries: [entry("4")], next_cursor: "4" } }));
  const loading = h.session.loadOlderTranscript(); const request = await nextRequest(h, "session/transcript", 0);
  publish(h, "1", { entries: [entry("4"), entry("5")], next_cursor: "4" });
  h.transport.respond(request.id, { type: "transcript_window", window: window({ entries: [entry("2"), entry("3")], next_cursor: "2" }) });
  assert.equal(await loading, true); assert.deepEqual(h.session.state.transcript.map(e => e.key), ["committed:2", "committed:3", "committed:4", "committed:5"]); h.client.close();
});

test("refresh replaces only unjoinable or still-mutable history windows", () => {
  for (const old of [entry("1"), { ...entry("1", true), response_pending: true }]) {
    const state = replaceFromSnapshot(snapshot({ transcript: { entries: [old, entry("4")], next_cursor: "1" } }), "1");
    const refreshed = reduce(state, { cursor: "2", event: { type: "read_domains_updated", transcript: { entries: [entry(old.response_pending ? "4" : "8")], next_cursor: "4" } } });
    assert.equal(refreshed.transcript.length, 1); assert.equal(refreshed.transcriptNextCursor, "4");
  }
});
test("failed repair keeps incremental continuation retired until an authoritative cut succeeds", async () => {
  const h = await harness();
  const repair = h.session.resync(); const failed = assert.rejects(repair, /read failed/);
  const read = await nextRequest(h, "session/snapshot", 0);
  h.transport.respondError(read.id, { code: -32000, message: "read failed", data: { kind: "invalid_state" } });
  await failed;
  publish(h, "1", { entries: [entry("unsafe")] }); assert.equal(h.session.state.cursor, "0");
  const retry = h.session.resync(); const fresh = await nextRequest(h, "session/snapshot", 1);
  h.transport.respond(fresh.id, { type: "snapshot", snapshot: snapshot(), cursor: "4" });
  const subscribe = await nextRequest(h, "session/subscribe", 0);
  h.transport.respond(subscribe.id, { type: "subscribed", after_cursor: "4" }); await retry;
  publish(h, "5", { entries: [entry("5")] }); assert.equal(h.session.state.cursor, "5"); h.client.close();
});
