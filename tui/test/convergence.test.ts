import assert from "node:assert/strict";
import { test } from "node:test";
import { CURSOR_MARKER, TUI, visibleWidth } from "@earendil-works/pi-tui";
import { ComposerEditor, composerIntent } from "../src/ui/composer.ts";
import { PromptHistory } from "../src/ui/components/prompt-history.ts";
import { PendingInputView } from "../src/ui/components/pending-input.ts";
import { PopupFrame } from "../src/ui/components/popup-frame.ts";
import { renderResourceBanner } from "../src/ui/components/resources.ts";
import { renderFooter } from "../src/ui/components/status.ts";
import { editorSubmission } from "../src/app-server/editor.ts";
import { mergeTranscriptPage, replaceFromSnapshot } from "../src/presentation/projection.ts";
import { stateOf, transcriptString, plain } from "./support/render.ts";
import { harness, nextRequest } from "./support/app-server-harness.ts";
import { snapshot, attemptView, assistantMessage, userMessage } from "./support/fixtures.ts";
import type { RuntimeClientSnapshot, UserInputBlock, MethodParams } from "../src/protocol/app-server.ts";

function terminal(columns = 80, rows = 24): TUI {
  return new TUI({ columns, rows, write() {}, start() {}, stop() {}, hideCursor() {}, showCursor() {}, moveBy() {}, clearLine() {}, clearFromCursor() {}, clearScreen() {}, setTitle() {} } as unknown as ConstructorParameters<typeof TUI>[0]);
}
function pending(sequence: string, revision: string, text: string) {
  return { sequence, revision, message: { id: `m-${sequence}`, source: "human" as const, content: [{ type: "text" as const, text }] } };
}

test("Composer idle Enter, running Enter and running Tab issue exactly one native request each", async () => {
  for (const [running, key, method] of [[false, "\r", "turn/start"], [true, "\r", "turn/steer"], [true, "\t", "turn/start"]] as const) {
    const h = await harness(snapshot({ attempt: running ? attemptView() : undefined }));
    const editor = new ComposerEditor(terminal());
    editor.running = () => running;
    let done: Promise<unknown> | undefined;
    const submit = (text: string, queue: boolean) => {
      const intent = composerIntent(h.session.state, queue);
      done = intent === "steer" ? h.session.steer([{ type: "text", text }]) : h.session.submitInbound([{ type: "text", text }]);
    };
    editor.onPrompt = text => submit(text, false);
    editor.onQueue = text => submit(text, true);
    editor.setText("draft"); editor.handleInput(key);
    const request = await nextRequest(h, method, 0);
    assert.deepEqual(request.params, { target: h.target, content: [{ type: "text", text: "draft" }] });
    h.transport.respond(request.id, { type: "inbound_accepted", message_id: "m", inbound_sequence: "10" });
    await done;
    assert.equal(h.transport.transportCount("turn/start") + h.transport.transportCount("turn/steer"), 1);
    assert.deepEqual(h.session.state.inbound.pending, []);
    h.client.close();
  }
});

test("native queue order survives reconnect reconstruction and is independent of transcript", async () => {
  const inbound = { pending: [pending("9", "2", "first"), pending("10", "9007199254740993", "second")] };
  const h = await harness(snapshot({ inbound }));
  const before = new PendingInputView(terminal(), h.session, () => {}, () => {}).render(80).join("\n");
  const repair = h.session.resync();
  const request = await nextRequest(h, "session/snapshot", 0);
  h.transport.respond(request.id, { type: "snapshot", snapshot: snapshot({ inbound, transcript: { entries: [] } }), cursor: "42" });
  const subscribe = await nextRequest(h, "session/subscribe", 0);
  h.transport.respond(subscribe.id, { type: "subscribed", after_cursor: "42" });
  await repair;
  const after = new PendingInputView(terminal(), h.session, () => {}, () => {}).render(80).join("\n");
  assert.equal(before, after); assert.match(after, /first[\s\S]*second/);
  assert.equal(h.transport.transportCount("turn/start"), 0); h.client.close();
});

