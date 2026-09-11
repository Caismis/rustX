import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { it } from "node:test";
import { renderSettings } from "../src/commands/dispatcher.ts";
import { emptyPresentationState, replaceFromSnapshot } from "../src/presentation/projection.ts";
import { RUNTIME_CLIENT_PROTOCOL_VERSION, type EffectiveNativeAgentExtensions, type RuntimeClientRequest, type RuntimeClientResult, type SettingsLifetimes } from "../src/protocol/types.ts";
import { agentStatus, attemptModel, attemptView, runtimeCursor, sessionModel, snapshot, temporalSection } from "./support/fixtures.ts";

it("CFG238 shares the native protocol fixture and explicit lifetimes", () => {
  const fixture: { request: RuntimeClientRequest; result: RuntimeClientResult; lifetimes: SettingsLifetimes } = JSON.parse(readFileSync(new URL("../../tests/fixtures/runtime-client/settings-v26.json", import.meta.url), "utf8"));
  // The fixture is named for the version that introduced its shape. v27 added
  // the subagent `profile_digest` and v28 crash-safe Session deletion, both
  // leaving the settings contract untouched; v29 (Issue #259) added the
  // `effective_extensions.todo` member the fixture now carries in every state,
  // including the `todo_only` combination that proves the two extensions
  // project on independent axes.
  assert.equal(RUNTIME_CLIENT_PROTOCOL_VERSION, 29);
  assert.equal(fixture.request.method, "default_save");
  if (fixture.request.method === "default_save") {
    assert.equal(fixture.request.scope, "user");
    assert.deepEqual(fixture.request.target, "model_selection");
  }
  assert.equal(fixture.result.type, "default_saved");
  assert.deepEqual(fixture.lifetimes, snapshot().settings_lifetimes);
});

it("CFG238 reconstructs distinct launch, Session, pending and frozen facts from a fresh snapshot", () => {
  const native = snapshot({
    launch_settings: { model: { model: "local/a", reasoning_profile: null }, model_origin: { kind: "user", document: "/config/settings.jsonc" }, reasoning_origin: { kind: "builtin" }, approval_mode: "policy", approval_origin: { kind: "builtin" }, runtime_root_origin: { kind: "cli" }, tool_selection_origin: { kind: "builtin" } },
    model: sessionModel("local/b"),
    effective_approval_mode: "policy", pending_approval_mode: "full_access",
    resources: { revision: 8, context_files: [], agent_profile: false },
    attempt: attemptView({ model: attemptModel("local/c"), execution_settings: { resource_revision: 7, approval_mode: "policy" } }),
  });
  const live = replaceFromSnapshot(native, runtimeCursor(50));
  const reconnect = replaceFromSnapshot(structuredClone(native), runtimeCursor(50));
  assert.deepEqual(reconnect, live);
  const rendered = renderSettings(reconnect);
  for (const text of ["local/a", "local/b", "local/c", "source user", "pending approval: full_access", "published revision: 8", "resource revision: 7", "next eligible admission", "frozen"])
    assert.ok(rendered.includes(text), text);
  assert.ok(!rendered.includes("requestParams"));
  assert.deepEqual(native, snapshot({ ...native }));
});

it("CFG238 historical settings remain explicitly unavailable", () => {
  const state = replaceFromSnapshot(snapshot({ model: null, settings_evidence: "historical_partial", launch_settings: null, attempt: attemptView({ model: null, execution_settings: null }) }), runtimeCursor(0));
  const rendered = renderSettings(state);
  assert.match(rendered, /partial/);
  assert.match(rendered, /unavailable/);
  assert.ok(!rendered.includes("inspection/durable"));
});

