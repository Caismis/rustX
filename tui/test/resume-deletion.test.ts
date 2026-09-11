import { ConnectionClosedError, RuntimeRequestError } from "../src/runtime/connection.ts";
import assert from "node:assert/strict";
import { test } from "node:test";
import { SessionDeletionWorkflow, type DeletionClient } from "../src/ui/session-deletion-workflow.ts";
import { ResumeSelector } from "../src/ui/components/resume-selector.ts";
import type { SessionDeleteResult, SessionSummaryView } from "../src/protocol/types.ts";
import { plainText } from "../src/ui/theme.ts";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const turn = () => new Promise<void>((resolve) => setImmediate(resolve));
const row = (id: string): SessionSummaryView => ({ id, name: id, active_node: id, active: false, updated_at: "2026-09-11" });
const preview = (id = "b", revision = "revision-1"): SessionDeleteResult => ({ status: "preview", preview: { session_id: id, name: id, target_revision: revision, owned_node_count: 1, owned_conversation_count: 2, owned_child_count: 1 } });
const down = "\x1b[B", del = "\x04", esc = "\x1b";
const confirm = (view: ResumeSelector) => { view.handleInput("\t"); view.handleInput("\r"); };
function harness(options: { rows?: SessionSummaryView[]; query?: string; nextOffset?: number } = {}) {
  const previews: string[] = [], executes: string[][] = [], recovers: string[] = [], lists: Array<[string | undefined, number | undefined]> = [], feedback: string[] = [];
  let closed = false;
  let nativeRows = options.rows ?? [row("a"), row("b"), row("c")];
  let previewResponse = deferred<SessionDeleteResult>();
  let execution = deferred<SessionDeleteResult>();
  let recovery = deferred<SessionDeleteResult>();
  let list = async (_query?: string, _offset?: number): Promise<{ sessions: SessionSummaryView[]; nextOffset?: number }> => ({ sessions: nativeRows });
  const client: DeletionClient = {
    previewSessionDeletion: (id) => { previews.push(id); return previewResponse.promise; },
    deleteSession: (id, revision) => { executes.push([id, revision]); return execution.promise; },
    recoverSessionDeletion: (id) => { recovers.push(id); return recovery.promise; },
    listSessions: (query, offset) => { lists.push([query, offset]); return list(query, offset); },
  };
  const workflow = new SessionDeletionWorkflow(client, () => true, (text) => feedback.push(text));
  const view = new ResumeSelector({ initialPage: { sessions: nativeRows, nextOffset: options.nextOffset }, query: options.query, alive: () => !closed, feedback: (text) => feedback.push(text), client, workflow });
  view.onCancel = () => { closed = true; };
  return { view, workflow, client, previews, executes, recovers, lists, feedback,
    dispose: () => { closed = true; view.dispose(); },
    get closed() { return closed; },
    get previewResponse() { return previewResponse; }, get execution() { return execution; }, get recovery() { return recovery; },
    setPreview: () => { previewResponse = deferred(); }, setExecution: () => { execution = deferred(); },
    rows: (rows: SessionSummaryView[]) => { nativeRows = rows; }, setList: (fn: typeof list) => { list = fn; },
    text: () => view.render(100).map(plainText).join("\n"),
    open: async () => { view.handleInput(down); view.handleInput(del); previewResponse.resolve(preview()); await turn(); },
  };
}