test("pending edit/remove use exact observed CAS once, including stale results", async () => {
  for (const method of ["inbound/edit", "inbound/remove"] as const) {
    for (const status of ["applied", "conflict", "not_pending", "durability_uncertain"] as const) {
      const item = pending("9007199254740994", "9007199254740995", "original");
      const h = await harness(snapshot({ inbound: { pending: [item] } }));
      const view = new PendingInputView(terminal(), h.session, () => {}, () => {});
      h.session.updateState(state => ({ ...state, inbound: { pending: [{ ...item, revision: "9007199254740996" }] } }));
      const work = view.mutate(method === "inbound/edit" ? "edited" : undefined);
      const request = await nextRequest(h, method, 0);
      assert.deepEqual((request.params as MethodParams<typeof method>).expected, { sequence: item.sequence, revision: item.revision, message_id: item.message.id });
      h.transport.respond(request.id, { type: "inbound_mutation", outcome: { status } });
      await work; await view.mutate("replay");
      assert.equal(h.transport.transportCount(method), 1);
      assert.match(view.render(120).join("\n"), status === "applied" ? /Applied/ : new RegExp(status));
      h.client.close();
    }
  }
});

test("Welcome consumes native origins, complete tools, extensions and admitted definitions", () => {
  const state = stateOf();
  state.effectivePlugins = { todo: {}, goal: {} };
  const tool = state.capabilities.tools![0]!;
  state.capabilities.tools = [
    { ...tool, id: "b", name: "builtin_tool", origin: "builtin" },
    { ...tool, id: "m", name: "web_search", origin: { mcp: { server_id: "exa" } } },
    { ...tool, id: "p", name: "dataframe", origin: { managed_python: { package: "data-tools" } } },
  ];
  state.resources.inspection.main = { identity: { kind: "main" }, tools: [], tool_selection: [], skills: [], agents: ["researcher"], workflows: ["audit"], plugins: [], diagnostics: [] };
  const banner = plain(renderResourceBanner(state));
  for (const fact of ["Extensions", "Agents", "researcher", "Skills", "Tools", "Builtin", "builtin_tool", "MCP · exa", "web_search", "Managed Python · data-tools", "dataframe", "Workflows", "audit"]) assert.ok(banner.includes(fact), fact);
  state.transcript = [];
  assert.equal(plain(renderResourceBanner(state)), banner);
});

test("uncertain upload responses are not replayed", async () => {
  const u = await harness();
  const upload = u.session.upload("selected.txt", new TextEncoder().encode("bytes"));
  const rejected = assert.rejects(upload, /unknown/);
  await nextRequest(u, "session/upload", 0); u.client.close(); await rejected;
  assert.equal(u.transport.transportCount("session/upload"), 1);
});

test("upload returns native receipts, preserves ordered blocks, never submits a local path", async () => {
  const h = await harness();
  const work = h.session.upload("data.csv", new TextEncoder().encode("a,b"));
  const request = await nextRequest(h, "session/upload", 0);
  assert.deepEqual(request.params, { target: h.target, files: [{ name: "data.csv", data: "YSxi" }] });
  const receipt = { session_id: h.session.sessionId, batch_id: "batch", token: "opaque" };
  h.transport.respond(request.id, { type: "session_uploaded", files: [{ receipt, file: { batch_id: "batch", name: "data.csv" }, path: "/server/uploads/data.csv" }] });
  const blocks: UserInputBlock[] = [{ type: "text", text: "before" }, ...await work, { type: "text", text: "after" }];
  assert.deepEqual(editorSubmission(blocks, "beforeafter"), blocks);
  assert.throws(() => editorSubmission(blocks, "ambiguous edit"), /ordering/);
  const sending = h.session.submitInbound(blocks);
  const sent = await nextRequest(h, "turn/start", 0);
  assert.deepEqual(sent.params, { target: h.target, content: blocks });
  h.transport.respond(sent.id, { type: "inbound_accepted", message_id: "m", inbound_sequence: "1" }); await sending;
  assert.ok(!JSON.stringify(sent.params).includes("local-path"));
  h.client.close();
});

