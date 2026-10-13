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
  SourceRepairAnnouncements,
  renderRepairInspection,
  retainSource,
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

/** The User source's observation of `value` at ledger `sequence`. */
function user(value: ConfigRepairReceipt | undefined, sequence = "1"): SourceRepairObservation {
  return { target: { kind: "user" }, receipt: value, sequence };
}

/** A Workspace source's observation of `value` at ledger `sequence`. */
function workspace(value: ConfigRepairReceipt | undefined, target: SourceTarget = WORKSPACE, sequence = "1"): SourceRepairObservation {
  return { target, receipt: value && { ...value, target }, sequence };
}

/** The receipt ids a status line currently presents, in source order. */
function presented(status: SourceRepairStatus): string {
  return plain(status.render(400).join(" "));
}

function projection(target: SourceTarget, user?: ConfigRepairReceipt, workspace?: ConfigRepairReceipt, sequence = "1") {
  return {
    target, provenance: {}, process_policy_impacts: {}, absent_resource_revision: "absent", resource_revisions: {},
    user: { path: "/user", revision: "u" }, user_resource_root: "/ur", runtime_root: "/r", user_mcp: { path: "/um", revision: "um" }, agents: [],
    ...(target.kind === "workspace" ? { workspace: { path: "/w", revision: "w" }, workspace_resource_root: "/wr", workspace_mcp: { path: "/wm", revision: "wm" } } : {}),
    user_repair: user ?? null,
    workspace_repair: workspace ?? null,
    repair_sequence: sequence,
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

it("presents each pending receipt once and acknowledges on input", () => {
  const status = new SourceRepairStatus();
  status.adopt(AUTHORITY);
  assert.deepEqual(status.render(120), []);
  assert.equal(status.observe(AUTHORITY, user(receipt())), true);
  assert.equal(status.observe(AUTHORITY, user(receipt())), false, "announced once");
  status.observe(AUTHORITY, workspace(receipt({ id: "w", changed_count: 1 })));
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
  transport.notify(notification("configuration/sourceRepairChanged", { target: { kind: "user" }, receipt: receipt(), sequence: "1" }));
  const client = await connecting;
  const host = new AppServerHost({ client, ownership: "external" });
  assert.deepEqual(host.sourceRepairs(), [user(receipt())]);
  const observed: string[] = [];
  host.onSourceRepair(({ receipt: value }) => observed.push(value?.id ?? "cleared"));
  // The same receipt at the same sequence is no change; a cleared
  // observation is.
  transport.notify(notification("configuration/sourceRepairChanged", { target: { kind: "user" }, receipt: receipt(), sequence: "1" }));
  transport.notify(notification("configuration/sourceRepairChanged", { target: WORKSPACE, receipt: receipt({ id: "w", target: WORKSPACE }), sequence: "2" }));
  transport.notify(notification("configuration/sourceRepairChanged", { target: WORKSPACE, receipt: null, sequence: "3" }));
  assert.deepEqual(observed, ["w", "cleared"]);
  assert.equal(host.attached.length, 0);
  assert.equal(transport.log.requests.length, 1, "observing notices issues no request");
});

it("announcements are kept once per negotiated App Server authority and receipt id", () => {
  const announced = new SourceRepairAnnouncements();
  assert.equal(announced.announce("authority-a", receipt()), true);
  assert.equal(announced.announce("authority-a", receipt()), false);
  // An equal content-derived id from another authority is its own repair.
  assert.equal(announced.announce("authority-b", receipt()), true);
  // Bounded: the oldest identity is forgotten beyond the window.
  for (let index = 0; index < SourceRepairAnnouncements.LIMIT; index++) announced.announce("authority-c", receipt({ id: `r${index}` }));
  assert.equal(announced.announce("authority-a", receipt()), true, "forgotten beyond the bound");
  assert.equal(announced.announce("authority-c", receipt({ id: `r${SourceRepairAnnouncements.LIMIT - 1}` })), false);
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
  if (early) transport.notify(notification("configuration/sourceRepairChanged", { target: early.target, receipt: early, sequence: "1" }));
  const client = await connecting;
  return { transport, host: new AppServerHost({ client, ownership: "external" }) };
}

it("each connection carries the authority its initialize negotiated", async () => {
  const status = new SourceRepairStatus();
  const announced = (host: AppServerHost) => {
    status.adopt(host.authorityId);
    const ids = host.sourceRepairs()
      .filter(observation => status.observe(host.authorityId, observation) && observation.receipt !== undefined)
      .map(({ receipt: value }) => value!.id);
    status.acknowledge();
    return ids;
  };
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
  replaced.transport.notify(notification("configuration/sourceRepairChanged", { target: { kind: "user" }, receipt: null, sequence: "2" }));
  assert.deepEqual(replaced.host.sourceRepairs(), [user(undefined, "2")]);
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
  h.transport.notify(notification("configuration/sourceRepairChanged", { target: { kind: "user" }, receipt: receipt(), sequence: "1" }));
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
  // Back to the first authority after b: b's line is retired, and a's
  // receipts were already announced.
  assert.equal(status.adopt("authority-a"), false, "not kept: b came between");
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

// Same-authority reconciliation: native observations are ordered by their
// ledger sequence, and acknowledgement never fences native authority.

it("an announced receipt identity never fences a newer observation", () => {
  const status = new SourceRepairStatus();
  status.adopt(AUTHORITY);
  // R0 was announced and acknowledged; R1 replaced it and is pending.
  status.observe(AUTHORITY, workspace(receipt({ id: "r0", status: "unresolved", diagnostic: "invalid value" }), WORKSPACE, "2"));
  status.acknowledge();
  status.observe(AUTHORITY, workspace(receipt({ id: "r1", status: "unresolved", diagnostic: "other value" }), WORKSPACE, "4"));
  assert.match(presented(status), /other value/);
  // Native returns to R0's content: the obsolete R1 entry goes, and the
  // already announced R0 is current without being announced again.
  assert.equal(status.observe(AUTHORITY, workspace(receipt({ id: "r0", status: "unresolved", diagnostic: "invalid value" }), WORKSPACE, "6")), true);
  assert.equal(status.visible, false);
  assert.deepEqual(status.pendingSources(), []);
});

it("an older observation of a source changes nothing, a newer one always applies", () => {
  const status = new SourceRepairStatus();
  status.adopt(AUTHORITY);
  status.observe(AUTHORITY, workspace(receipt({ id: "r2" }), WORKSPACE, "6"));
  // Older: a read answered after the newer notice, or a late notice.
  assert.equal(status.observe(AUTHORITY, workspace(receipt({ id: "r1" }), WORKSPACE, "5")), false);
  assert.equal(status.observe(AUTHORITY, workspace(undefined, WORKSPACE, "5")), false);
  assert.equal(status.pendingSources().length, 1);
  assert.match(presented(status), /3 entries in Workspace/);
  // Equal or newer applies; sequences of other sources are independent.
  assert.equal(status.observe(AUTHORITY, user(receipt({ id: "u" }), "1")), true);
  assert.equal(status.observe(AUTHORITY, workspace(undefined, WORKSPACE, "7")), true);
  assert.deepEqual(status.pendingSources(), [{ kind: "user" }]);
  // Exact decimal sequences beyond the safe integer range still order.
  status.observe(AUTHORITY, user(receipt({ id: "u2" }), "9007199254740993"));
  assert.equal(status.observe(AUTHORITY, user(undefined, "9007199254740992")), false);
});

it("BOUND-02/BOUND-03 the line keeps finitely many sources and never evicts the User", () => {
  const status = new SourceRepairStatus();
  status.adopt(AUTHORITY);
  status.observe(AUTHORITY, user(receipt({ id: "u", status: "unresolved", diagnostic: "user value" }), "1"));
  for (let index = 0; index < 70; index++) {
    status.observe(AUTHORITY, workspace(receipt({ id: `w${index}` }), { kind: "workspace", directory: `/work/w${index}` }, String(index + 2)));
  }
  const pending = status.pendingSources();
  assert.equal(pending.length, 65);
  assert.deepEqual(pending[0], { kind: "user" });
  // The least recently observed Workspaces are gone, with their entries.
  assert.ok(!pending.some(target => target.kind === "workspace" && ["/work/w0", "/work/w5"].includes(target.directory)));
  assert.ok(pending.some(target => target.kind === "workspace" && target.directory === "/work/w69"));
});

/** A host whose next `configuration/sourcesRead` is answered by the test. */
async function heldRead(target: SourceTarget) {
  const { transport, host } = await connect(AUTHORITY);
  const status = new SourceRepairStatus();
  status.adopt(host.authorityId);
  host.onSourceRepair(observation => status.observe(host.authorityId, observation));
  const reading = host.readSourceRepairs(target);
  const read = (await transport.log.awaitMethod("configuration/sourcesRead")).at(-1)!;
  return { transport, host, status, reading, read };
}

it("RACE-01 a newer notice wins over an older read answered after it", async () => {
  const { transport, host, status, reading, read } = await heldRead(WORKSPACE);
  // The read observed R1 at sequence 5; before its answer arrives, native
  // moved to R2 at 6 and its notice arrived first.
  transport.notify(repairChanged(WORKSPACE, receipt({ id: "r2", changed_count: 2 }), "6"));
  transport.respond(read.id, { type: "source_settings", projection: projection(WORKSPACE, undefined, receipt({ id: "r1", target: WORKSPACE }), "5") });
  await reading;
  assert.deepEqual(host.sourceRepairs().find(observation => observation.target.kind === "workspace")?.receipt?.id, "r2");
  assert.match(presented(status), /2 entries in Workspace/);
});

it("RACE-02 a late notice never resurrects a source a fresher read withdrew", async () => {
  const { transport, host, status, reading, read } = await heldRead(WORKSPACE);
  // The read observed the withdrawal at sequence 7; a notice of R1 produced
  // at 5 arrives only afterwards.
  transport.respond(read.id, { type: "source_settings", projection: projection(WORKSPACE, undefined, undefined, "7") });
  await reading;
  transport.notify(repairChanged(WORKSPACE, receipt({ id: "r1", status: "unresolved", diagnostic: "invalid value" }), "5"));
  assert.equal(host.sourceRepairs().find(observation => observation.target.kind === "workspace")?.receipt, undefined);
  assert.equal(status.visible, false);
});

it("REC-09 a reconcile read is one read-only request and reaches no Session", async () => {
  const { transport, reading, read } = await heldRead(WORKSPACE);
  transport.respond(read.id, { type: "source_settings", projection: projection(WORKSPACE, undefined, undefined, "3") });
  await reading;
  assert.deepEqual(transport.log.requests.map(request => request.method), ["initialize", "configuration/sourcesRead"]);
});

it("RACE-03 a read and a notice at one native sequence are one observation", async () => {
  const { transport, host, status, reading, read } = await heldRead(WORKSPACE);
  const observed: string[] = [];
  host.onSourceRepair(({ receipt: value }) => observed.push(value?.id ?? "withdrawn"));
  const announced: boolean[] = [];
  const announce = status.observe.bind(status);
  // The notice and the read both carry R2 at sequence 6, in either order.
  transport.notify(repairChanged(WORKSPACE, receipt({ id: "r2" }), "6"));
  transport.respond(read.id, { type: "source_settings", projection: projection(WORKSPACE, undefined, receipt({ id: "r2", target: WORKSPACE }), "6") });
  await reading;
  announced.push(announce(host.authorityId, workspace(receipt({ id: "r2" }), WORKSPACE, "6")));
  assert.deepEqual(observed.filter(id => id === "r2"), ["r2"], "the host applies it once");
  assert.deepEqual(announced, [false], "and the line announced it once");
  assert.equal(status.pendingSources().length, 1);
});

it("RACE-04 an identity cycle follows native order, not receipt identity", async () => {
  const { transport, host, status, reading, read } = await heldRead({ kind: "user" });
  transport.respond(read.id, { type: "source_settings", projection: projection({ kind: "user" }, receipt({ id: "r1" }), undefined, "1") });
  await reading;
  assert.match(presented(status), /3 entries in User/);
  transport.notify(repairChanged({ kind: "user" }, receipt({ id: "r2", changed_count: 2 }), "2"));
  assert.match(presented(status), /2 entries in User/);
  transport.notify(repairChanged({ kind: "user" }, receipt({ id: "r1" }), "3"));
  // R1 is current again: R2 is retired, and R1, announced before, is not
  // announced a second time.
  assert.equal(host.sourceRepairs()[0]?.receipt?.id, "r1");
  assert.equal(host.sourceRepairs()[0]?.sequence, "3");
  assert.equal(status.visible, false);
  // A stale R2 notice afterwards changes nothing.
  transport.notify(repairChanged({ kind: "user" }, receipt({ id: "r2", changed_count: 2 }), "2"));
  assert.equal(host.sourceRepairs()[0]?.receipt?.id, "r1");
  assert.equal(status.visible, false);
});

it("RACE-05 an empty read of a Workspace the ledger forgot outranks its earlier notice", async () => {
  const { transport, host, status, reading, read } = await heldRead(WORKSPACE);
  // The ledger forgot W after 64 newer Workspaces: the read finds no entry,
  // at the ledger frontier, which is later than any change W ever had.
  transport.respond(read.id, { type: "source_settings", projection: projection(WORKSPACE, undefined, undefined, "70") });
  await reading;
  transport.notify(repairChanged(WORKSPACE, receipt({ id: "w", status: "unresolved", diagnostic: "invalid value" }), "5"));
  assert.equal(host.sourceRepairs().find(observation => observation.target.kind === "workspace")?.receipt, undefined);
  assert.equal(status.visible, false);
  // A genuinely newer observation of W after the read still applies.
  transport.notify(repairChanged(WORKSPACE, receipt({ id: "w2", status: "unresolved", diagnostic: "invalid value" }), "71"));
  assert.equal(status.visible, true);
});

it("BOUND-01 a connection keeps finitely many source observations, empty ones included", async () => {
  const { transport, host } = await connect(AUTHORITY);
  transport.notify(repairChanged({ kind: "user" }, receipt({ id: "u" }), "1"));
  // 70 distinct Workspace reads, most of them without any receipt.
  for (let index = 0; index < 70; index++) {
    const target: SourceTarget = { kind: "workspace", directory: `/work/w${index}` };
    const reading = host.readSourceRepairs(target);
    const read = (await transport.log.awaitMethod("configuration/sourcesRead", index + 1)).at(-1)!;
    transport.respond(read.id, { type: "source_settings", projection: projection(target, receipt({ id: "u" }), index % 7 === 0 ? receipt({ id: `w${index}`, target }) : undefined, String(index + 2)) });
    await reading;
  }
  const kept = host.sourceRepairs();
  assert.equal(kept.length, 65);
  assert.ok(kept.some(({ target }) => target.kind === "user"), "the User is never evicted by Workspace churn");
  assert.ok(!kept.some(({ target }) => target.kind === "workspace" && ["/work/w0", "/work/w4"].includes(target.directory)));
  assert.ok(kept.some(({ target }) => target.kind === "workspace" && target.directory === "/work/w69"));
});

it("BOUND-04 evicting a Workspace retires its warning, and its newer observation returns", () => {
  const status = new SourceRepairStatus();
  status.adopt(AUTHORITY);
  const first: SourceTarget = { kind: "workspace", directory: "/work/first" };
  status.observe(AUTHORITY, workspace(receipt({ id: "first", status: "unresolved", diagnostic: "first value" }), first, "1"));
  assert.match(presented(status), /first value/);
  // 65 newer Workspaces without receipts push it out of the line's 65
  // sources.
  let changed = false;
  for (let index = 0; index < 65; index++) {
    changed = status.observe(AUTHORITY, workspace(undefined, { kind: "workspace", directory: `/work/w${index}` }, String(index + 2))) || changed;
  }
  assert.equal(changed, true, "the eviction changed the line");
  assert.equal(status.visible, false);
  assert.deepEqual(status.pendingSources(), []);
  // Re-observed later with a newer receipt, it is a source again.
  assert.equal(status.observe(AUTHORITY, workspace(receipt({ id: "first-2", status: "unresolved", diagnostic: "newer value" }), first, "90")), true);
  assert.match(presented(status), /newer value/);
});

// Eviction keeps native order (Issue #464): a source a connection forgot is
// never resurrected by older history, and is recovered by an authoritative
// read when a notice of it cannot be ordered.

/** Notices withdrawing `count` other Workspaces from `first` on: enough
 * distinct sources to push the least recently observed ones out. */
function churn(transport: FakeTransport, first: number, count = 64) {
  for (let index = 0; index < count; index++) {
    transport.notify(repairChanged({ kind: "workspace", directory: `/work/churn${first + index}` }, null, String(first + index)));
  }
}

/** What `host` holds of `target`, if it holds the source at all. */
function held(host: AppServerHost, target: SourceTarget) {
  return host.sourceRepairs().find(observation => observation.target.kind === target.kind
    && (target.kind === "user" || (observation.target.kind === "workspace" && observation.target.directory === target.directory)));
}

/** A connection whose observations reach a status line. */
async function watched() {
  const { transport, host } = await connect(AUTHORITY);
  const status = new SourceRepairStatus();
  status.adopt(host.authorityId);
  host.onSourceRepair(observation => status.observe(host.authorityId, observation));
  return { transport, host, status };
}

/** Lets settled responses and the host's recovery continue. */
async function settle() {
  for (let turn = 0; turn < 4; turn++) await new Promise(resolve => setImmediate(resolve));
}

/** Answers `request` as `configuration/sourcesRead` of `target` at `sequence`. */
function answer(transport: FakeTransport, request: { id: string | number }, target: SourceTarget, value: ConfigRepairReceipt | undefined, sequence: string) {
  transport.respond(request.id, { type: "source_settings", projection: projection(target, undefined, value && { ...value, target }, sequence) });
}

const UNRESOLVED_R1 = receipt({ id: "r1", status: "unresolved", diagnostic: "obsolete value" });

it("EVICT-01 a read held across a newer withdrawal and the source's eviction never resurrects it", async () => {
  const { transport, host, status } = await watched();
  const observed: string[] = [];
  host.onSourceRepair(({ target, receipt: value }) => { if (target.kind === "workspace" && target.directory === WORKSPACE.directory) observed.push(value?.id ?? "withdrawn"); });
  // Read A of W captures R1 at 5; its answer is held.
  const readingA = host.readSourceRepairs(WORKSPACE);
  const [readA] = await transport.log.awaitMethod("configuration/sourcesRead");
  // Native withdraws R1 at 6, and read B delivers that withdrawal.
  const readingB = host.readSourceRepairs(WORKSPACE);
  const readB = (await transport.log.awaitMethod("configuration/sourcesRead", 2)).at(-1)!;
  answer(transport, readB, WORKSPACE, undefined, "6");
  await readingB;
  assert.equal(held(host, WORKSPACE)?.sequence, "6");
  // 64 other Workspaces evict W.
  churn(transport, 7);
  assert.equal(held(host, WORKSPACE), undefined, "W was forgotten");
  answer(transport, readA!, WORKSPACE, UNRESOLVED_R1, "5");
  const snapshot = await readingA;
  assert.equal(snapshot.workspace_repair?.id, "r1", "the caller still gets the snapshot it read");
  assert.equal(held(host, WORKSPACE), undefined);
  assert.deepEqual(observed, ["withdrawn"]);
  assert.equal(status.visible, false);
  assert.doesNotMatch(presented(status), /obsolete value/);
});

it("EVICT-02 a delayed notice of an evicted source is ambiguous: never applied, recovered by a read", async () => {
  const { transport, host } = await connect(AUTHORITY);
  const status = new SourceRepairStatus();
  status.adopt(host.authorityId);
  const observed: string[] = [];
  host.onSourceRepair(observation => {
    status.observe(host.authorityId, observation);
    if (observation.target.kind === "workspace" && observation.target.directory === WORKSPACE.directory) observed.push(observation.receipt?.id ?? "withdrawn");
  });
  transport.notify(repairChanged(WORKSPACE, UNRESOLVED_R1, "5"));
  transport.notify(repairChanged(WORKSPACE, null, "6"));
  churn(transport, 7);
  assert.equal(held(host, WORKSPACE), undefined);
  // R1's notice, produced at 5, arrives late.
  transport.notify(repairChanged(WORKSPACE, UNRESOLVED_R1, "5"));
  assert.equal(held(host, WORKSPACE), undefined, "not applied");
  assert.equal(status.visible, false);
  const [recovery] = await transport.log.awaitMethod("configuration/sourcesRead");
  assert.deepEqual(paramsOf(recovery!, "configuration/sourcesRead"), { target: WORKSPACE });
  answer(transport, recovery!, WORKSPACE, undefined, "70");
  await settle();
  assert.deepEqual(held(host, WORKSPACE), { target: WORKSPACE, receipt: undefined, sequence: "70" });
  assert.deepEqual(observed, ["r1", "withdrawn", "withdrawn"]);
  assert.equal(status.visible, false);
  assert.equal(transport.log.count("configuration/sourcesRead"), 1, "one recovery read");
});

it("EVICT-03 a fresh read re-observes an evicted source: its current receipt or its current absence", async () => {
  const { transport, host, status } = await watched();
  transport.notify(repairChanged(WORKSPACE, UNRESOLVED_R1, "5"));
  transport.notify(repairChanged(WORKSPACE, null, "6"));
  churn(transport, 7);
  // Current absence: the read is newer than everything forgotten.
  let reading = host.readSourceRepairs(WORKSPACE);
  let read = (await transport.log.awaitMethod("configuration/sourcesRead", 1)).at(-1)!;
  answer(transport, read, WORKSPACE, undefined, "71");
  await reading;
  assert.deepEqual(held(host, WORKSPACE), { target: WORKSPACE, receipt: undefined, sequence: "71" });
  // Evicted again; then a current receipt R3.
  churn(transport, 72);
  assert.equal(held(host, WORKSPACE), undefined);
  reading = host.readSourceRepairs(WORKSPACE);
  read = (await transport.log.awaitMethod("configuration/sourcesRead", 2)).at(-1)!;
  answer(transport, read, WORKSPACE, receipt({ id: "r3", status: "unresolved", diagnostic: "current value" }), "140");
  await reading;
  assert.equal(held(host, WORKSPACE)?.receipt?.id, "r3");
  assert.match(presented(status), /current value/);
  assert.doesNotMatch(presented(status), /obsolete value/);
});

it("EVICT-04 a newer native receipt after eviction applies at once, announced once, with no recovery", async () => {
  const { transport, host } = await connect(AUTHORITY);
  const status = new SourceRepairStatus();
  status.adopt(host.authorityId);
  const announced: string[] = [];
  host.onSourceRepair(observation => { if (status.observe(host.authorityId, observation) && observation.receipt) announced.push(observation.receipt.id); });
  transport.notify(repairChanged(WORKSPACE, UNRESOLVED_R1, "5"));
  transport.notify(repairChanged(WORKSPACE, null, "6"));
  churn(transport, 7);
  const r2 = receipt({ id: "r2", status: "unresolved", diagnostic: "newer value" });
  transport.notify(repairChanged(WORKSPACE, r2, "71"));
  transport.notify(repairChanged(WORKSPACE, r2, "71"));
  assert.equal(held(host, WORKSPACE)?.receipt?.id, "r2");
  assert.deepEqual(announced, ["r1", "r2"]);
  assert.match(presented(status), /newer value/);
  await settle();
  assert.equal(transport.log.count("configuration/sourcesRead"), 0);
});

it("EVICT-05 an unfamiliar source older than the frontier is recovered, never discarded", async () => {
  const { transport, host } = await connect(AUTHORITY);
  const status = new SourceRepairStatus();
  status.adopt(host.authorityId);
  host.onSourceRepair(observation => status.observe(host.authorityId, observation));
  churn(transport, 10, 65);
  const fresh: SourceTarget = { kind: "workspace", directory: "/work/never-seen" };
  const legitimate = receipt({ id: "legit", status: "unresolved", diagnostic: "legitimate value" });
  // A connection newly interested in a source is told its latest receipt at
  // the sequence it was recorded at, long before the frontier.
  transport.notify(repairChanged(fresh, legitimate, "3"));
  assert.equal(held(host, fresh), undefined);
  const [recovery] = await transport.log.awaitMethod("configuration/sourcesRead");
  // Noticed again while its recovery read is in flight: read once more after.
  transport.notify(repairChanged(fresh, legitimate, "3"));
  transport.notify(repairChanged(fresh, legitimate, "3"));
  answer(transport, recovery!, fresh, legitimate, "80");
  await settle();
  assert.equal(held(host, fresh)?.receipt?.id, "legit");
  assert.match(presented(status), /legitimate value/);
  const [, again] = await transport.log.awaitMethod("configuration/sourcesRead", 2);
  answer(transport, again!, fresh, legitimate, "80");
  await settle();
  assert.equal(transport.log.count("configuration/sourcesRead"), 2, "one queued reread, however many repeats");
  assert.equal(held(host, fresh)?.sequence, "80");
});

it("EVICT-06 hundreds of sources keep observations, ordering state and recovery finite", async () => {
  const { transport, host } = await connect(AUTHORITY);
  churn(transport, 1000, 400);
  assert.equal(host.sourceRepairs().length, 64);
  // 300 ambiguous notices of distinct unfamiliar Workspaces.
  for (let index = 0; index < 300; index++) {
    transport.notify(repairChanged({ kind: "workspace", directory: `/work/late${index}` }, receipt({ id: `late${index}` }), "2"));
  }
  // One read in flight; at most 64 queued behind it.
  for (let reads = 1; ; reads++) {
    const request = (await transport.log.awaitMethod("configuration/sourcesRead", reads)).at(-1)!;
    assert.equal(transport.log.count("configuration/sourcesRead"), reads, "one at a time");
    const target = paramsOf(request, "configuration/sourcesRead").target;
    answer(transport, request, target, undefined, String(2000 + reads));
    await settle();
    if (transport.log.count("configuration/sourcesRead") === reads) break;
  }
  assert.ok(transport.log.count("configuration/sourcesRead") <= 1 + 64);
  // Each recovery read observes the User too, in its own slot.
  assert.equal(host.sourceRepairs().filter(({ target }) => target.kind === "workspace").length, 64);
  assert.equal(host.sourceRepairs().length, 65);
});

it("EVICT-07 overlapping reads resolved in reverse order after eviction never overwrite the final state", async () => {
  const { transport, host, status } = await watched();
  // Read A snapshots R1 at 5.
  const readingA = host.readSourceRepairs(WORKSPACE);
  const readA = (await transport.log.awaitMethod("configuration/sourcesRead", 1)).at(-1)!;
  // Native moves to R2 at 6; its notice arrives, then read B snapshots it.
  const r2 = receipt({ id: "r2", status: "unresolved", diagnostic: "final value" });
  transport.notify(repairChanged(WORKSPACE, r2, "6"));
  const readingB = host.readSourceRepairs(WORKSPACE);
  const readB = (await transport.log.awaitMethod("configuration/sourcesRead", 2)).at(-1)!;
  churn(transport, 7);
  assert.equal(held(host, WORKSPACE), undefined);
  // Reverse order: B (current at 6, as new as what was forgotten), then A.
  answer(transport, readB, WORKSPACE, r2, "6");
  await readingB;
  answer(transport, readA, WORKSPACE, UNRESOLVED_R1, "5");
  await readingA;
  assert.deepEqual(held(host, WORKSPACE), { target: WORKSPACE, receipt: { ...r2, target: WORKSPACE }, sequence: "6" });
  // R2 was announced before the eviction and is current again, unannounced.
  assert.deepEqual(status.pendingSources(), []);
  assert.doesNotMatch(presented(status), /obsolete value/);
  // And with neither B nor any notice in between: forgotten at 6, A is stale.
  churn(transport, 71);
  const readingC = host.readSourceRepairs(WORKSPACE);
  const readC = (await transport.log.awaitMethod("configuration/sourcesRead", 3)).at(-1)!;
  transport.notify(repairChanged(WORKSPACE, null, "135"));
  churn(transport, 136);
  answer(transport, readC, WORKSPACE, r2, "134");
  await readingC;
  assert.equal(held(host, WORKSPACE), undefined, "C was older than the withdrawal forgotten during its flight");
});

it("BOUND-05 65 Workspaces without the User keep exactly 64, the oldest forgotten", async () => {
  const sources = new Map<string, number>();
  for (let index = 0; index < 65; index++) retainSource(sources, `workspace:/work/w${index}`, index);
  assert.equal(sources.size, 64);
  assert.ok(!sources.has("workspace:/work/w0"));
  const { transport, host } = await connect(AUTHORITY);
  churn(transport, 1, 65);
  assert.equal(host.sourceRepairs().length, 64);
  assert.equal(held(host, { kind: "workspace", directory: "/work/churn1" }), undefined);
  const status = new SourceRepairStatus();
  status.adopt(AUTHORITY);
  for (let index = 0; index < 65; index++) {
    status.observe(AUTHORITY, workspace(receipt({ id: `w${index}` }), { kind: "workspace", directory: `/work/w${index}` }, String(index + 1)));
  }
  assert.equal(status.pendingSources().length, 64);
  assert.ok(!status.pendingSources().some(target => target.kind === "workspace" && target.directory === "/work/w0"));
  // So does the client's buffer of notices before its first subscriber.
  const early = await connectWith((transport) => churn(transport, 1, 65));
  const buffered = new AppServerHost({ client: early.client, ownership: "external" });
  assert.equal(buffered.sourceRepairs().length, 64);
  assert.equal(held(buffered, { kind: "workspace", directory: "/work/churn1" }), undefined);
});

it("BOUND-06 64 Workspaces and the User coexist", async () => {
  const { transport, host } = await connect(AUTHORITY);
  transport.notify(repairChanged({ kind: "user" }, receipt({ id: "u" }), "1"));
  churn(transport, 2, 64);
  assert.equal(host.sourceRepairs().length, 65);
  assert.equal(held(host, { kind: "workspace", directory: "/work/churn2" })?.sequence, "2");
});

it("BOUND-07 the User observed after Workspace churn is kept beside at most 64 Workspaces", async () => {
  const { transport, host } = await connect(AUTHORITY);
  churn(transport, 1, 100);
  transport.notify(repairChanged({ kind: "user" }, receipt({ id: "u" }), "101"));
  churn(transport, 102, 30);
  const kept = host.sourceRepairs();
  assert.equal(held(host, { kind: "user" })?.receipt?.id, "u");
  assert.equal(kept.filter(({ target }) => target.kind === "workspace").length, 64);
});

it("BOUND-08 re-observing a Workspace refreshes its recency, so the least recently observed goes", async () => {
  const { transport, host } = await connect(AUTHORITY);
  churn(transport, 1, 64);
  transport.notify(repairChanged({ kind: "workspace", directory: "/work/churn1" }, receipt({ id: "again" }), "65"));
  transport.notify(repairChanged({ kind: "workspace", directory: "/work/new" }, null, "66"));
  assert.equal(held(host, { kind: "workspace", directory: "/work/churn1" })?.receipt?.id, "again");
  assert.equal(held(host, { kind: "workspace", directory: "/work/churn2" }), undefined);
  assert.equal(host.sourceRepairs().length, 64);
});

it("BOUND-09 a source evicted at the exact limit without the User stays fenced against its held read", async () => {
  const { transport, host } = await connect(AUTHORITY);
  transport.notify(repairChanged(WORKSPACE, UNRESOLVED_R1, "5"));
  const reading = host.readSourceRepairs(WORKSPACE);
  const read = (await transport.log.awaitMethod("configuration/sourcesRead")).at(-1)!;
  transport.notify(repairChanged(WORKSPACE, null, "6"));
  // W and 63 others are 64: W stays. The 65th Workspace evicts it.
  churn(transport, 7, 63);
  assert.equal(held(host, WORKSPACE)?.sequence, "6");
  churn(transport, 70, 1);
  assert.equal(held(host, WORKSPACE), undefined);
  assert.equal(host.sourceRepairs().length, 64);
  answer(transport, read, WORKSPACE, UNRESOLVED_R1, "5");
  await reading;
  assert.equal(held(host, WORKSPACE), undefined);
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

function repairChanged(target: SourceTarget, value: ConfigRepairReceipt | null, sequence = "1") {
  return notification("configuration/sourceRepairChanged", { target, receipt: value && { ...value, target }, sequence });
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