test("1–7: exact preview, safe default/cancel, immutable confirmation, focus and single submit", async () => {
  const h = harness();
  await h.open();
  assert.deepEqual(h.previews, ["b"]);
  assert.deepEqual(h.executes, []);
  assert.match(h.text(), /❯ Cancel/);
  h.view.handleInput("\r");
  assert.deepEqual(h.executes, []);
  h.view.handleInput(del); await turn(); h.view.handleInput(esc);
  assert.deepEqual(h.executes, []);
  h.view.handleInput(del); await turn();
  h.view.selector.replacePage([row("different")]);
  for (const input of [down, "x", del]) h.view.handleInput(input);
  assert.deepEqual(h.lists, []);
  assert.equal(h.previews.length, 3);
  confirm(h.view); h.view.handleInput("\r"); h.view.handleInput(del);
  assert.deepEqual(h.executes, [["b", "revision-1"]]);
});
for (const [reason, text] of [
  [{ kind: "current_session" }, /\/new/], [{ kind: "in_use" }, /currently in use/],
  [{ kind: "workspace", resource_count: 3 }, /3 retained.*disposal/], [{ kind: "invalid_ownership" }, /ownership.*blocked/],
] as const) test(`8–10: native ${reason.kind} is a non-executable blocker`, async () => {
  const h = harness({ rows: [{ ...row("b"), active: true }] });
  h.view.handleInput(del);
  assert.deepEqual(h.previews, ["b"], "active flag cannot bypass Rust preview");
  h.previewResponse.resolve({ status: "blocked", session_id: "b", reason }); await turn();
  assert.match(h.text(), text);
  confirm(h.view); h.view.handleInput(del); h.view.handleInput("r");
  assert.deepEqual(h.executes, []); assert.deepEqual(h.recovers, []); assert.equal(h.previews.length, 1);
  h.view.handleInput(esc); assert.equal(h.view.selector.visibleSessions().length, 1);
});
test("11: stale requires a fresh token and second explicit confirmation", async () => {
  const h = harness(); await h.open(); confirm(h.view);
  h.setPreview(); h.execution.resolve({ status: "stale", session_id: "b" }); await turn();
  assert.deepEqual(h.previews, ["b", "b"]);
  h.previewResponse.resolve(preview("b", "revision-2")); await turn();
  assert.equal(h.executes.length, 1); assert.match(h.text(), /❯ Cancel/);
  h.setExecution(); confirm(h.view);
  assert.deepEqual(h.executes, [["b", "revision-1"], ["b", "revision-2"]]);
});
test("12–13,20: no optimistic removal; authoritative rebuild selects next, previous, empty", async () => {
  for (const rows of [[row("a"), row("c")], [row("a")], []]) {
    const h = harness(); await h.open(); confirm(h.view);
    assert.deepEqual(h.view.selector.visibleSessions().map((r) => r.id), ["a", "b", "c"]);
    h.rows(rows); h.execution.resolve({ status: "deleted", session_id: "b" }); await turn();
    assert.deepEqual(h.lists, [["", 0]]);
    assert.equal(h.view.selector.selectedSession()?.id, rows.at(-1)?.id);
    assert.match(h.feedback.join(), /permanently deleted/);
  }
});
test("16: unknown execute outcome reconciles native rows and never replays execute", async () => {
  for (const remains of [true, false]) {
    const h = harness(); await h.open(); confirm(h.view);
    if (!remains) h.rows([row("a"), row("c")]);
    h.execution.reject(new Error("response lost without a semantic result")); await turn();
    assert.match(h.text(), /outcome unknown/); assert.doesNotMatch(h.text(), /delete failed/);
    assert.equal(h.view.selector.visibleSessions().some((r) => r.id === "b"), remains);
    assert.equal(h.executes.length, 1); assert.deepEqual(h.lists, [["", 0]]);
  }
});
test("14: committed cleanup stays absent and retry is native and single-submit", async () => {
  const h = harness(); await h.open(); confirm(h.view); h.rows([row("a")]);
  h.execution.resolve({ status: "committed_cleanup_pending", session_id: "b" }); await turn();
  assert.match(h.text(), /removed and cannot be resumed/);
  assert.deepEqual(h.view.selector.visibleSessions().map((r) => r.id), ["a"]);
  h.view.handleInput("r"); h.view.handleInput("r"); confirm(h.view);
  assert.deepEqual(h.recovers, ["b"]); assert.equal(h.executes.length, 1);
  h.recovery.resolve({ status: "deleted", session_id: "b" }); await turn();
  assert.equal(h.lists.length, 2);
});
test("15: durability uncertainty and not-found are distinct from definite failure/success", async () => {
  for (const status of ["committed_durability_uncertain", "not_found"] as const) {
    const h = harness(); await h.open(); confirm(h.view); h.rows([]);
    h.execution.resolve({ status, session_id: "b" }); await turn();
    assert.match(h.text(), status === "not_found" ? /absent/ : /durability is uncertain/);
    assert.doesNotMatch(h.text() + h.feedback.join(), /permanently deleted|delete failed/);
    assert.deepEqual(h.view.selector.visibleSessions(), []);
  }
});
test("17–19: old continuation cannot resurrect rows; search and fresh page offsets survive mutation", async () => {
  const h = harness({ rows: [row("b0"), row("b1")], query: "b", nextOffset: 2 });
  const oldPage = deferred<{ sessions: SessionSummaryView[]; nextOffset?: number }>();
  h.setList(async () => oldPage.promise);
  h.view.handleInput(down); h.view.handleInput(down); // starts old continuation
  h.view.selector.selectIdentity("b1"); h.view.handleInput(del);
  h.previewResponse.resolve(preview("b1")); await turn(); confirm(h.view);
  h.setList(async (_q, offset) => offset === 0 ? { sessions: [row("b0")], nextOffset: 1 } : { sessions: [row("b2")], nextOffset: 2 });
  h.execution.resolve({ status: "deleted", session_id: "b1" }); await turn();
  oldPage.resolve({ sessions: [row("b1"), row("b2")], nextOffset: 4 }); await turn();
  assert.deepEqual(h.lists, [["b", 2], ["b", 0], ["b", 1]]);
  assert.deepEqual(h.view.selector.visibleSessions().map((r) => r.id), ["b0", "b2"]);
  assert.equal(h.view.selector.selectedSession()?.id, "b2");
});
test("empty/nonmatching Ctrl+D is inert; confirmation is bounded and sanitizes external names", async () => {
  const empty = harness({ rows: [] }); empty.view.handleInput(del); assert.deepEqual(empty.previews, []);
  const h = harness(); h.view.handleInput(del);
  const value = preview("a"); assert.equal(value.status, "preview");
  if (value.status === "preview") value.preview.name = "weird\x1b[2J\n\u202e名字";
  h.previewResponse.resolve(value); await turn();
  h.view.setBodyHeight(3); const lines = h.view.render(12).map(plainText);
  assert.ok(lines.length <= 3); assert.ok(lines.every((line) => !/[\x1b\n\u202e]/.test(line)));
});

