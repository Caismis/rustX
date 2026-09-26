import assert from "node:assert/strict";
import { test } from "node:test";
import { SubagentTranscript } from "../src/app-server/subagent-transcript.ts";
import { harness, nextRequest } from "./support/app-server-harness.ts";
import { snapshot, userMessage } from "./support/fixtures.ts";
import type { RuntimeClientTranscriptPage } from "../src/protocol/app-server.ts";
const page = (id: string, next?: string): RuntimeClientTranscriptPage => ({ entries: [{ cursor: id, item: { type: "message", message: userMessage(id, id) } }], next_cursor: next });
function controlled(id = "A") {
  const reads: { id: string; before?: string; resolve: (page: RuntimeClientTranscriptPage) => void; reject: (error: Error) => void }[] = [];
  const reader = new SubagentTranscript({ subagentTranscriptPage: (id, before) => new Promise((resolve, reject) => { reads.push({ id, before, resolve, reject }); }) }, id);
  return { reads, reader };
}
test("child selection disposes A before B; late A success or failure cannot update B", async () => {
  for (const fails of [false, true]) {
    const a = controlled("A"); const b = controlled("B");
    const old = a.reader.newest();
    a.reader.dispose();
    const fresh = b.reader.newest();
    b.reads[0]!.resolve(page("20", "20")); await fresh;
    if (fails) a.reads[0]!.reject(new Error("old unavailable"));
    else a.reads[0]!.resolve(page("10"));
    await old;
    assert.equal(a.reader.page, undefined);
    assert.deepEqual(b.reader.page, page("20", "20"));
    assert.equal(b.reader.error, undefined);
  }
});
test("paging replaces one bounded page and exact boundary generation rejects stale pages", async () => {
  const { reads, reader } = controlled();
  const first = reader.newest(); reads[0]!.resolve(page("30", "30")); await first;
  const older = reader.older(); assert.equal(reads[1]!.before, "30");
  const newest = reader.newest(); reads[2]!.resolve(page("40", "40")); await newest;
  reads[1]!.resolve(page("20", "20")); await older;
  assert.deepEqual(reader.page, page("40", "40"));
  const next = reader.older(); reads[3]!.resolve(page("30", "30")); await next;
  assert.deepEqual(reader.page, page("30", "30"));
  await reader.refresh(); assert.equal(reads.length, 4, "reading older pauses newest polling");
});
test("refresh reads transcript authority; termination cannot invent settlement; disposal fences reads", async () => {
  const { reads, reader } = controlled();
  const first = reader.refresh(); reads[0]!.resolve(page("10")); await first;
  const refreshed = reader.refresh(); reads[1]!.resolve(page("10")); await refreshed;
  assert.deepEqual(reader.page, page("10"));
  const late = reader.refresh(); reader.dispose(); reads[2]!.resolve(page("20")); await late;
  assert.equal(reader.page, undefined); await reader.refresh(); assert.equal(reads.length, 3);
});
test("unavailable history is explicit and clears the obsolete projection", async () => {
  const { reads, reader } = controlled();
  const first = reader.newest(); reads[0]!.resolve(page("10")); await first;
  const next = reader.refresh(); reads[1]!.reject(new Error("subagent_history_unavailable")); await next;
  assert.equal(reader.page, undefined); assert.match(reader.error!, /Child history unavailable/);
});
test("parent detach fences child continuation; reads send exact parent and Subagent only", async () => {
  const h = await harness();
  const reading = h.session.subagentTranscriptPage("A", "12");
  const read = await nextRequest(h, "subagent/transcript", 0);
  assert.deepEqual(read.params, { target: h.target, subagent_id: "A", before: "12", limit: 32 });
  const detached = h.session.detach();
  const detach = await nextRequest(h, "session/detach", 0);
  h.transport.respond(detach.id, { type: "detached" }); await detached;
  h.transport.respond(read.id, { type: "transcript", page: page("10") });
  assert.equal(await reading, undefined);
  assert.equal(h.transport.transportCount("session/attach"), 1);
  for (const method of ["turn/start", "turn/steer", "subagent/cancel", "interaction/respond"] as const) assert.equal(h.transport.transportCount(method), 0);
  h.client.close();
});
test("resync fences old child read; authoritative reread reconstructs without writes", async () => {
  const h = await harness();
  const reader = new SubagentTranscript(h.session, "A");
  const old = reader.newest(); const read = await nextRequest(h, "subagent/transcript", 0);
  const repair = h.session.resync(); const snap = await nextRequest(h, "session/snapshot", 0);
  h.transport.respond(snap.id, { type: "snapshot", snapshot: snapshot(), cursor: "2" });
  const sub = await nextRequest(h, "session/subscribe", 0); h.transport.respond(sub.id, { type: "subscribed", after_cursor: "2" }); await repair;
  const fresh = reader.newest(); const next = await nextRequest(h, "subagent/transcript", 1);
  h.transport.respond(next.id, { type: "transcript", page: page("20") }); await fresh;
  h.transport.respond(read.id, { type: "transcript", page: page("10") }); await old;
  assert.deepEqual(reader.page, page("20"));
  assert.deepEqual(h.transport.log.requests.map(r => r.method), ["initialize", "session/attach", "subagent/transcript", "session/snapshot", "session/subscribe", "subagent/transcript"]);
  reader.dispose(); h.client.close();
});