it("CFG238 renders every lifetime from the supplied native field, with no unattached defaults", () => {
  const empty = emptyPresentationState(sessionModel("local/a"));
  assert.equal(empty.settingsLifetimes, null);
  const native = snapshot({ settings_lifetimes: {
    launch: "safe_boundary", model: "client_local", approval: "next_launch",
    resources: "frozen_admission", attempt: "resource_publication",
    presentation: "next_admission", saved_defaults: "launch_capture",
    extensions: "next_launch",
  } });
  const state = replaceFromSnapshot(native, runtimeCursor(1));
  assert.deepEqual(state.settingsLifetimes, native.settings_lifetimes);
  const rendered = renderSettings(state);
  for (const expected of ["Launch capture (safe boundary", "selection (immediate, client-local)", "Runtime policy (future launch)", "resource generation (frozen at admission)", "Admitted execution (resource publication)", "Presentation (next eligible admission)", "Defaults (launch capture", "Native Agent Extensions (future launch)"])
    assert.ok(rendered.includes(expected), expected);
});

/**
 * Issue #256 regression 10 (TypeScript half): the same shared protocol
 * fixture the Rust `ext256_effective_extension_protocol_fixture_round_trips_exactly`
 * test reads. Both halves must agree on all three semantic states and on the
 * new `extensions` lifetime, or the wire contract has drifted.
 */
it("EXT256 shares the native effective-extension protocol fixture exactly", () => {
  const fixture: {
    lifetimes: SettingsLifetimes;
    child_lifetimes: SettingsLifetimes;
    effective_extensions: Record<"composed" | "contributors_disabled" | "not_composed", EffectiveNativeAgentExtensions>;
  } = JSON.parse(readFileSync(new URL("../../tests/fixtures/runtime-client/settings-v26.json", import.meta.url), "utf8"));

  assert.equal(fixture.lifetimes.extensions, "launch_capture");
  assert.equal(fixture.child_lifetimes.extensions, "frozen_admission");
  assert.equal(fixture.child_lifetimes.model, "frozen_admission");
  assert.deepEqual(fixture.lifetimes, snapshot().settings_lifetimes);

  // The three states are distinct values on the wire, not spellings of one
  // another: "not composed" is never "composed with everything off".
  const { composed, contributors_disabled: disabled, not_composed: absent } = fixture.effective_extensions;
  assert.deepEqual(composed.agent_status, { time: { enabled: true, timezone: "Asia/Shanghai" }, background: { enabled: true } });
  assert.deepEqual(disabled.agent_status, { time: { enabled: false, timezone: null }, background: { enabled: false } });
  assert.equal(absent.agent_status, null);
  assert.notDeepEqual(composed, disabled);
  assert.notDeepEqual(disabled, absent);
});

/**
 * Issue #256 regression 11: `/settings` distinguishes an absent Agent Status
 * extension, a composed one, and a configured timezone from no explicit one —
 * and says nothing about extension internals.
 */