test("22: presentation deletion path has no filesystem/process/provider authority", async () => {
  const { readFile } = await import("node:fs/promises");
  const source = (await Promise.all(["../src/ui/components/resume-selector.ts", "../src/ui/session-deletion-workflow.ts"].map((path) => readFile(new URL(path, import.meta.url), "utf8")))).join("\n");
  assert.doesNotMatch(source, /node:|RuntimeClientConnection|submitInbound|selectSession|newSession|modelSet|spawn|unlink|removeDirectory|\.request\(/);
  const attachment = await readFile(new URL("../src/runtime/attachment.ts", import.meta.url), "utf8");
  const deletion = attachment.slice(attachment.indexOf("  previewSessionDeletion("), attachment.indexOf("  /** Lists bounded persisted Sessions"));
  assert.doesNotMatch(deletion, /node:|unlink|spawn|submitInbound|#state\s*=/);
});


test("execute pending retains settlement across Esc and presents native CleanupPending", async () => {
  const h = harness(); await h.open(); confirm(h.view);
  for (const input of [esc, "\r", del, "r", down, "x", esc]) h.view.handleInput(input);
  assert.equal(h.closed, false, "Esc must not close the outer popup");
  assert.match(h.text(), /Waiting for native deletion/);
  assert.deepEqual(h.view.popupFooter(), []);
  assert.deepEqual(h.executes, [["b", "revision-1"]]);
  assert.deepEqual(h.previews, ["b"]);
  assert.deepEqual(h.recovers, []);
  assert.deepEqual(h.lists, [], "search input cannot escape the pending surface");
  assert.equal(h.view.selector.selectedSession()?.id, "b");
  h.rows([row("a"), row("c")]);
  h.execution.resolve({ status: "committed_cleanup_pending", session_id: "b" }); await turn();
  assert.match(h.text(), /removed and cannot be resumed.*needs cleanup/);
  assert.deepEqual(h.lists, [["", 0]]);
  assert.deepEqual(h.view.selector.visibleSessions().map((r) => r.id), ["a", "c"]);
  assert.match(h.view.popupFooter().join(), /R retry native cleanup/);
  h.view.handleInput("r"); h.view.handleInput("r");
  assert.deepEqual(h.recovers, ["b"]);
});

test("execute pending retains durability uncertainty and reconciliation after Esc", async () => {
  const h = harness(); await h.open(); confirm(h.view);
  h.view.handleInput(esc);
  assert.equal(h.closed, false);
  assert.match(h.text(), /Waiting for native deletion/);
  h.rows([]);
  h.execution.resolve({ status: "committed_durability_uncertain", session_id: "b" }); await turn();
  assert.match(h.text(), /durability is uncertain/);
  assert.doesNotMatch(h.text() + h.feedback.join(), /delete failed|permanently deleted/);
  assert.deepEqual(h.lists, [["", 0]]);
  assert.deepEqual(h.view.selector.visibleSessions(), []);
  assert.equal(h.executes.length, 1);
});

test("recover pending retains settlement across Esc and reconciles native Deleted", async () => {
  const h = harness(); await h.open(); confirm(h.view); h.rows([row("a")]);
  h.execution.resolve({ status: "committed_cleanup_pending", session_id: "b" }); await turn();
  h.view.handleInput("r");
  for (const input of [esc, "r", "\r", del, down, "x", esc]) h.view.handleInput(input);
  assert.equal(h.closed, false, "recovery must retain the outer popup");
  assert.match(h.text(), /Waiting for native cleanup/);
  assert.deepEqual(h.view.popupFooter(), []);
  assert.deepEqual(h.recovers, ["b"]);
  assert.deepEqual(h.previews, ["b"]);
  assert.equal(h.executes.length, 1);
  assert.deepEqual(h.lists, [["", 0]]);
  h.recovery.resolve({ status: "deleted", session_id: "b" }); await turn();
  assert.match(h.feedback.join(), /Session permanently deleted/);
  assert.equal(h.view.popupTitle(), "Resume session");
  assert.deepEqual(h.lists, [["", 0], ["", 0]]);
  assert.deepEqual(h.view.selector.visibleSessions().map((r) => r.id), ["a"]);
});

test("preview pending remains cancellable and its late response cannot reopen confirmation", async () => {
  const h = harness(); h.view.handleInput(down); h.view.handleInput(del);
  assert.match(h.text(), /Waiting for native preview/);
  assert.deepEqual(h.view.popupFooter(), ["Esc cancel"]);
  h.view.handleInput(esc);
  assert.equal(h.closed, false);
  assert.equal(h.view.popupTitle(), "Resume session");
  h.previewResponse.resolve(preview()); await turn();
  assert.equal(h.view.popupTitle(), "Resume session");
  assert.doesNotMatch(h.text(), /Permanently delete/);
  assert.deepEqual(h.executes, []);
});


test("a remounted stale workflow preserves the query and neighbor anchor through fresh confirmation", async () => {
  const history = (id: string) => ({ ...row(id), name: `history-${id}` });
  const h = harness({ query: "history", rows: [history("a"), history("b"), history("c")] });
  await h.open(); confirm(h.view); h.dispose(); h.setPreview();
  h.execution.resolve({ status: "stale", session_id: "b" }); await turn();
  assert.deepEqual(h.previews, ["b"], "no disposed surface can start a new preview");
  const replacement = new ResumeSelector({ query: h.workflow.context.query,
    client: h.client, workflow: h.workflow, alive: () => true, feedback: () => {} });
  h.previewResponse.resolve(preview("b", "revision-2")); await turn();
  assert.equal(h.executes.length, 1);
  h.setExecution(); confirm(replacement);
  assert.deepEqual(h.executes, [["b", "revision-1"], ["b", "revision-2"]]);
  h.rows([history("a"), history("c")]);
  h.execution.resolve({ status: "deleted", session_id: "b" }); await turn();
  assert.deepEqual(h.lists, [["history", 0], ["history", 0]]);
  assert.equal(replacement.selector.selectedSession()?.id, "c");
  replacement.dispose();
});

for (const outcome of ["committed_cleanup_pending", "committed_durability_uncertain", "unknown"] as const) {
  test(`${outcome}: failed reconciliation is not empty authority and fresh remount retains recovery`, async () => {
    const h = harness();
    assert.deepEqual(h.workflow.reconciliation, { kind: "none" });
    await h.open(); confirm(h.view);
    const refresh = deferred<{ sessions: SessionSummaryView[] }>();
    h.setList(() => refresh.promise);
    if (outcome === "unknown") h.execution.reject(new Error("healthy request rejection"));
    else h.execution.resolve({ status: outcome, session_id: "b" });
    await turn();
    assert.deepEqual(h.workflow.reconciliation, { kind: "pending" });
    assert.equal(h.workflow.generation, 1);
    refresh.reject(new Error("native list unavailable")); await turn();
    assert.deepEqual(h.workflow.reconciliation, { kind: "failed" });
    assert.deepEqual(h.workflow.state, { kind: "result", outcome: outcome === "unknown" ? { status: "unknown" } : { status: outcome, session_id: "b" }, sessionId: "b" });
    assert.equal(h.workflow.canRecover(), true);
    assert.equal(h.executes.length, 1);
    assert.doesNotMatch(h.text() + h.feedback.join(), /delete failed/);
    assert.match(h.feedback.join(), /visibility could not be refreshed/);
    h.view.handleInput(esc);
    assert.match(h.text(), /visibility unavailable/);
    assert.doesNotMatch(h.text(), /No sessions|❯/);
    h.dispose();
    // An explicit current-generation native list succeeds independently of the
    // retained deletion result. Its rows must survive mounting the old workflow.
    h.rows([row("a"), row("c")]); h.setList(async () => ({ sessions: [row("a"), row("c")] }));
    const initialPage = await h.client.listSessions("", 0);
    const replacement = new ResumeSelector({ initialPage, client: h.client, workflow: h.workflow, alive: () => true, feedback: () => {} });
    assert.deepEqual(replacement.selector.visibleSessions().map((r) => r.id), ["a", "c"]);
    assert.equal(replacement.selector.selectedSession()?.id, "c");
    replacement.handleInput("r"); replacement.handleInput("r");
    assert.deepEqual(h.recovers, ["b"], "recovery uses the native result, not selected C");
    assert.equal(h.executes.length, 1);
    replacement.dispose();
  });
}

test("only a successful reconciliation can publish an authoritative empty page", async () => {
  const h = harness(); await h.open(); confirm(h.view); h.rows([]);
  h.execution.resolve({ status: "committed_cleanup_pending", session_id: "b" }); await turn();
  assert.deepEqual(h.workflow.reconciliation, { kind: "ready", query: "", page: { sessions: [], nextOffset: undefined } });
  h.view.handleInput(esc);
  assert.deepEqual(h.view.selector.visibleSessions(), []);
  assert.doesNotMatch(h.text(), /visibility unavailable/);
});

test("Deleted plus failed list refresh reports success without empty authority or recovery attention", async () => {
  const h = harness(); await h.open(); confirm(h.view);
  h.setList(async () => { throw new Error("list rejected"); });
  h.execution.resolve({ status: "deleted", session_id: "b" }); await turn();
  assert.deepEqual(h.workflow.reconciliation, { kind: "failed" });
  assert.equal(h.workflow.canRecover(), false); assert.equal(h.workflow.needsPresentation, false);
  assert.match(h.feedback.join(), /permanently deleted/);
  assert.match(h.text(), /visibility unavailable/);
  h.view.handleInput(del); h.view.handleInput("r"); h.view.handleInput("\r");
  assert.deepEqual(h.recovers, []); assert.equal(h.executes.length, 1);
  h.dispose();
  const replacement = new ResumeSelector({ initialPage: { sessions: [row("a"), row("c")] }, client: h.client, workflow: h.workflow, alive: () => true, feedback: () => {} });
  assert.deepEqual(replacement.selector.visibleSessions().map((r) => r.id), ["a", "c"]);
  assert.doesNotMatch(replacement.render(100).join(), /visibility unavailable/);
  replacement.dispose();
});

test("failed continuation reconciliation publishes neither a partial page nor empty authority", async () => {
  const h = harness(); await h.open(); confirm(h.view);
  h.setList(async (_q, offset) => {
    if (offset === 0) return { sessions: [row("a")], nextOffset: 1 };
    throw new Error("fresh continuation rejected");
  });
  h.execution.resolve({ status: "committed_cleanup_pending", session_id: "b" }); await turn();
  assert.deepEqual(h.workflow.reconciliation, { kind: "failed" });
  assert.deepEqual(h.lists, [["", 0], ["", 1]]);
  assert.equal(h.workflow.canRecover(), true);
});

test("pre-mutation continuation stays invalid after reconciliation failure and a fresh remount", async () => {
  const h = harness({ rows: [row("b0"), row("b1")], query: "b", nextOffset: 2 });
  const oldPage = deferred<{ sessions: SessionSummaryView[] }>();
  h.setList(() => oldPage.promise);
  h.view.handleInput(down); h.view.handleInput(down);
  h.view.selector.selectIdentity("b1"); h.view.handleInput(del);
  h.previewResponse.resolve(preview("b1")); await turn(); confirm(h.view);
  h.setList(async () => { throw new Error("refresh failed"); });
  h.execution.resolve({ status: "committed_cleanup_pending", session_id: "b1" }); await turn();
  assert.deepEqual(h.workflow.reconciliation, { kind: "failed" });
  assert.equal(h.workflow.generation, 1);
  h.dispose();
  h.setList(async () => ({ sessions: [row("b0"), row("b2")] }));
  const initialPage = await h.client.listSessions("b", 0);
  const replacement = new ResumeSelector({ initialPage, query: "b", client: h.client, workflow: h.workflow, alive: () => true, feedback: () => {} });
  oldPage.resolve({ sessions: [row("b1")] }); await turn();
  assert.deepEqual(replacement.selector.visibleSessions().map((r) => r.id), ["b0", "b2"]);
  assert.deepEqual(h.lists, [["b", 2], ["b", 0], ["b", 0]]);
  assert.equal(h.workflow.generation, 1);
  replacement.handleInput("r"); replacement.handleInput("r"); assert.deepEqual(h.recovers, ["b1"]);
  replacement.dispose();
});

test("fresh initial native authority supersedes an older successful workflow page", async () => {
  const h = harness(); await h.open(); confirm(h.view); h.rows([row("a"), row("c")]);
  h.execution.resolve({ status: "committed_cleanup_pending", session_id: "b" }); await turn(); h.dispose();
  const replacement = new ResumeSelector({ initialPage: { sessions: [] }, client: h.client, workflow: h.workflow, alive: () => true, feedback: () => {} });
  assert.deepEqual(replacement.selector.visibleSessions(), []);
  replacement.handleInput(esc);
  assert.doesNotMatch(replacement.render(100).join(), /visibility unavailable/);
  replacement.dispose();
});

test("a fresh current-generation page survives a concurrent reconciliation failure", async () => {
  const h = harness(); await h.open(); confirm(h.view);
  const refresh = deferred<{ sessions: SessionSummaryView[] }>(); h.setList(() => refresh.promise);
  h.execution.resolve({ status: "committed_cleanup_pending", session_id: "b" }); await turn();
  h.dispose();
  const replacement = new ResumeSelector({ initialPage: { sessions: [row("a"), row("c")] }, client: h.client, workflow: h.workflow, alive: () => true, feedback: () => {} });
  refresh.reject(new Error("concurrent workflow read failed")); await turn();
  replacement.handleInput(esc);
  assert.deepEqual(replacement.selector.visibleSessions().map((r) => r.id), ["a", "c"]);
  assert.doesNotMatch(replacement.render(100).join(), /visibility unavailable/);
  replacement.dispose();
});

test("recovery freezes B but rebuilds the current query with only its fresh offsets", async () => {
  const old = (id: string) => ({ ...row(id), name: `old-${id}` });
  const fresh = (id: string) => ({ ...row(id), name: `new-${id}` });
  const h = harness({ rows: [old("A"), old("B"), old("C")], query: "old", nextOffset: 101 });
  h.view.handleInput(down); h.view.handleInput(del);
  h.previewResponse.resolve(preview("B")); await turn(); confirm(h.view);
  h.setList(async (query, offset) => {
    assert.equal(query, "old");
    if (offset === 0) return { sessions: [old("A"), old("C")], nextOffset: 101 };
    assert.equal(offset, 101); return { sessions: [old("X")], nextOffset: 102 };
  });
  h.execution.resolve({ status: "committed_cleanup_pending", session_id: "B" }); await turn();
  h.view.handleInput(esc);
  h.setList(async () => ({ sessions: [fresh("D"), fresh("E"), fresh("F")], nextOffset: 202 }));
  for (const input of ["\x7f", "\x7f", "\x7f", "n", "e", "w"]) h.view.handleInput(input);
  await turn(); h.view.selector.selectIdentity("E");
  assert.deepEqual(h.view.reconciliationContext(), { query: "new", ids: ["D", "E", "F"], index: 1, loaded: 3 });
  h.view.handleInput(del);
  const start = h.lists.length;
  h.setList(async (query, offset) => {
    assert.equal(query, "new");
    if (offset === 0) return { sessions: [fresh("D")], nextOffset: 202 };
    if (offset === 202) return { sessions: [fresh("E")], nextOffset: 303 };
    if (offset === 303) return { sessions: [fresh("F")], nextOffset: 404 };
    assert.equal(offset, 404); return { sessions: [fresh("G")] };
  });
  for (const input of ["r", "r", "\r", del, "\x1b[A", down, esc]) h.view.handleInput(input);
  assert.deepEqual(h.recovers, ["B"]); assert.equal(h.executes.length, 1);
  h.recovery.resolve({ status: "deleted", session_id: "B" }); await turn();
  assert.deepEqual(h.lists.slice(start), [["new", 0], ["new", 202], ["new", 303]]);
  assert.deepEqual(h.view.selector.visibleSessions().map((r) => r.id), ["D", "E", "F"]);
  assert.equal(h.view.selector.selectedSession()?.id, "E"); assert.match(h.text(), /Search: new/);
  h.view.selector.selectIdentity("F"); h.view.handleInput(down); await turn();
  assert.deepEqual(h.lists.slice(start), [["new", 0], ["new", 202], ["new", 303], ["new", 404]]);
  assert.deepEqual(h.view.selector.visibleSessions().map((r) => r.id), ["D", "E", "F", "G"]);
  h.dispose();
});

test("recovery with unavailable visibility uses the current query without fabricated anchors", async () => {
  const h = harness(); await h.open(); confirm(h.view);
  h.execution.resolve({ status: "committed_cleanup_pending", session_id: "b" }); await turn(); h.dispose();
  const replacement = new ResumeSelector({ query: "new", client: h.client, workflow: h.workflow, alive: () => true, feedback: () => {} });
  assert.deepEqual(replacement.reconciliationContext(), { query: "new", ids: [], index: 0, loaded: 0 });
  const start = h.lists.length;
  h.setList(async () => ({ sessions: [row("new-A")], nextOffset: 202 }));
  replacement.handleInput("r"); replacement.handleInput("r");
  assert.deepEqual(h.recovers, ["b"]);
  h.recovery.resolve({ status: "deleted", session_id: "b" }); await turn();
  assert.deepEqual(h.lists.slice(start), [["new", 0]]);
  assert.deepEqual(replacement.selector.visibleSessions().map((r) => r.id), ["new-A"]);
  replacement.dispose();
});

test("12: typed precommit failure reconciles the target without unknown or recovery authority", async () => {
  const h = harness(); await h.open(); confirm(h.view);
  h.execution.reject(new RuntimeRequestError({ type: "session_failure", message: "Session deletion failed before logical commit." })); await turn();
  assert.deepEqual(h.workflow.state, { kind: "result", sessionId: "b", outcome: { status: "precommit_failure" } });
  assert.deepEqual(h.lists, [["", 0]]); assert.equal(h.executes.length, 1);
  assert.equal(h.workflow.canRecover(), false);
  assert.match(h.text(), /failed before logical commit/);
  assert.doesNotMatch(h.text(), /outcome unknown|durability is uncertain|has been removed|permanently deleted/);
  assert.deepEqual(h.view.selector.visibleSessions().map((r) => r.id), ["a", "b", "c"]);
  h.view.handleInput("r"); h.workflow.recover(h.view.reconciliationContext());
  assert.deepEqual(h.recovers, []);
  h.view.handleInput(esc); h.setPreview(); h.view.handleInput(del);
  assert.deepEqual(h.previews, ["b", "b"]); assert.equal(h.executes.length, 1);
  h.dispose();
});

test("stale obligation is acknowledged only by adopted confirmation or explicit cancellation", async () => {
  const h = harness(); await h.open(); confirm(h.view); h.setPreview();
  h.execution.resolve({ status: "stale", session_id: "b" }); await turn();
  assert.deepEqual(h.workflow.state, { kind: "needs_fresh_preview", sessionId: "b" });
  assert.equal(h.workflow.needsPresentation, true);
  h.previewResponse.resolve(preview("b", "rev2")); await turn();
  assert.match(h.text(), /❯ Cancel/); assert.deepEqual(h.workflow.state, { kind: "idle" });
  assert.equal(h.executes.length, 1);
  h.setExecution(); confirm(h.view); h.setPreview();
  h.execution.resolve({ status: "stale", session_id: "b" }); await turn();
  assert.equal(h.workflow.state.kind, "needs_fresh_preview");
  h.view.handleInput(esc);
  assert.deepEqual(h.workflow.state, { kind: "idle" }); assert.equal(h.workflow.needsPresentation, false);
  h.previewResponse.resolve(preview("b", "rev3")); await turn();
  assert.doesNotMatch(h.text(), /Permanently delete/); assert.equal(h.executes.length, 2);
  h.dispose();
});

test("typed terminal rejection ends observation without unknown result or recovery replay", async () => {
  const h = harness(); await h.open(); confirm(h.view);
  h.execution.reject(new ConnectionClosedError("process_exit", "transport ended")); await turn();
  assert.equal(h.workflow.needsPresentation, false);
  assert.equal(h.workflow.canRecover(), false);
  assert.notEqual(h.workflow.state.kind, "result");
  assert.deepEqual(h.lists, []); assert.deepEqual(h.feedback, []);
  h.workflow.recover(h.view.reconciliationContext());
  assert.deepEqual(h.recovers, []); assert.equal(h.executes.length, 1);
  h.dispose();
});
