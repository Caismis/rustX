import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { it } from "node:test";
import { renderSettings } from "../src/commands/dispatcher.ts";
import { emptyPresentationState, replaceFromSnapshot } from "../src/presentation/projection.ts";
import { RUNTIME_CLIENT_PROTOCOL_VERSION, type RuntimeClientRequest, type RuntimeClientResult, type SettingsLifetimes } from "../src/protocol/types.ts";
import { attemptModel, attemptView, runtimeCursor, sessionModel, snapshot } from "./support/fixtures.ts";

it("CFG238 shares the native protocol fixture and explicit lifetimes", () => {
  const fixture: { request: RuntimeClientRequest; result: RuntimeClientResult; lifetimes: SettingsLifetimes } = JSON.parse(readFileSync(new URL("../../tests/fixtures/runtime-client/settings-v25.json", import.meta.url), "utf8"));
  assert.equal(RUNTIME_CLIENT_PROTOCOL_VERSION, 25);
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
  } });
  const state = replaceFromSnapshot(native, runtimeCursor(1));
  assert.deepEqual(state.settingsLifetimes, native.settings_lifetimes);
  const rendered = renderSettings(state);
  for (const expected of ["Launch capture (safe boundary", "selection (immediate, client-local)", "Runtime policy (future launch)", "resource generation (frozen at admission)", "Admitted execution (resource publication)", "Presentation (next eligible admission)", "Defaults (launch capture"])
    assert.ok(rendered.includes(expected), expected);
});