test("local prompt search previews, accepts and Esc leaves the exact draft untouched", () => {
  const draft = new ComposerEditor(terminal()); draft.setText("未送信 e\u0301👨‍👩‍👧‍👦\nexact draft");
  const before = draft.getExpandedText(); let closed = 0;
  const search = new PromptHistory(["first prompt", "second prompt"], text => draft.setText(text), () => closed++);
  search.handleInput("first");
  assert.match(search.render(80).join("\n"), /first prompt/);
  assert.equal(draft.getExpandedText(), before);
  search.handleInput("\x1b"); assert.equal(closed, 1); assert.equal(draft.getExpandedText(), before);
  search.handleInput("\r"); assert.equal(draft.getExpandedText(), "first prompt");
});

test("whole and fragmented bracketed paste cannot Send/Queue/dispatch a slash; large paste stays bounded", () => {
  for (const fragments of [["\x1b[200~/permissions\n\t中😀e\u0301\x1b[201~"], ["\x1b[200~", "/permissions", "\r", "\t", "中😀", "\x1b[201", "~"]]) {
    const editor = new ComposerEditor(terminal()); editor.running = () => true;
    const submitted: boolean[] = []; editor.onPrompt = (_text, commands) => submitted.push(commands); editor.onQueue = () => assert.fail("paste queued");
    for (const fragment of fragments) editor.handleInput(fragment);
    assert.deepEqual(submitted, []); editor.handleInput("\r"); assert.deepEqual(submitted, [false]);
  }
  const editor = new ComposerEditor(terminal()); const payload = "{\"中文\":\"👩‍💻\"}\n".repeat(10000);
  editor.handleInput(`\x1b[200~${payload}\x1b[201~`);
  assert.equal(editor.getExpandedText(), payload); assert.ok(editor.render(40).length < 24);
});

test("Pi grapheme cursor and deletion retain CJK, emoji and combining sequences", () => {
  const editor = new ComposerEditor(terminal()); editor.setText("中👨‍👩‍👧‍👦e\u0301");
  editor.handleInput("\x7f"); assert.equal(editor.getExpandedText(), "中👨‍👩‍👧‍👦");
  editor.handleInput("\x1b[D"); editor.handleInput("\x7f"); assert.equal(editor.getExpandedText(), "👨‍👩‍👧‍👦");
  for (const width of [160, 120, 80, 60, 40]) assert.ok(editor.render(width).every(line => visibleWidth(line) <= width));
});

test("all representative terminal sizes retain bounded Composer and actionable popup", () => {
  for (const [columns, rows] of [[160, 50], [120, 40], [80, 24], [60, 20], [40, 15]] as const) {
    const editor = new ComposerEditor(terminal(columns, rows)); editor.setText("中😀 e\u0301\n".repeat(100));
    const draft = editor.getExpandedText();
    const view = new PromptHistory(["preview"], () => {}, () => {});
    const frame = new PopupFrame(view); frame.setViewportHeight(Math.floor(rows * .75));
    const rendered = frame.render(columns);
    assert.ok(rendered.length <= rows); assert.ok(rendered.every(line => visibleWidth(line) <= columns));
    assert.match(plain(rendered.join("\n")), /preview/);
    assert.ok(editor.render(columns).length <= rows); assert.equal(editor.getExpandedText(), draft);
  }
});

