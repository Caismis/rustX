import assert from "node:assert/strict";
import { test } from "node:test";
import { harness, nextRequest } from "./support/app-server-harness.ts";
import { snapshot, assistantMessage, userMessage } from "./support/fixtures.ts";
import { refreshFromSnapshot, replaceFromSnapshot } from "../src/presentation/projection.ts";
import type { RuntimeClientTranscriptEntry } from "../src/protocol/app-server.ts";

const entry = (cursor: string, assistant = false): RuntimeClientTranscriptEntry => ({ cursor, item: { type: "message", message: assistant ? assistantMessage(cursor, "answer") : userMessage(cursor, "older") } });
const response = { closing_message_id: "4", origin: { conversation_id: "conv_01900000-0000-7000-8000-000000000002", attempt_id: "old", closing_message_id: "4" }, completed_at: "2026-01-01T00:00:00Z", surface_revision: "4", timing: { total_duration_ms: 18000 } };
const settled = (h: Awaited<ReturnType<typeof harness>>, cursor: string) => h.session.applyNotification({ jsonrpc: "2.0", method: "session/event", params: { target: h.target, cursor, event: { type: "attempt_settled", attempt_id: "a1", outcome: { type: "completed", finish_reason: { type: "stop" } } } } });
const published = (h: Awaited<ReturnType<typeof harness>>) => new Promise<void>(resolve => { const stop = h.session.onState(() => { stop(); resolve(); }); });

test("settlement refresh retains loaded history and older cursor, enriches exact overlap and installs native totals", async () => {
  const h = await harness(snapshot({ transcript: { entries: [entry("4", true)], next_cursor: "4" } }));
  const loading = h.session.loadOlderTranscript();
  const page = await nextRequest(h, "session/transcript", 0);
  h.transport.respond(page.id, { type: "transcript", page: { entries: [entry("2"), entry("3")], next_cursor: "2" } });
  await loading;
  let replacements = 0; h.session.onSnapshot(() => replacements++);
  settled(h, "1");
  const read = await nextRequest(h, "session/snapshot", 0);
  const done = published(h);
  const statistics = { completed_responses: "50", model_requests: "100", requests_with_usage: "99" };
  h.transport.respond(read.id, { type: "snapshot", snapshot: snapshot({ transcript: { entries: [{ ...entry("4", true), completed_response: response }], next_cursor: "4", statistics } }), cursor: "1" });
  await done;
  assert.deepEqual(h.session.state.transcript.map(e => e.key), ["committed:2", "committed:3", "committed:4"]);
  const answer = h.session.state.transcript[2]!;
  assert.equal(answer.kind, "committed"); if (answer.kind === "committed") assert.deepEqual(answer.completedResponse, response);
  assert.equal(h.session.state.transcriptNextCursor, "2");
  assert.deepEqual(h.session.state.statistics, statistics);
  assert.equal(replacements, 0); assert.equal(h.session.resyncCount, 0);
  assert.equal(h.transport.transportCount("session/subscribe"), 0);
  h.client.close();
});

test("live reads cannot overwrite newer events; settlement reads coalesce without subscriptions", async () => {
  const h = await harness(); settled(h, "9007199254740993");
  const first = await nextRequest(h, "session/snapshot", 0);
  settled(h, "9007199254740994");
  h.transport.respond(first.id, { type: "snapshot", snapshot: snapshot({ shutting_down: true }), cursor: "9007199254740993" });
  const second = await nextRequest(h, "session/snapshot", 1);
  assert.equal(h.session.state.runtimeShutdown, false);
  assert.equal(h.session.state.cursor, "9007199254740994");
  const done = published(h);
  h.transport.respond(second.id, { type: "snapshot", snapshot: snapshot(), cursor: "9007199254740994" }); await done;
  assert.equal(h.transport.transportCount("session/snapshot"), 2);
  assert.equal(h.transport.transportCount("session/subscribe"), 0); h.client.close();
});

test("real resync fences an in-flight live read and still replaces projection ownership", async () => {
  const h = await harness(); settled(h, "1");
  const live = await nextRequest(h, "session/snapshot", 0);
  let replacements = 0; h.session.onSnapshot(() => replacements++);
  const repairing = h.session.resync();
  const repair = await nextRequest(h, "session/snapshot", 1);
  h.transport.respond(repair.id, { type: "snapshot", snapshot: snapshot(), cursor: "3" });
  const subscribe = await nextRequest(h, "session/subscribe", 0);
  h.transport.respond(subscribe.id, { type: "subscribed", after_cursor: "3" }); await repairing;
  h.transport.respond(live.id, { type: "snapshot", snapshot: snapshot({ shutting_down: true }), cursor: "2" });
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.equal(h.session.state.cursor, "3"); assert.equal(h.session.state.runtimeShutdown, false);
  assert.equal(replacements, 1); h.client.close();
});

test("refresh replaces only unjoinable or still-mutable history windows", () => {
  for (const old of [entry("1"), { ...entry("1", true), response_pending: true }]) {
    const state = replaceFromSnapshot(snapshot({ transcript: { entries: [old, entry("4")], next_cursor: "1" } }), "1");
    const refreshed = refreshFromSnapshot(state, snapshot({ transcript: { entries: [entry(old.response_pending ? "4" : "8")], next_cursor: "4" } }), "2");
    assert.equal(refreshed.transcript.length, 1); assert.equal(refreshed.transcriptNextCursor, "4");
  }
});

test("released attachment ignores an in-flight live read", async () => {
  const h = await harness(); settled(h, "1");
  const live = await nextRequest(h, "session/snapshot", 0);
  const detached = h.session.detach();
  const request = await nextRequest(h, "session/detach", 0);
  h.transport.respond(request.id, { type: "detached" }); await detached;
  h.transport.respond(live.id, { type: "snapshot", snapshot: snapshot({ shutting_down: true }), cursor: "2" });
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.equal(h.session.state.cursor, "1"); assert.equal(h.session.state.runtimeShutdown, false); h.client.close();
});

test("live overlap requires both exact identity and durable cursor", () => {
  const original = replaceFromSnapshot(snapshot({ transcript: { entries: [entry("1"), entry("4")], next_cursor: "1" } }), "1");
  for (const conflicting of [{ ...entry("1"), cursor: "2" }, { ...entry("2"), cursor: "1" }]) {
    const native = snapshot({ transcript: { entries: [conflicting, entry("4")], next_cursor: "2" } });
    const fresh = refreshFromSnapshot(original, native, "2");
    assert.equal(fresh.transcript.length, 2);
    assert.equal(fresh.transcriptNextCursor, "2");
  }
});
