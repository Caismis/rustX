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
} from "../src/presentation/source-repairs.ts";
import type { ConfigRepairReceipt, SourceTarget } from "../src/protocol/app-server.ts";
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
  assert.deepEqual(status.render(120), []);
  status.observe(receipt());
  status.observe(receipt({ id: "w", target: WORKSPACE, changed_count: 1 }));
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

it("/settings repairs commands read sources authoritatively without a Session", async () => {
  const h = await harness();
  const workspace = realpathSync(mkdtempSync(join(tmpdir(), "rustx-tui-repairs-")));
  const dispatcher = new CommandDispatcher({
    host: h.host, session: undefined, sessionSettings: { cwd: workspace }, diagnostics: NO_DIAGNOSTICS,
  });
  const current: SourceTarget = { kind: "workspace", directory: workspace };
  for (const [command, target, sections] of [
    ["/settings repairs", current, ["## User rustx.toml", "## Workspace rustx.toml"]],
    ["/settings user repairs", { kind: "user" }, ["## User rustx.toml"]],
    ["/settings workspace repairs", current, ["## Workspace rustx.toml"]],
    [`/settings workspace "${workspace}" repairs`, current, ["## Workspace rustx.toml"]],
  ] as const) {
    const operation = dispatcher.submit(command);
    const read = await nextRequest(h, "configuration/sourcesRead");
    assert.deepEqual(paramsOf(read, "configuration/sourcesRead").target, target, command);
    h.transport.respond(read.id, { type: "source_settings", projection: projection(target, receipt(), receipt({ id: "w", target: current, status: "unresolved", after_revision: null, diagnostic: "malformed TOML at line 1, column 2" })) });
    const outcome = await operation;
    assert.equal(outcome.kind, "inspect", command);
    if (outcome.kind !== "inspect") continue;
    assert.equal(outcome.title, "Configuration repairs");
    for (const section of sections) assert.ok(outcome.body.includes(section), `${command}: ${section}`);
    if (!(sections as readonly string[]).includes("## User rustx.toml")) assert.ok(!outcome.body.includes("## User rustx.toml"), command);
  }
  // Reading is never a mutation and never a reconciliation.
  assert.equal(h.transport.transportCount("configuration/sourceWrite"), 0);
  assert.equal(h.transport.transportCount("configuration/reconcile"), 0);
  for (const invalid of ["/settings repairs extra", "/settings user repairs now", "/settings nowhere repairs"]) {
    const outcome = await dispatcher.submit(invalid);
    assert.equal(outcome.kind, "transient", invalid);
  }
});

it("/settings workspace repairs needs a resolvable current Workspace", async () => {
  const h = await harness();
  const outcome = await h.dispatcher.submit("/settings workspace repairs");
  assert.deepEqual(outcome, { kind: "transient", level: "error", text: "The current Workspace directory is unavailable; name it explicitly." });
  assert.equal(h.transport.transportCount("configuration/sourcesRead"), 0);
});