test("exact historical completed_response drives tails and whole-conversation statistics survive paging", () => {
  const message = assistantMessage("answer", "historical response");
  const response = { closing_message_id: "answer", origin: { conversation_id: "origin", attempt_id: "old", closing_message_id: "answer" }, completed_at: "2026-01-01T00:00:00Z", surface_revision: "4", usage: { input_tokens: 100, output_tokens: 20, total_tokens: 120 }, timing: { total_duration_ms: 18000 } };
  const statistics = { turns: "1", steps: "1", completed_responses: "50", model_requests: "100", requests_with_usage: "99", reported_usage: { input_tokens: 10000, output_tokens: 2000, total_tokens: 12000 } };
  const native: RuntimeClientSnapshot = snapshot({ transcript: { entries: [{ cursor: "4", item: { type: "message", message }, completed_response: response }], statistics }, attempt: attemptView({ last_usage: { input_tokens: 999, output_tokens: 999, total_tokens: 1998 } }) });
  const state = replaceFromSnapshot(native, "10");
  assert.match(transcriptString(state), /120 tok · 18.0s/); assert.doesNotMatch(transcriptString(state), /1,998 tok/);
  const paged = mergeTranscriptPage(state, { entries: [{ cursor: "1", item: { type: "message", message: userMessage("old", "older") } }], statistics: { ...statistics, model_requests: "1" } });
  assert.deepEqual(paged.statistics, statistics);
  assert.equal(renderFooter(state, "connected", 160), renderFooter(paged, "connected", 160));
  assert.match(transcriptString(replaceFromSnapshot(native, "20")), /120 tok · 18.0s/);
});

test("T12/T16 native application notifications reject reorder and adoption sends the inspected identity", async () => {
  const h = await harness();
  const candidate = { identity: { input_revision: "input", attempt: "4" }, expected_binding: "2", impact: "prefix_changed" as const };
  // A Session application scope is the Session identity; its authored source
  // owners are a separate native fact.
  const application = { eligibility: { status: "eligible" as const }, scope: h.session.sessionId, sources: [{ kind: "user" as const }, { kind: "workspace" as const, directory: "/workspace/A" }], version: "9007199254740993", desired: candidate.identity,
    units: { execution_policy: { status: "applied" as const }, instructions: { status: "ready" as const, impact: "prefix_changed" as const } }, candidate };
  h.session.applyNotification({ jsonrpc: "2.0", method: "configuration/changed", params: { application } });
  h.session.applyNotification({ jsonrpc: "2.0", method: "configuration/changed", params: { application: { ...application, version: "9", candidate: null } } });
  assert.deepEqual(h.session.application, application);
  const adopting = h.session.adoptConfiguration(candidate);
  const request = await nextRequest(h, "session/adoptConfiguration", 0);
  assert.deepEqual(request.params, { session_id: h.session.sessionId, candidate: candidate.identity, expected_binding: candidate.expected_binding });
  h.transport.respond(request.id, { type: "configuration_application", application: { ...application, version: "9007199254740994", candidate: null } });
  await adopting;
  assert.deepEqual(h.session.application?.candidate, candidate, "acknowledgement alone cannot clear pending state");
  const reading = h.session.readConfiguration();
  const read = await nextRequest(h, "session/configuration", 0);
  h.transport.respond(read.id, { type: "session_configuration", application: { ...application, version: "9007199254740994", candidate: null } });
  await reading;
  assert.equal(h.session.application?.candidate, null);
  assert.equal(h.transport.transportCount("session/adoptConfiguration"), 1);
  h.client.close();
});

test("settlement reads native response statistics instead of summing usage events", async () => {
  const h = await harness(snapshot({ attempt: attemptView() }));
  h.session.applyNotification({ jsonrpc: "2.0", method: "session/event", params: { target: h.target, cursor: "1", event: { type: "attempt_settled", attempt_id: "a1", outcome: { type: "completed", finish_reason: { type: "stop" } } } } });
  const read = await nextRequest(h, "session/snapshot", 0);
  const statistics = { turns: "1", steps: "1", completed_responses: "2", model_requests: "5", requests_with_usage: "4" };
  h.transport.respond(read.id, { type: "snapshot", snapshot: snapshot({ transcript: { statistics } }), cursor: "2" });
  await new Promise<void>(resolve => h.session.onState(() => resolve()));
  assert.equal(h.transport.transportCount("session/subscribe"), 0);
  assert.deepEqual(h.session.state.statistics, statistics); h.client.close();
});

