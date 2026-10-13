/**
 * Issue #464: native automatic configuration repair as the terminal presents
 * it — Session-independent routing, one notice per receipt identity, and the
 * read-only `/settings … repairs` inspection.
 */
import assert from "node:assert/strict";
import { mkdtempSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { it } from "node:test";

import { APP_SERVER_PROTOCOL_VERSION, AppServerClient } from "../src/app-server/client.ts";
import { AppServerHost } from "../src/app-server/host.ts";
import { CommandDispatcher } from "../src/commands/dispatcher.ts";
import {
  SourceRepairNotices,
  renderRepairInspection,
  repairNotice,
  type SourceRepairObservation,
} from "../src/presentation/source-repairs.ts";
import type { ConfigRepairReceipt, SourceTarget } from "../src/protocol/app-server.ts";
import type { AppServerSession } from "../src/app-server/session.ts";
import { SourceRepairStatus } from "../src/ui/components/source-repair-status.ts";
import { NO_DIAGNOSTICS, SERVER_CAPABILITIES, harness, nextRequest } from "./support/app-server-harness.ts";
import { FakeTransport, notification, paramsOf } from "./support/app-server-peer.ts";
import { plain } from "./support/render.ts";

const WORKSPACE: SourceTarget = { kind: "workspace", directory: "/work/project" };
const AUTHORITY = "fixture-app-server-authority";

function receipt(overrides: Partial<ConfigRepairReceipt> = {}): ConfigRepairReceipt {
  return {
    id: "receipt-user-1",
    target: { kind: "user" },
    status: "repaired",
    before_revision: "sha256:before",
    after_revision: "sha256:after",
    changed_count: 3,
    changes: [
      { kind: "updated_schema_version", from: 8, to: 10 },
      { kind: "removed_field", field_path: "agent.plugins.todo.unknown_permission" },
      { kind: "removed_native_tool", name: "old_tool", reason: "unknown" },
    ],
    truncated: false,
    backup_available: true,
    diagnostic: null,
    ...overrides,
  };
}

/** The User source's observation of `value`. */
function user(value: ConfigRepairReceipt | undefined): SourceRepairObservation {
  return { target: { kind: "user" }, receipt: value };
}

/** A Workspace source's observation of `value`. */
function workspace(value: ConfigRepairReceipt | undefined, target: SourceTarget = WORKSPACE): SourceRepairObservation {
  return { target, receipt: value && { ...value, target } };
}

/** The receipt ids a status line currently presents, in source order. */
function presented(status: SourceRepairStatus): string {
  return plain(status.render(400).join(" "));
}

function projection(target: SourceTarget, user?: ConfigRepairReceipt, workspace?: ConfigRepairReceipt) {
  return {
    target, provenance: {}, process_policy_impacts: {}, absent_resource_revision: "absent", resource_revisions: {},
    user: { path: "/user", revision: "u" }, user_resource_root: "/ur", runtime_root: "/r", user_mcp: { path: "/um", revision: "um" }, agents: [],
    ...(target.kind === "workspace" ? { workspace: { path: "/w", revision: "w" }, workspace_resource_root: "/wr", workspace_mcp: { path: "/wm", revision: "wm" } } : {}),
    user_repair: user ?? null,
    workspace_repair: workspace ?? null,
  };
}

it("words repaired, combined, unresolved and uncertain receipts without values", () => {
  assert.deepEqual(repairNotice([receipt()]), {
    level: "info",
    text: "Config: Auto-repaired 3 entries in User rustx.toml. Run /settings repairs for details.",
  });
  assert.equal(
    repairNotice([receipt(), receipt({ id: "w", target: WORKSPACE, changed_count: 1 })])?.text,
    "Config: Auto-repaired 3 entries in User and 1 entry in Workspace rustx.toml. Run /settings repairs for details.",
  );
  const unresolved = repairNotice([receipt({ status: "unresolved", target: WORKSPACE, after_revision: null, backup_available: false, diagnostic: "malformed TOML at line 3, column 1" })]);
  assert.equal(unresolved?.level, "warning");
  assert.match(unresolved!.text, /Workspace rustx\.toml could not be auto-repaired: malformed TOML at line 3, column 1\. Run \/settings workspace repairs/);
  const uncertain = repairNotice([receipt({ status: "commit_uncertain" })]);
  assert.equal(uncertain?.level, "error");
  assert.match(uncertain!.text, /not durably confirmed/);
  assert.equal(repairNotice([]), undefined);

  const body = renderRepairInspection([
    { target: { kind: "user" }, receipt: receipt({ changes: [...receipt().changes, { kind: "removed_native_tool", name: "read", reason: "duplicate" }, { kind: "removed_native_tool", name: "agent_list", reason: "agent_control" }, { kind: "removed_native_tool", name: "todo", reason: "plugin_owned" }] }) },
    { target: WORKSPACE, receipt: undefined },
  ]);
  for (const line of [
    "## User rustx.toml",
    "Status: repaired",
    "Original backup: kept in private rustX storage",
    "~ schema_version 8 → 10",
    "- agent.plugins.todo.unknown_permission (unknown field)",
    '- agent.tools.builtin "old_tool" (unknown Native Tool)',
    '- agent.tools.builtin "read" (duplicate selection)',
    '- agent.tools.builtin "agent_list" (Agent control composed automatically)',
    '- agent.tools.builtin "todo" (provided by a Plugin)',
    "## Workspace rustx.toml (/work/project)",
    "No automatic repair was needed in this App Server process.",
    "runtime application is reported separately",
  ]) assert.ok(body.includes(line), line);
  const failed = renderRepairInspection([{ target: WORKSPACE, receipt: receipt({ status: "unresolved", after_revision: null, diagnostic: "invalid value" }) }]);
  assert.match(failed, /Status: unresolved — the source is unchanged/);
  assert.match(failed, /Eligible, not applied:/);
  // A control character in an authored name is reduced before it is drawn.
  assert.doesNotMatch(renderRepairInspection([{ target: { kind: "user" }, receipt: receipt({ changes: [{ kind: "removed_field", field_path: "x\u001b[2Jy" }] }) }]), /\u001b/);
});

it("never presents an uncertain publication as a confirmed application", () => {
  const heading = (status: ConfigRepairReceipt["status"]) => {
    const body = renderRepairInspection([{ target: { kind: "user" }, receipt: receipt({ status }) }]);
    return body.split("\n").filter(line => line.endsWith(":") && !line.startsWith("#"));
  };
  assert.deepEqual(heading("repaired"), ["Applied:"]);
  assert.deepEqual(heading("unresolved"), ["Eligible, not applied:"]);
  assert.deepEqual(heading("commit_uncertain"), ["Possibly published; durability not confirmed:"]);
  const uncertain = renderRepairInspection([{ target: WORKSPACE, receipt: receipt({ target: WORKSPACE, status: "commit_uncertain", diagnostic: "publication was not durably confirmed; reread the source before editing" }) }]);
  assert.doesNotMatch(uncertain, /^Applied:/m);
  assert.match(uncertain, /Status: commit uncertain — reread before editing; nothing is replayed/);
  assert.match(uncertain, /Possibly published; durability not confirmed:\n~ schema_version 8 → 10/);
});

it("announces each receipt identity once and acknowledges on input", () => {
  const notices = new SourceRepairNotices();
  assert.equal(notices.fresh(AUTHORITY, receipt()), true);
  assert.equal(notices.fresh(AUTHORITY, receipt()), false);
  assert.equal(notices.fresh(AUTHORITY, receipt({ id: "receipt-user-2" })), true);
  const status = new SourceRepairStatus();
  status.adopt(AUTHORITY);
  assert.deepEqual(status.render(120), []);
  assert.equal(status.observe(AUTHORITY, user(receipt())), true);
  assert.equal(status.observe(AUTHORITY, user(receipt())), false, "announced once");
  status.observe(AUTHORITY, { target: WORKSPACE, receipt: receipt({ id: "w", target: WORKSPACE, changed_count: 1 }) });
  assert.match(plain(status.render(200).join("\n")), /3 entries in User and 1 entry in Workspace/);
  // Bounded to two rows whatever the width.
  assert.ok(status.render(24).length <= 2);
  status.acknowledge();
  assert.equal(status.visible, false);
  assert.deepEqual(status.render(120), []);
});

it("routes a source notice sent before the host existed, with no Session", async () => {
  const transport = new FakeTransport();
  const connecting = AppServerClient.initialize({ transport });
  const initialize = (await transport.log.awaitMethod("initialize")).at(-1)!;
  transport.respond(initialize.id, {
    type: "initialized", authority_id: "fixture-app-server-authority",
    protocol_version: APP_SERVER_PROTOCOL_VERSION, capabilities: SERVER_CAPABILITIES,
  });
  // The server announces before the composition root builds the host.
  transport.notify(notification("configuration/sourceRepairChanged", { target: { kind: "user" }, receipt: receipt() }));
  const client = await connecting;
  const host = new AppServerHost({ client, ownership: "external" });
  assert.deepEqual(host.sourceRepairs(), [{ target: { kind: "user" }, receipt: receipt() }]);
  const observed: string[] = [];
  host.onSourceRepair(({ receipt: value }) => observed.push(value?.id ?? "cleared"));
  // The same receipt again is no change; a cleared observation is.
  transport.notify(notification("configuration/sourceRepairChanged", { target: { kind: "user" }, receipt: receipt() }));
  transport.notify(notification("configuration/sourceRepairChanged", { target: WORKSPACE, receipt: receipt({ id: "w", target: WORKSPACE }) }));
  transport.notify(notification("configuration/sourceRepairChanged", { target: WORKSPACE, receipt: null }));
  assert.deepEqual(observed, ["w", "cleared"]);
  assert.equal(host.attached.length, 0);
  assert.equal(transport.log.requests.length, 1, "observing notices issues no request");
});

it("deduplicates notices by negotiated App Server authority and receipt id", () => {
  const notices = new SourceRepairNotices();
  // The same authority repeating, or announcing again after a reconnect.
  assert.equal(notices.fresh("authority-a", receipt()), true);
  assert.equal(notices.fresh("authority-a", receipt()), false);
  // An equal content-derived id from another authority is its own repair.
  assert.equal(notices.fresh("authority-b", receipt()), true);
  assert.equal(notices.fresh("authority-b", receipt()), false);
  // Bounded: the oldest identity is forgotten beyond the window.
  for (let index = 0; index < SourceRepairNotices.LIMIT; index++) notices.fresh("authority-c", receipt({ id: `r${index}` }));
  assert.equal(notices.fresh("authority-a", receipt()), true, "forgotten beyond the bound");
  assert.equal(notices.fresh("authority-c", receipt({ id: `r${SourceRepairNotices.LIMIT - 1}` })), false);
});

/** One initialized connection negotiating `authority`, with its host. */
async function connect(authority: string, early?: ConfigRepairReceipt) {
  const transport = new FakeTransport();
  const connecting = AppServerClient.initialize({ transport });
  const initialize = (await transport.log.awaitMethod("initialize")).at(-1)!;
  transport.respond(initialize.id, {
    type: "initialized", authority_id: authority,
    protocol_version: APP_SERVER_PROTOCOL_VERSION, capabilities: SERVER_CAPABILITIES,
  });
  if (early) transport.notify(notification("configuration/sourceRepairChanged", { target: early.target, receipt: early }));
  const client = await connecting;
  return { transport, host: new AppServerHost({ client, ownership: "external" }) };
}

it("each connection carries the authority its initialize negotiated", async () => {
  const notices = new SourceRepairNotices();
  const announced = (host: AppServerHost) => host.sourceRepairs()
    .filter(({ receipt: value }) => value !== undefined && notices.fresh(host.authorityId, value))
    .map(({ receipt: value }) => value!.id);
  // A startup notice before the host exists is attributed to its connection.
  const first = await connect("authority-a", receipt());
  assert.equal(first.host.authorityId, "authority-a");
  assert.equal(first.host.client.authorityId, "authority-a");
  assert.deepEqual(announced(first.host), ["receipt-user-1"]);
  // Reconnecting to the same server process: nothing is announced again.
  const reconnected = await connect("authority-a", receipt());
  assert.deepEqual(announced(reconnected.host), []);
  // A replacement process at the same endpoint negotiates a new authority.
  const replaced = await connect("authority-b", receipt());
  assert.deepEqual(announced(replaced.host), ["receipt-user-1"]);
  // A withdrawn receipt is an observation without a receipt, never a notice.
  replaced.transport.notify(notification("configuration/sourceRepairChanged", { target: { kind: "user" }, receipt: null }));
  assert.deepEqual(replaced.host.sourceRepairs(), [{ target: { kind: "user" }, receipt: undefined }]);
  assert.deepEqual(announced(replaced.host), []);
});

it("refuses a connection that negotiates no authority", async () => {
  const transport = new FakeTransport();
  const connecting = AppServerClient.initialize({ transport });
  const initialize = (await transport.log.awaitMethod("initialize")).at(-1)!;
  transport.respond(initialize.id, {
    type: "initialized", authority_id: "",
    protocol_version: APP_SERVER_PROTOCOL_VERSION, capabilities: SERVER_CAPABILITIES,
  });
  await assert.rejects(connecting, /did not negotiate an authority identity/);
});

it("source notices never reach an attached Session's conversation", async () => {
  const h = await harness();
  const before = structuredClone(h.session.state);
  h.transport.notify(notification("configuration/sourceRepairChanged", { target: { kind: "user" }, receipt: receipt() }));
  assert.deepEqual(h.session.state, before);
  assert.equal(h.host.sourceRepairs()[0]?.receipt?.id, "receipt-user-1");
  for (const method of ["turn/start", "turn/steer"]) assert.equal(h.transport.transportCount(method), 0);
});

// The repair status line belongs to one App Server authority.

it("a reconnect to the same authority keeps the line and announces nothing twice", () => {
  const status = new SourceRepairStatus();
  status.adopt("authority-a");
  status.observe("authority-a", user(receipt()));
  status.observe("authority-a", workspace(receipt({ id: "w", changed_count: 1 })));
  // Reconnected: the same authority reports the same receipts again.
  status.adopt("authority-a");
  assert.equal(status.observe("authority-a", user(receipt())), false);
  assert.equal(status.observe("authority-a", workspace(receipt({ id: "w", changed_count: 1 }))), false);
  assert.match(presented(status), /3 entries in User and 1 entry in Workspace/);
  // A receipt the user already acknowledged stays acknowledged.
  status.acknowledge();
  status.adopt("authority-a");
  assert.equal(status.observe("authority-a", user(receipt())), false);
  assert.equal(status.visible, false);
  // A new receipt of that authority is still announced.
  assert.equal(status.observe("authority-a", user(receipt({ id: "receipt-user-2" }))), true);
});

it("a replacement authority retires the previous authority's line", () => {
  const status = new SourceRepairStatus();
  status.adopt("authority-a");
  status.observe("authority-a", user(receipt({ status: "unresolved", after_revision: null, diagnostic: "invalid value" })));
  status.observe("authority-a", workspace(receipt({ id: "w" })));
  assert.equal(status.visible, true);
  // The replacement process holds no receipt at all: no warning survives.
  status.adopt("authority-b");
  assert.equal(status.visible, false);
  assert.deepEqual(status.render(120), []);
  // Its receipts are its own, even with an equal content-derived id.
  assert.equal(status.observe("authority-b", user(receipt({ status: "unresolved", after_revision: null, diagnostic: "invalid value" }))), true);
  assert.match(presented(status), /User rustx\.toml could not be auto-repaired: invalid value/);
  assert.doesNotMatch(presented(status), /Workspace/);
});

it("multiple sources move across authority transitions independently", () => {
  const status = new SourceRepairStatus();
  const other: SourceTarget = { kind: "workspace", directory: "/work/other" };
  status.adopt("authority-a");
  status.observe("authority-a", user(receipt()));
  status.observe("authority-a", workspace(receipt({ id: "w1", changed_count: 1 })));
  status.adopt("authority-b");
  status.observe("authority-b", workspace(receipt({ id: "w1", changed_count: 1 })));
  status.observe("authority-b", workspace(receipt({ id: "w2", changed_count: 2 }), other));
  assert.match(presented(status), /1 entry in Workspace and 2 entries in Workspace/);
  assert.doesNotMatch(presented(status), /User/);
  // Back to the first authority (another reconnect): b's line is retired,
  // and a's receipts were already announced.
  status.adopt("authority-a");
  assert.equal(status.observe("authority-a", user(receipt())), false);
  assert.equal(status.visible, false);
});

it("a native withdrawal removes its source before and after a reconnect", () => {
  const status = new SourceRepairStatus();
  status.adopt("authority-a");
  status.observe("authority-a", user(receipt()));
  status.observe("authority-a", workspace(receipt({ id: "w", changed_count: 1 })));
  assert.equal(status.observe("authority-a", workspace(undefined)), true);
  assert.doesNotMatch(presented(status), /Workspace/);
  status.adopt("authority-a");
  assert.equal(status.observe("authority-a", user(undefined)), true);
  assert.equal(status.visible, false);
  // Withdrawing nothing changes nothing.
  assert.equal(status.observe("authority-a", user(undefined)), false);
});

it("an observation of any other authority never reaches the current line", () => {
  const status = new SourceRepairStatus();
  status.adopt("authority-a");
  status.observe("authority-a", user(receipt()));
  status.adopt("authority-b");
  // A late notice of the replaced authority: neither a receipt nor a
  // withdrawal of it changes the replacement's line.
  assert.equal(status.observe("authority-a", workspace(receipt({ id: "late" }))), false);
  status.observe("authority-b", user(receipt({ id: "b" })));
  assert.equal(status.observe("authority-a", user(undefined)), false);
  assert.match(presented(status), /3 entries in User/);
  // Nothing was announced on a's behalf after its retirement.
  status.adopt("authority-a");
  assert.equal(status.observe("authority-a", workspace(receipt({ id: "late" }))), true);
});

// Repair notifications before the first subscriber (Issue #464).

/** A connection whose server sends `early` right after `initialize`
 * answers, in the same delivery turn, before anything subscribes. */
async function connectWith(early: (transport: FakeTransport) => void) {
  const transport = new FakeTransport();
  const connecting = AppServerClient.initialize({ transport });
  const initialize = (await transport.log.awaitMethod("initialize")).at(-1)!;
  transport.respond(initialize.id, {
    type: "initialized", authority_id: AUTHORITY,
    protocol_version: APP_SERVER_PROTOCOL_VERSION, capabilities: SERVER_CAPABILITIES,
  });
  early(transport);
  return { transport, client: await connecting };
}

function repairChanged(target: SourceTarget, value: ConfigRepairReceipt | null) {
  return notification("configuration/sourceRepairChanged", { target, receipt: value && { ...value, target } });
}

/** One unrelated Session notification. */
function unrelated(index: number) {
  return notification("session/summaryInvalidated", { session_id: `ses_00000000-0000-7000-8000-${String(index).padStart(12, "0")}`, catalog_changed: false });
}

it("a repair notice after more than 64 unrelated early notices is never lost", async () => {
  const { transport, client } = await connectWith((transport) => {
    for (let index = 0; index < 80; index++) transport.notify(unrelated(index));
    transport.notify(repairChanged({ kind: "user" }, receipt()));
  });
  const host = new AppServerHost({ client, ownership: "external" });
  assert.deepEqual(host.sourceRepairs(), [user(receipt())]);
  // Presented exactly once by receipt identity after the handoff.
  const status = new SourceRepairStatus();
  status.adopt(host.authorityId);
  const announced = host.sourceRepairs().filter(observation => status.observe(host.authorityId, observation));
  assert.equal(announced.length, 1);
  transport.notify(repairChanged({ kind: "user" }, receipt()));
  assert.equal(status.observe(host.authorityId, host.sourceRepairs()[0]!), false);
  // Recovered from the buffer, never from a read.
  assert.equal(transport.log.requests.length, 1);
});

it("early notices keep the latest observation of each source, withdrawals included", async () => {
  const other: SourceTarget = { kind: "workspace", directory: "/work/other" };
  const { transport, client } = await connectWith((transport) => {
    transport.notify(repairChanged({ kind: "user" }, receipt({ id: "u1" })));
    transport.notify(repairChanged(WORKSPACE, receipt({ id: "w1" })));
    transport.notify(repairChanged(other, receipt({ id: "o1" })));
    for (let index = 0; index < 70; index++) transport.notify(unrelated(index));
    transport.notify(repairChanged({ kind: "user" }, receipt({ id: "u2" })));
    transport.notify(repairChanged(WORKSPACE, null));
  });
  const replayed: string[] = [];
  client.onNotification((message) => {
    if (message.method === "configuration/sourceRepairChanged") {
      replayed.push(`${message.params.target.kind === "user" ? "user" : message.params.target.directory}=${message.params.receipt?.id ?? "withdrawn"}`);
    }
  });
  // One per source, the latest, in the order each source last changed.
  assert.deepEqual(replayed, ["/work/other=o1", "user=u2", "/work/project=withdrawn"]);
  assert.equal(transport.log.requests.length, 1);
});

it("a notification racing the first subscription is replayed after everything older", async () => {
  const { transport, client } = await connectWith((transport) => {
    transport.notify(unrelated(1));
    transport.notify(repairChanged({ kind: "user" }, receipt({ id: "u1" })));
  });
  const seen: string[] = [];
  let raced = false;
  client.onNotification((message) => {
    if (message.method === "configuration/sourceRepairChanged") seen.push(message.params.receipt?.id ?? "withdrawn");
    else seen.push("unrelated");
    // While the buffer is replayed, the server delivers a newer observation
    // of the source still waiting in it.
    if (!raced) {
      raced = true;
      transport.notify(repairChanged({ kind: "user" }, receipt({ id: "u2" })));
    }
  });
  assert.deepEqual(seen, ["unrelated", "u2"], "the older observation is never delivered after the newer");
  // After the handoff, delivery is direct.
  transport.notify(repairChanged({ kind: "user" }, receipt({ id: "u3" })));
  assert.deepEqual(seen, ["unrelated", "u2", "u3"]);
});

it("early repair notices stay bounded to the identities the native ledger retains", async () => {
  const workspaces = Array.from({ length: 70 }, (_, index): SourceTarget => ({ kind: "workspace", directory: `/work/w${index}` }));
  const { client } = await connectWith((transport) => {
    transport.notify(repairChanged({ kind: "user" }, receipt({ id: "u" })));
    for (const [index, target] of workspaces.entries()) transport.notify(repairChanged(target, receipt({ id: `w${index}` })));
  });
  const host = new AppServerHost({ client, ownership: "external" });
  const ids = host.sourceRepairs().map(({ receipt: value }) => value!.id);
  assert.equal(ids.length, 65);
  // The User is never crowded out; the least recently observed Workspaces go.
  assert.ok(ids.includes("u"));
  assert.ok(!ids.includes("w0") && !ids.includes("w5") && ids.includes("w6") && ids.includes("w69"));
});

it("early repair notices never reach a Session or start a Turn", async () => {
  const { transport, client } = await connectWith((transport) => {
    for (let index = 0; index < 80; index++) transport.notify(unrelated(index));
    transport.notify(repairChanged({ kind: "user" }, receipt()));
  });
  const host = new AppServerHost({ client, ownership: "external" });
  assert.equal(host.sourceRepairs().length, 1);
  assert.equal(host.attached.length, 0);
  assert.deepEqual(transport.log.requests.map(request => request.method), ["initialize"]);
});

// `/settings … repairs`: the current Workspace is the focused Session's own.

const SESSION_A = "ses_84097828-fc31-78c8-9292-10df48901a85";
const SESSION_B = "ses_0198a0b1-0000-7000-8000-00000000000b";

/** A focused Session as far as the dispatcher is concerned. */
function focused(sessionId: string): AppServerSession {
  return { sessionId } as AppServerSession;
}

/** Answers the next `session/summary` of `harnessed` with `cwd`, and returns
 * the Session it asked about. */
async function answerSummary(harnessed: Awaited<ReturnType<typeof harness>>, cwd: string): Promise<string> {
  const read = await nextRequest(harnessed, "session/summary");
  const { session_id } = paramsOf(read, "session/summary");
  harnessed.transport.respond(read.id, { type: "session_summary", summary: {
    id: session_id, cwd, ownership_generation: "1", updated_at: "2026-01-01T00:00:00Z",
    active_node: "node_35971be6-e9bb-724a-8955-82fe0e42e048",
  } });
  return session_id;
}

/** Answers the next `configuration/sourcesRead` and returns its target. */
async function answerRead(harnessed: Awaited<ReturnType<typeof harness>>): Promise<SourceTarget> {
  const read = await nextRequest(harnessed, "configuration/sourcesRead");
  const { target } = paramsOf(read, "configuration/sourcesRead");
  harnessed.transport.respond(read.id, { type: "source_settings", projection: projection(target, receipt(), target.kind === "workspace" ? receipt({ id: "w", target, status: "unresolved", after_revision: null, diagnostic: "invalid value" }) : undefined) });
  return target;
}

/** Inspection only: the connection carried reads and nothing else. */
function assertReadOnly(harnessed: Awaited<ReturnType<typeof harness>>) {
  const methods = new Set(harnessed.transport.log.requests.map(request => request.method));
  for (const method of methods) assert.ok(["initialize", "session/attach", "session/summary", "configuration/sourcesRead"].includes(method), method);
}

function body(outcome: Awaited<ReturnType<CommandDispatcher["submit"]>>): string {
  assert.equal(outcome.kind, "inspect");
  return outcome.kind === "inspect" ? outcome.body : "";
}

it("the current Workspace is the focused Session's native cwd, never the launch directory", async () => {
  const h = await harness();
  // Launched in Workspace A; the focused (resumed) Session belongs to B, a
  // path the App Server has and this machine does not.
  const launch = realpathSync(mkdtempSync(join(tmpdir(), "rustx-tui-launch-")));
  const dispatcher = new CommandDispatcher({ host: h.host, session: focused(SESSION_A), sessionSettings: { cwd: launch }, diagnostics: NO_DIAGNOSTICS });
  const remote = "/srv/app-server-only/workspace-b";
  for (const [command, sections] of [
    ["/settings repairs", ["## User rustx.toml", `## Workspace rustx.toml (${remote})`]],
    ["/settings workspace repairs", [`## Workspace rustx.toml (${remote})`]],
  ] as const) {
    const operation = dispatcher.submit(command);
    assert.equal(await answerSummary(h, remote), SESSION_A, command);
    assert.deepEqual(await answerRead(h), { kind: "workspace", directory: remote }, command);
    const text = body(await operation);
    for (const section of sections) assert.ok(text.includes(section), `${command}: ${section}`);
    assert.ok(!text.includes(launch), command);
    assert.equal(text.includes("## User rustx.toml"), sections.length === 2, command);
  }
  assertReadOnly(h);
});

it("switching Sessions switches the current Workspace", async () => {
  const h = await harness();
  const dispatcher = new CommandDispatcher({ host: h.host, session: focused(SESSION_A), sessionSettings: { cwd: "/launch" }, diagnostics: NO_DIAGNOSTICS });
  for (const [session, cwd] of [[SESSION_A, "/srv/a"], [SESSION_B, "/srv/b"], [SESSION_A, "/srv/a"]] as const) {
    dispatcher.setSession(focused(session));
    const operation = dispatcher.submit("/settings workspace repairs");
    assert.equal(await answerSummary(h, cwd), session);
    assert.deepEqual(await answerRead(h), { kind: "workspace", directory: cwd });
    assert.ok(body(await operation).includes(`## Workspace rustx.toml (${cwd})`));
  }
  assertReadOnly(h);
});

it("without a focused Session only an authoritative or named Workspace is inspected", async () => {
  const h = await harness();
  const dispatcher = new CommandDispatcher({ host: h.host, session: undefined, sessionSettings: { cwd: "/launch" }, diagnostics: NO_DIAGNOSTICS });
  // The User alone, saying why no Workspace is shown.
  const operation = dispatcher.submit("/settings repairs");
  assert.deepEqual(await answerRead(h), { kind: "user" });
  const text = body(await operation);
  assert.ok(text.includes("## User rustx.toml"));
  assert.ok(!text.includes("## Workspace rustx.toml"));
  assert.ok(text.includes("No Session is focused, so there is no current Workspace."));
  // The implicit Workspace form reports it instead of guessing.
  assert.deepEqual(await dispatcher.submit("/settings workspace repairs"), {
    kind: "transient", level: "error",
    text: 'No Session is focused, so there is no current Workspace. Name one with /settings workspace "<path>" repairs.',
  });
  // The User form and an explicitly named Workspace need no Session.
  const userOnly = dispatcher.submit("/settings user repairs");
  assert.deepEqual(await answerRead(h), { kind: "user" });
  assert.ok(!body(await userOnly).includes("No Session is focused"));
  const named = dispatcher.submit('/settings workspace "/srv/named" repairs');
  assert.deepEqual(await answerRead(h), { kind: "workspace", directory: "/srv/named" });
  assert.ok(body(await named).includes("## Workspace rustx.toml (/srv/named)"));
  assert.equal(h.transport.transportCount("session/summary"), 0);
  assertReadOnly(h);
});

it("an explicitly named Workspace is inspected exactly, whatever is focused", async () => {
  const h = await harness();
  const dispatcher = new CommandDispatcher({ host: h.host, session: focused(SESSION_A), sessionSettings: { cwd: "/launch" }, diagnostics: NO_DIAGNOSTICS });
  const operation = dispatcher.submit('/settings workspace "/srv/explicit path" repairs');
  assert.deepEqual(await answerRead(h), { kind: "workspace", directory: "/srv/explicit path" });
  assert.ok(body(await operation).includes("## Workspace rustx.toml (/srv/explicit path)"));
  assert.equal(h.transport.transportCount("session/summary"), 0);
  for (const invalid of ["/settings repairs extra", "/settings user repairs now", "/settings nowhere repairs"]) {
    assert.equal((await dispatcher.submit(invalid)).kind, "transient", invalid);
  }
  assertReadOnly(h);
});

it("an in-flight inspection keeps its Session and never presents a replaced App Server", async () => {
  const h = await harness();
  const dispatcher = new CommandDispatcher({ host: h.host, session: focused(SESSION_A), sessionSettings: { cwd: "/launch" }, diagnostics: NO_DIAGNOSTICS });
  // Focus moves to B while A's Workspace is being resolved: A's command
  // still inspects A's Workspace.
  const operation = dispatcher.submit("/settings workspace repairs");
  const summary = await nextRequest(h, "session/summary");
  dispatcher.setSession(focused(SESSION_B));
  assert.equal(paramsOf(summary, "session/summary").session_id, SESSION_A);
  h.transport.respond(summary.id, { type: "session_summary", summary: {
    id: SESSION_A, cwd: "/srv/a", ownership_generation: "1", updated_at: "2026-01-01T00:00:00Z",
    active_node: "node_35971be6-e9bb-724a-8955-82fe0e42e048",
  } });
  assert.deepEqual(await answerRead(h), { kind: "workspace", directory: "/srv/a" });
  assert.ok(body(await operation).includes("## Workspace rustx.toml (/srv/a)"));
  assert.equal(h.transport.transportCount("session/summary"), 1);
  // The App Server is replaced while a command resolves: its result is
  // fenced, and the replacement is never asked on its behalf.
  dispatcher.setSession(focused(SESSION_A));
  const replaced = dispatcher.submit("/settings repairs");
  const pending = await nextRequest(h, "session/summary");
  const replacement = await harness();
  dispatcher.setHost(replacement.host);
  h.transport.respond(pending.id, { type: "session_summary", summary: {
    id: SESSION_A, cwd: "/srv/a", ownership_generation: "1", updated_at: "2026-01-01T00:00:00Z",
    active_node: "node_35971be6-e9bb-724a-8955-82fe0e42e048",
  } });
  await answerRead(h);
  assert.deepEqual(await replaced, {
    kind: "transient", level: "error",
    text: "The App Server connection changed during the inspection; nothing was shown. Run it again.",
  });
  assert.equal(replacement.transport.transportCount("session/summary"), 0);
  assert.equal(replacement.transport.transportCount("configuration/sourcesRead"), 0);
  assertReadOnly(h);
  assertReadOnly(replacement);
});