it("EXT256 /settings distinguishes absent, composed, and timezone-configured Agent Status", () => {
  const composed = renderSettings(replaceFromSnapshot(snapshot({
    effective_extensions: { agent_status: { time: { enabled: true, timezone: "Asia/Shanghai" }, background: { enabled: true } }, todo: {} },
  }), runtimeCursor(1)));
  assert.match(composed, /### Native Agent Extensions \(launch capture\)/);
  assert.match(composed, /- Agent Status: enabled/);
  assert.match(composed, /  - Time: enabled/);
  assert.match(composed, /  - timezone: Asia\/Shanghai/);
  assert.match(composed, /  - Background: enabled/);
  assert.ok(!composed.includes("none configured"));
  assert.match(composed, /reload republishes resources and never recomposes extensions/);

  // Composed, but with both contributors off. This is not "disabled".
  const idle = renderSettings(replaceFromSnapshot(snapshot({
    effective_extensions: { agent_status: { time: { enabled: false, timezone: null }, background: { enabled: false } }, todo: {} },
  }), runtimeCursor(1)));
  assert.match(idle, /- Agent Status: enabled/);
  assert.match(idle, /  - Time: disabled/);
  assert.match(idle, /  - timezone: none configured/);
  assert.match(idle, /  - Background: disabled/);

  // Not part of the composition at all: no contributor lines exist to read.
  const absent = renderSettings(replaceFromSnapshot(snapshot({
    effective_extensions: { agent_status: null, todo: null },
  }), runtimeCursor(1)));
  assert.match(absent, /- Agent Status: disabled \(not composed for this Agent\)/);
  assert.ok(!absent.includes("- Time:"));
  assert.ok(!absent.includes("timezone"));
  assert.ok(!absent.includes("- Background:"));

  // A frozen child reports the frozen-child lifetime and its own vocabulary.
  const child = renderSettings(replaceFromSnapshot(snapshot({
    settings_evidence: "frozen_child",
    settings_lifetimes: { ...snapshot().settings_lifetimes, model: "frozen_admission", extensions: "frozen_admission" },
    effective_extensions: { agent_status: { time: { enabled: true, timezone: "America/New_York" }, background: { enabled: false } }, todo: {} },
  }), runtimeCursor(1)));
  assert.match(child, /### Native Agent Extensions \(frozen at admission\)/);
  assert.match(child, /  - timezone: America\/New_York/);
  assert.match(child, /child execution profile its invoking generation resolved and froze/);

  // The rendering is native facts only: no prompt, registry, or document dump.
  for (const rendered of [composed, idle, absent, child])
    for (const forbidden of ["system-reminder", "requestParams", "rustx.jsonc", "NativeAgentExtensionsDocument"])
      assert.ok(!rendered.includes(forbidden), forbidden);
});

/**
 * Issue #256 regression 3 (client half): enablement comes from the
 * projection, never from the composed-status window. An extension that is
 * enabled but has produced no observation still renders as enabled, and an
 * absent extension is not implied to be present by a status that exists.
 */
it("EXT256 /settings never infers extension enablement from Agent Status observations", () => {
  const enabledWithoutObservation = replaceFromSnapshot(snapshot({
    effective_extensions: { agent_status: { time: { enabled: true, timezone: "UTC" }, background: { enabled: true } }, todo: {} },
    statuses: [],
  }), runtimeCursor(1));
  assert.deepEqual(enabledWithoutObservation.statuses, []);
  assert.match(renderSettings(enabledWithoutObservation), /- Agent Status: enabled/);

  const absentWithObservation = replaceFromSnapshot(snapshot({
    effective_extensions: { agent_status: null, todo: null },
    statuses: [agentStatus({ status_message_id: "m1", sections: [temporalSection()] })],
  }), runtimeCursor(1));
  assert.equal(absentWithObservation.statuses.length, 1);
  assert.match(renderSettings(absentWithObservation), /- Agent Status: disabled \(not composed for this Agent\)/);
});

/**
 * Issue #256 regression 12 (client half): a reconnect rebuilds the identical
 * effective-extension view from the authoritative snapshot alone. There is no
 * client-side merge with the state it replaces, so a fresh projection of the
 * same snapshot is byte-identical.
 */
it("EXT256 reconnect reconstructs the same effective-extension view", () => {
  const native = snapshot({
    effective_extensions: { agent_status: { time: { enabled: true, timezone: "Asia/Shanghai" }, background: { enabled: false } }, todo: {} },
  });
  const live = replaceFromSnapshot(native, runtimeCursor(9));
  const reconnect = replaceFromSnapshot(structuredClone(native), runtimeCursor(9));
  assert.deepEqual(reconnect.effectiveExtensions, live.effectiveExtensions);
  assert.equal(renderSettings(reconnect), renderSettings(live));

  // An unattached client has no composition of its own to show.
  assert.equal(emptyPresentationState(sessionModel("local/a")).effectiveExtensions, null);
});

/**
 * Issue #256 regression 9 (client half): historical-only inspection reports
 * the composition as unavailable instead of inventing one.
 */
it("EXT256 historical inspection reports no effective extension composition", () => {
  const rendered = renderSettings(replaceFromSnapshot(snapshot({
    model: null, settings_evidence: "historical_partial", launch_settings: null,
    effective_extensions: null,
  }), runtimeCursor(0)));
  assert.match(rendered, /### Native Agent Extensions \(evidence unavailable\)/);
  assert.match(rendered, /no effective extension composition exists for historical-only inspection/);
  assert.ok(!rendered.includes("launch capture"), "no boundary is claimed for an absent value");
  assert.ok(!rendered.includes("Agent Status: enabled"));
  assert.ok(!rendered.includes("Agent Status: disabled"));
});