test("40x15 through 160x50 keep HITL decisions actionable", async () => {
  const { HumanInteractionOverlay } = await import("../src/ui/components/hitl.ts");
  const { defaultPreferences } = await import("../src/ui/preferences.ts");
  const { approvalInteraction } = await import("./support/fixtures.ts");
  const { ComposerContext } = await import("../src/ui/components/composer-context.ts");
  const { ComposerDraft } = await import("../src/ui/composer-draft.ts");
  const h = await harness();
  for (const [columns, rows] of [[160, 50], [120, 40], [80, 24], [60, 20], [40, 15]]) {
    let decisions = 0;
    const hitl = new HumanInteractionOverlay({ onReview() {}, onDecision() { decisions++; }, onQuestionnaireSubmit() {}, onQuestionnaireDecline() {}, onDismiss() {}, onInterrupt() {}, onNavigate() {}, onToggleExpand() {} });
    const approval = approvalInteraction(); hitl.update([approval], approval.interaction, defaultPreferences());
    const frame = new PopupFrame(hitl); frame.setViewportHeight(Math.floor(rows! * .9));
    const content = frame.render(columns!);
    assert.ok(content.length <= rows!); assert.ok(content.every(row => visibleWidth(row) <= columns!));
    assert.match(plain(content.join("\n")), /Deny/); frame.handleInput("\r"); assert.equal(decisions, 1);
    const draft = new ComposerDraft(); draft.text = "中👩‍💻e\u0301".repeat(10000);
    const context = new ComposerContext(() => ({ state: h.session.state, draft }));
    assert.ok(context.render(columns!).length <= 4);
    assert.ok(context.render(columns!).every(row => visibleWidth(row) <= columns!));
  }
  h.client.close();
});

test("queue editor propagates popup focus to Pi IME cursor markers", async () => {
  const h = await harness(snapshot({ inbound: { pending: [pending("1", "0", "中文入力")] } }));
  const view = new PendingInputView(terminal(), h.session, () => {}, () => {});
  const frame = new PopupFrame(view);
  frame.focused = true; view.handleInput("\r");
  assert.ok(view.editor.render(80).join("\n").includes(CURSOR_MARKER));
  frame.focused = false;
  assert.ok(!view.editor.render(80).join("\n").includes(CURSOR_MARKER));
  h.client.close();
});

test("queued edit submits exact pre-submit whitespace and trailing newlines with observed CAS", async () => {
  for (const text of ["  hello  ", "  first\nsecond  \n", "first\n\n"]) {
    const item = pending("9007199254740994", "9007199254740995", text);
    const h = await harness(snapshot({ inbound: { pending: [item] } }));
    let applied!: () => void;
    const view = new PendingInputView(terminal(), h.session, () => {}, () => applied?.());
    view.handleInput("\r"); // open the captured observation
    assert.equal(view.editor.getExpandedText(), text);
    const completion = new Promise<void>(resolve => { applied = resolve; });
    view.handleInput("\r"); // save without changing a byte
    const request = await nextRequest(h, "inbound/edit", 0);
    assert.deepEqual(request.params, { target: h.target, expected: { sequence: item.sequence, message_id: item.message.id, revision: item.revision }, text });
    h.transport.respond(request.id, { type: "inbound_mutation", outcome: { status: "conflict" } });
    await completion;
    view.handleInput("\r");
    assert.equal(h.transport.transportCount("inbound/edit"), 1);
    assert.equal(h.transport.transportCount("session/snapshot"), 0);
    h.client.close();
  }
});
