import { checkpoint } from './support/fixtures.ts';
import { domainReceipt } from './support/fixtures.ts';
import { contentAdvance } from './support/fixtures.ts';
import { followAttachment } from './support/app-server-peer.ts';
import { fixtures as uploadFixtures } from "../../protocol/app-server/fixtures.ts";
const uploadPolicy = uploadFixtures.find(row => "result" in row && row.result?.type === "initialized")!;
const nativeUploadPolicy = "result" in uploadPolicy && uploadPolicy.result?.type === "initialized" ? uploadPolicy.result.capabilities.upload_policy : (() => { throw new Error("missing policy") })();
/**
 * The A -> B model invariant, proven deterministically.
 *
 * ```text
 * session model = A
 * attempt admitted; the attempt freezes A
 * model_set(B) succeeds while the attempt is active
 *
 *   presentation MUST show:
 *     desired session model = B
 *     active attempt model  = A
 *
 * after settlement, the next attempt admitted
 *   presentation MUST show:
 *     active attempt model  = B
 * ```
 *
 * The currently executing attempt must never visually mutate to B. This is
 * proven twice — once through the pure reducer, once end to end through the
 * typed App Server client and its Session owner against a scripted peer — and
 * neither proof uses a timer.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { reduce } from "../src/presentation/projection.ts";
import { replaceFromSnapshot } from "../src/presentation/projection.ts";
import type { PresentationState } from "../src/presentation/state.ts";
import {
  APP_SERVER_PROTOCOL_VERSION,
  AppServerClient,
} from "../src/app-server/client.ts";
import { AppServerSession } from "../src/app-server/session.ts";
import type {
  AttachmentTarget,
  RuntimeClientEvent,
} from "../src/protocol/app-server.ts";
import type { ProjectedEvent } from "../src/presentation/projection.ts";
import { runtimeCursor, transcriptCursor } from "./support/fixtures.ts";
import {
  attemptModel,
  catalogModel,
  inboundBlock,
  sessionModel,
  snapshot,
  nextCursor,
} from "./support/fixtures.ts";
import { FakeTransport, paramsOf, tick } from "./support/app-server-peer.ts";
import { harness, nextRequest } from "./support/app-server-harness.ts";
import { renderModel } from "../src/commands/presentation.ts";
import { TransientFeedbackSurface } from "../src/ui/components/transient-feedback.ts";
import { UncertainOutcomeError } from "../src/app-server/client.ts";

const MODEL_A = "alpha/model-a";
const MODEL_B = "beta/model-b";

function fold(
  state: PresentationState,
  events: RuntimeClientEvent[],
): PresentationState {
  let current = state;
  let cursor = current.cursor;
  for (const event of events) {
    cursor = nextCursor(cursor);
    const protocolEvent: ProjectedEvent = { cursor, event };
    current = reduce(current, protocolEvent);
  }
  return current;
}

describe("session model A -> B invariant", () => {
  it("same-model profile changes retain the complete frozen invocation across events and reconnect", () => {
    const frozen = attemptModel(MODEL_A, { profile: "high", reasoningEnabled: true, requestParams: { effort: "high", temperature: 0.2 }, maxOutputTokens: 777 });
    frozen.summary = { mode: "explicit", ...frozen.primary, model: "summary/original" };
    const initial = snapshot({ model: sessionModel(MODEL_A, frozen.primary), attempt: { attempt_id: "profile-attempt", phase: { type: "running" }, turn: 1, model: frozen, execution_settings: null } });
    const low = sessionModel(MODEL_A, { profile: "low", reasoningEnabled: false, requestParams: { effort: "low", temperature: 0.2 }, maxOutputTokens: 777 });
    low.configured.profile = "low";
    const changed = fold(replaceFromSnapshot(initial, runtimeCursor(0)), [{ type: "session_model_changed", model_status: { available: true, body_deferred: false }, model: low }]);
    assert.deepEqual(changed.attempt?.model, frozen);
    assert.equal(changed.sessionModel?.effective?.profile, "low");
    const rendered = renderModel(changed);
    assert.match(rendered, /effective profile: low/);
    assert.match(rendered, /profile: high/);
    assert.match(rendered, /"effort": "high"/);
    assert.match(rendered, /summary\/original/);
    const recovered = replaceFromSnapshot({ ...initial, model: low }, runtimeCursor(1));
    assert.deepEqual(recovered.attempt?.model, frozen);
    assert.equal(recovered.sessionModel?.configured.profile, "low");
  });

  it("rejected selection rereads authority and prevents another mutation until the reread settles", async () => {
    const h = await harness(snapshot({ model: sessionModel(MODEL_A) }));
    const rejected = assert.rejects(h.session.modelSet({ model: MODEL_B }), /selection conflict/);
    const set = (await h.transport.log.awaitMethod("session/setModel")).at(-1)!;
    h.transport.respondError(set.id, { code: -32000, message: "selection conflict" });
    const read = (await h.transport.log.awaitMethod("session/checkpoint", 2)).at(-1)!;
    await assert.rejects(h.session.modelSet({ model: MODEL_B }), /still in flight/);
    h.transport.respond(read.id, {type:'checkpoint',checkpoint:checkpoint(snapshot({ model: sessionModel(MODEL_A) }),runtimeCursor(1))});
    const subscribe = (await h.transport.log.awaitMethod("session/subscribe", 2)).at(-1)!;
    h.transport.respond(subscribe.id, { type: "subscribed", after_cursor: runtimeCursor(1) });
    await rejected;
    assert.equal(h.session.state.sessionModel?.configured.model, MODEL_A);
    assert.equal(h.transport.log.count("session/setModel"), 1);
    assert.equal(h.session.modelSelectionOutcomeUnknown, false, "known rejection is terminal evidence");
    h.client.close();
  });

  it("lost B acknowledgement remains unknown after snapshot A, acknowledged no-op A, late B and reconnect B without replay", async () => {
    const frozen = attemptModel(MODEL_A, { profile: "high" });
    const initial = snapshot({ model: sessionModel(MODEL_A), attempt: { attempt_id: "still-running", phase: { type: "running" }, turn: 1, model: frozen, execution_settings: null } });
    const h = await harness(initial);
    const uncertain = assert.rejects(h.session.modelSet({ model: MODEL_B }), UncertainOutcomeError);
    await h.transport.log.awaitMethod("session/setModel");
    h.transport.fail("input_eof");
    await uncertain;
    assert.equal(h.session.state.sessionModel?.configured.model, MODEL_A);
    assert.equal(h.transport.log.count("session/setModel"), 1);
    assert.equal(h.session.modelSelectionOutcomeUnknown, true);

    // Native r10 parks captured B across exactly this old readback and no-op
    // A. This peer controls the client boundary; it does not simulate native CAS.
    const recovered = await harness(initial, h.session.sessionId, h.host);
    assert.equal(recovered.session.state.sessionModel?.configured.model, MODEL_A);
    assert.equal(recovered.session.modelSelectionOutcomeUnknown, true);
    const repairing = recovered.session.resync();
    const read = (await recovered.transport.log.awaitMethod("session/checkpoint", 2)).at(-1)!;
    recovered.transport.respond(read.id, {type:'checkpoint',checkpoint:checkpoint(initial,runtimeCursor(1))});
    const subscribe = (await recovered.transport.log.awaitMethod("session/subscribe", 2)).at(-1)!;
    recovered.transport.respond(subscribe.id, { type: "subscribed", after_cursor: runtimeCursor(1) });
    await repairing;
    assert.equal(recovered.session.modelSelectionOutcomeUnknown, true);
    assert.equal(recovered.transport.log.count("session/setModel"), 0, "readback does not replay B");
    const showing = await recovered.dispatcher.submit("/model show");
    assert.equal(showing.kind, "inspect");
    if (showing.kind === "inspect") assert.match(showing.body, /readback cannot settle it/);

    // A new intentional request is permitted, but its acknowledgement settles
    // only that request. A true no-op A does not fence previously captured B.
    const choosing = recovered.dispatcher.selectModel(catalogModel(MODEL_A));
    const modelRead = (await recovered.transport.log.awaitMethod("session/model")).at(-1)!;
    recovered.transport.respond(modelRead.id, domainReceipt('model',[sessionModel(MODEL_A)]));
    const set = (await recovered.transport.log.awaitMethod("session/setModel")).at(-1)!;
    assert.equal(paramsOf(set, "session/setModel").config.model, MODEL_A);
    recovered.transport.respond(set.id, {type:'model_set',settings_revision:'1'});
    const confirmedRead=(await recovered.transport.log.awaitMethod("session/model",2)).at(-1)!;
    recovered.transport.respond(confirmedRead.id,domainReceipt("model",[sessionModel(MODEL_A)]));
    const selected = await choosing;
    assert.equal(selected.kind, "transient");
    if (selected.kind === "transient") {
      assert.equal(selected.level, "info");
      assert.match(selected.text, /Earlier model selection outcome unknown/);
      const feedback = new TransientFeedbackSurface();
      feedback.replace(selected);
      assert.match(feedback.render(80).join("\n"), /readback cannot settle it/);
      assert.ok(selected.text.split("\n").length <= 3);
    }
    assert.equal(recovered.session.modelSelectionOutcomeUnknown, true);
    recovered.transport.emit(recovered.target, runtimeCursor(2), { type: "session_model_changed", model_status: { available: true, body_deferred: false }, model: sessionModel(MODEL_B) });
    assert.equal(recovered.session.state.sessionModel?.configured.model, MODEL_B);
    assert.deepEqual(recovered.session.state.attempt?.model, frozen);
    assert.equal(recovered.session.modelSelectionOutcomeUnknown, true, "an observation is not a correlated response");

    const detaching = recovered.host.detach(h.session.sessionId);
    const detach = (await recovered.transport.log.awaitMethod("session/detach")).at(-1)!;
    recovered.transport.respond(detach.id, { type: "detached" });
    await detaching;
    const attaching = recovered.host.attach(h.session.sessionId);
    const attach = (await recovered.transport.log.awaitMethod("session/attach", 2)).at(-1)!;
    const replacementTarget={...recovered.target,attachment_id:"att-2",runtime_incarnation:"2"};
    recovered.transport.respond(attach.id,{type:"attached",target:replacementTarget,attachment_kind:"controller",observation_source:"live"});
    await followAttachment(recovered.transport,replacementTarget,{...initial,model:sessionModel(MODEL_B)},runtimeCursor(0));
    assert.equal((await attaching).modelSelectionOutcomeUnknown, true, "a replacement runtime still cannot prove the old request's outcome");

    const reconnected = await harness({ ...initial, model: sessionModel(MODEL_B) }, h.session.sessionId, recovered.host);
    assert.equal(reconnected.session.state.sessionModel?.configured.model, MODEL_B);
    assert.deepEqual(reconnected.session.state.attempt?.model, frozen);
    assert.equal(reconnected.session.modelSelectionOutcomeUnknown, true);
    assert.equal(reconnected.transport.log.count("session/setModel"), 0);
    recovered.client.close();
    reconnected.client.close();
  });

  it("a successful snapshot cannot release the guard for a still pending selection", async () => {
    const h = await harness(snapshot({ model: sessionModel(MODEL_A) }));
    const retired = assert.rejects(h.session.modelSet({ model: MODEL_B }), /acknowledgement belongs to a retired observation/);
    const set = (await h.transport.log.awaitMethod("session/setModel")).at(-1)!;
    const repairing = h.session.resync();
    const read = (await h.transport.log.awaitMethod("session/checkpoint", 2)).at(-1)!;
    h.transport.respond(read.id, {type:'checkpoint',checkpoint:checkpoint(snapshot({ model: sessionModel(MODEL_A) }),runtimeCursor(1))});
    const subscribe = (await h.transport.log.awaitMethod("session/subscribe", 2)).at(-1)!;
    h.transport.respond(subscribe.id, { type: "subscribed", after_cursor: runtimeCursor(1) });
    await repairing;
    await assert.rejects(h.session.modelSet({ model: MODEL_A }), /still in flight/);
    h.transport.respond(set.id, {type:'model_set',settings_revision:'1'});
    const readAfterAck = (await h.transport.log.awaitMethod("session/checkpoint", 3)).at(-1)!;
    h.transport.respond(readAfterAck.id, {type:'checkpoint',checkpoint:checkpoint(snapshot({ model: sessionModel(MODEL_B) }),runtimeCursor(2))});
    const subscribedAfterAck = (await h.transport.log.awaitMethod("session/subscribe", 3)).at(-1)!;
    h.transport.respond(subscribedAfterAck.id, { type: "subscribed", after_cursor: runtimeCursor(2) });
    await retired;
    assert.equal(h.session.modelSelectionOutcomeUnknown, false, "the native acknowledgement was received, but its observation retired");
    assert.equal(h.session.state.sessionModel?.configured.model, MODEL_B);
    assert.equal(h.transport.log.count("session/setModel"), 1);
    h.client.close();
  });

  it("a committed durability-uncertain response is known committed evidence, not an unanswered request", async () => {
    const h = await harness(snapshot({ model: sessionModel(MODEL_A) }));
    const rejected = assert.rejects(h.session.modelSet({ model: MODEL_B }), /committed.*durability.*unknown/);
    const set = (await h.transport.log.awaitMethod("session/setModel")).at(-1)!;
    h.transport.respondError(set.id, { code: -32000, message: "directory sync failed", data: { kind: "committed_durability_uncertain" } });
    const read = (await h.transport.log.awaitMethod("session/checkpoint", 2)).at(-1)!;
    assert.equal(h.session.state.sessionModel?.configured.model, MODEL_A, "the response does not publish optimistic authority");
    h.transport.respond(read.id, {type:'checkpoint',checkpoint:checkpoint(snapshot({ model: sessionModel(MODEL_B) }),runtimeCursor(1))});
    const subscribe = (await h.transport.log.awaitMethod("session/subscribe", 2)).at(-1)!;
    h.transport.respond(subscribe.id, { type: "subscribed", after_cursor: runtimeCursor(1) });
    await rejected;
    assert.equal(h.session.modelSelectionOutcomeUnknown, false);
    assert.equal(h.session.state.sessionModel?.configured.model, MODEL_B);
    h.client.close();
  });

  it("an acknowledgement from a retired attachment cannot report selection success", async () => {
    const h = await harness(snapshot({ model: sessionModel(MODEL_A) }));
    const rejected = assert.rejects(h.session.modelSet({ model: MODEL_B }), /retired observation/);
    const set = (await h.transport.log.awaitMethod("session/setModel")).at(-1)!;
    const detaching = h.session.detach();
    const detach = (await h.transport.log.awaitMethod("session/detach")).at(-1)!;
    h.transport.respond(set.id, {type:'model_set',settings_revision:'1'});
    h.transport.respond(detach.id, { type: "detached" });
    await Promise.all([rejected, detaching]);
    assert.equal(h.session.state.sessionModel?.configured.model, MODEL_A);
    assert.equal(h.transport.log.count("session/setModel"), 1);
    assert.equal(h.session.modelSelectionOutcomeUnknown, false);
    h.client.close();
  });

  it("keeps the running attempt on A while the session moves to B", () => {
    let state = replaceFromSnapshot(
      snapshot({ model: sessionModel(MODEL_A) }),
      runtimeCursor(0),
    );

    // The attempt is admitted while the session model is A.
    state = fold(state, [
      { type: "attempt_started", execution_settings: null, attempt_id: "a1", model: attemptModel(MODEL_A) },
    ]);
    assert.equal(state.attempt?.model!.primary.model, MODEL_A);
    assert.equal(state.sessionModel!.configured.model, MODEL_A);

    // The session switches to B while that attempt is still running.
    state = fold(state, [
      { type: "session_model_changed", model_status: { available: true, body_deferred: false }, model: sessionModel(MODEL_B) },
    ]);

    assert.equal(
      state.sessionModel!.configured.model,
      MODEL_B,
      "desired session model = B",
    );
    assert.equal(
      state.attempt?.model!.primary.model,
      MODEL_A,
      "active attempt model stays A",
    );
    assert.equal(
      state.attempt?.phase.type,
      "running",
      "the running attempt is not restarted by a model change",
    );

    // The attempt settles; it still reports the model it ran with.
    state = fold(state, [
      {
        type: "attempt_settled",
        attempt_id: "a1",
        outcome: { type: "completed", finish_reason: { type: "stop" } },
      },
    ]);
    assert.equal(state.attempt?.model!.primary.model, MODEL_A);
    assert.equal(state.sessionModel!.configured.model, MODEL_B);

    // The next admission uses B.
    state = fold(state, [
      { type: "attempt_started", execution_settings: null, attempt_id: "a2", model: attemptModel(MODEL_B) },
    ]);
    assert.equal(state.attempt?.attemptId, "a2");
    assert.equal(
      state.attempt?.model!.primary.model,
      MODEL_B,
      "the next attempt uses B",
    );
  });

  it("never mutates the active attempt model on any interleaved event", () => {
    let state = replaceFromSnapshot(
      snapshot({ model: sessionModel(MODEL_A) }),
      runtimeCursor(0),
    );
    state = fold(state, [
      { type: "attempt_started", execution_settings: null, attempt_id: "a1", model: attemptModel(MODEL_A) },
    ]);

    // Every kind of activity that can occur mid-attempt, with the session
    // switching to B in the middle of it.
    const interleaved: RuntimeClientEvent[] = [
      { type: "assistant_message_started", attempt_id: "a1", message_id: "m1" },
      contentAdvance({attempt_id:"a1",message_id:"m1",block_index:0,kind:'text',delta:"still on A"}, 0, 1),
      { type: "session_model_changed", model_status: { available: true, body_deferred: false }, model: sessionModel(MODEL_B) },
      {
        type: "tool_execution_started",
        attempt_id: "a1",
        tool_call_id: "c1",
        tool_id: "tool-bash",
      },
      { type: "capability_updated", capabilities: { configured_tools: [], revision: "9" } },
      {
        type: "inbound_enqueued",
        sequence: "1",
        message: inboundBlock("m2", "queued"),
        transcript_cursor: transcriptCursor(1),
      },
    ];

    for (const event of interleaved) {
      state = fold(state, [event]);
      assert.equal(
        state.attempt?.model!.primary.model,
        MODEL_A,
        `the attempt model survived ${event.type}`,
      );
    }
    assert.equal(state.sessionModel!.configured.model, MODEL_B);
  });

  it("proves the invariant end to end through the App Server client", async () => {
    const transport = new FakeTransport();
    const connecting = AppServerClient.initialize({ transport });
    const initialize = (await transport.log.awaitMethod("initialize")).at(-1)!;
    transport.respond(initialize.id, {
      type: "initialized", authority_id: 'fixture-app-server-authority',
      protocol_version: APP_SERVER_PROTOCOL_VERSION,
      capabilities: {
        upload_policy: nativeUploadPolicy, multi_session: true,
        single_writable_controller: true,
        headless_interactions: true, delivery_access: false,
        experimental_methods: [],
      },
    });
    const client = await connecting;

    const target: AttachmentTarget = {
      session_id: "ses_0a7e1c20-6bdf-7720-acad-22e2d65841be",
      conversation_id: "conv_36524fd8-f674-7fc2-b125-06d01fee0e18",
      runtime_incarnation: "1",
      attachment_id: "att-1",
    };
    const attaching = AppServerSession.attach(client, target.session_id);
    const attach = (await transport.log.awaitMethod("session/attach")).at(-1)!;
    // Identity receipt, bounded cut and registration are independent boundaries.
    transport.respond(attach.id, {type:'attached', target, configuration_revision:'0', attachment_kind:'controller', observation_source:'live'});
    await followAttachment(transport,target,snapshot({model:sessionModel(MODEL_A)}),runtimeCursor(0));
    const session = await attaching;
    const stop = client.onNotification((message) =>
      session.applyNotification(message),
    );

    assert.equal(session.state.sessionModel!.configured.model, MODEL_A);

    // The runtime admits an attempt on A. The start event is self-contained:
    // the client learns the frozen model without a second snapshot read.
    transport.emit(target, runtimeCursor(1), {
      type: "attempt_started",
      execution_settings: null,
      attempt_id: "a1",
      model: attemptModel(MODEL_A),
    });
    await tick();
    assert.equal(session.state.attempt?.attemptId, "a1");
    assert.equal(session.state.attempt?.model!.primary.model, MODEL_A);

    // The client requests B while the attempt runs; the runtime accepts.
    const setting = session.modelSet({ model: MODEL_B });
    const modelSet = (await transport.log.awaitMethod("session/setModel")).at(-1)!;
    assert.deepEqual(paramsOf(modelSet, "session/setModel").target, target);
    transport.respond(modelSet.id, {type:'model_set',settings_revision:'1'});
    const modelRead=(await transport.log.awaitMethod('session/model')).at(-1)!;
    transport.respond(modelRead.id,domainReceipt('model',[sessionModel(MODEL_B)]));
    await setting;

    // The authoritative change arrives on the same observation stream.
    transport.emit(target, runtimeCursor(2), {
      type: "session_model_changed", model_status: { available: true, body_deferred: false },
      model: sessionModel(MODEL_B),
    });
    await tick();
    assert.equal(session.state.sessionModel!.configured.model, MODEL_B);

    assert.equal(
      session.state.attempt?.model!.primary.model,
      MODEL_A,
      "the executing attempt did not visually mutate to B",
    );
    assert.equal(session.state.attempt?.phase.type, "running");

    // Settle, then admit the next attempt.
    transport.emit(target, runtimeCursor(3), {
      type: "attempt_settled",
      attempt_id: "a1",
      outcome: { type: "completed", finish_reason: { type: "stop" } },
    });
    transport.emit(target, runtimeCursor(4), {
      type: "attempt_started",
      execution_settings: null,
      attempt_id: "a2",
      model: attemptModel(MODEL_B),
    });
    await tick();

    assert.equal(session.state.attempt?.attemptId, "a2");
    assert.equal(
      session.state.attempt?.model!.primary.model,
      MODEL_B,
      "the next attempt uses B",
    );
    stop();
    client.close();
  });

  it("presents runtime-published reasoning support without inventing profiles", () => {
    // A reasoning-capable model with no declared profiles means: reasoning is
    // supported, no profile is selectable, and provider/runtime defaults
    // apply. The client must not synthesize off/low/medium/high.
    const alwaysOn = sessionModel("always/always-on", {
      reasoningEnabled: true,
      capabilities: {
        inputModalities: ["text"],
        outputModalities: ["text"],
        toolCalls: true,
        reasoning: true,
        jsonSchemaOutput: false,
      },
      declaredCapabilities: {
        inputModalities: ["text"],
        outputModalities: ["text"],
        toolCalls: true,
        reasoning: true,
        jsonSchemaOutput: false,
      },
    });
    const state = replaceFromSnapshot(
      snapshot({ model: alwaysOn }),
      runtimeCursor(0),
    );

    assert.equal(state.sessionModel!.effective!.capabilities.reasoning, true);
    assert.equal(state.sessionModel!.effective!.reasoningEnabled, true);
    assert.equal(
      state.sessionModel!.effective!.profile,
      undefined,
      "no profile is invented for a model that declares none",
    );
  });

  it("retains native JSON Schema capability in published model snapshots", () => {
    for (const enabled of [false, true]) {
      const model = sessionModel(MODEL_A);
      model.effective!.capabilities.jsonSchemaOutput = enabled;
      model.effective!.declaredCapabilities.jsonSchemaOutput = enabled;
      const state = replaceFromSnapshot(snapshot({ model }), runtimeCursor(0));
      assert.equal(state.sessionModel!.effective!.capabilities.jsonSchemaOutput, enabled);
      assert.equal(state.sessionModel!.effective!.declaredCapabilities.jsonSchemaOutput, enabled);
    }
  });

  it("shows effective capability, not the raw catalog claim", () => {
    // The catalog claims image input; the runtime cannot represent it yet.
    const narrowed = sessionModel(MODEL_A, {
      capabilities: {
        inputModalities: ["text"],
        outputModalities: ["text"],
        toolCalls: true,
        reasoning: false,
        jsonSchemaOutput: false,
      },
      declaredCapabilities: {
        inputModalities: ["text", "image"],
        outputModalities: ["text"],
        toolCalls: true,
        reasoning: false,
        jsonSchemaOutput: false,
      },
    });
    const state = replaceFromSnapshot(
      snapshot({ model: narrowed }),
      runtimeCursor(0),
    );

    assert.deepEqual(state.sessionModel!.effective!.capabilities.inputModalities, [
      "text",
    ]);
    assert.ok(
      state.sessionModel!.effective!.declaredCapabilities.inputModalities.includes(
        "image",
      ),
      "the declaration is still available to explain the difference",
    );
  });
});


it('T13 T15 finite native catalog selection confirms a receipt without loading a growing model identity', async () => {
  const h=await harness();
  const choosing=h.dispatcher.submit('/model-position 9007199254740993 42 7');
  const request=await nextRequest(h,'session/selectModel');
  assert.deepEqual(request.params,{target:h.target,choice:{binding_revision:'9007199254740993',model_position:42,profile_position:7}});
  h.transport.respond(request.id,{type:'model_set',settings_revision:'9'});
  const result=await choosing;
  assert.equal(result.kind,'transient');
  assert.match(JSON.stringify(result),/selection confirmed/);
  assert.equal(h.transport.transportCount('session/model'),0);
  assert.equal(h.session.state.cursor,'0');
  assert.equal(h.session.modelSelectionOutcomeUnknown,false);
  h.client.close();
});

it('T13 T20 native catalog selection validates safe ordinals and never replays an unknown mutation', async () => {
  const h=await harness();
  await assert.rejects(h.session.modelSelect({binding_revision:'1',model_position:9007199254740992,profile_position:null}),/ordinal/);
  assert.equal(h.transport.transportCount('session/selectModel'),0);
  const choosing=h.session.modelSelect({binding_revision:'1',model_position:42,profile_position:null});
  const rejected=assert.rejects(choosing,error=>error instanceof Error&&error.name==='UncertainOutcomeError');
  await nextRequest(h,'session/selectModel');
  h.client.close();
  await rejected;
  assert.equal(h.session.modelSelectionOutcomeUnknown,true);
  assert.equal(h.transport.transportCount('session/selectModel'),1);
  assert.equal(h.transport.transportCount('session/setModel'),0);
});


it('T15 model catalog browsing retains deferred rows and profile evidence without loading their bodies', async () => {
  const h = await harness();
  const reading = h.session.modelCatalog();
  const request = await nextRequest(h, 'session/models');
  const complete = catalogModel('small/model');
  const partial = {...catalogModel('partial/model'), profilesDeferred: true, profileCount: '1000'};
  const page = domainReceipt('model_catalog.models', [complete]).page;
  page.items = [
    {...page.items[0]!, offset: '0', value: complete},
    {...page.items[0]!, offset: '1', value: partial},
    {...page.items[0]!, offset: '2', value: null, deferred: true},
  ];
  page.total = '100'; page.next_offset = '3';
  h.transport.respond(request.id, {type: 'domain_page', page});
  assert.deepEqual((await reading).models, [complete]);
  assert.match(h.session.catalogDeferredNotice!, /positions 1, 2/);
  assert.match(h.session.catalogDeferredNotice!, /native defaults/);
  assert.equal(h.session.nextModelOffset, 3);
  assert.equal(h.transport.transportCount('conversation/checkpointValueRead'), 0);
  assert.equal(h.session.state.cursor, '0');
  h.client.close();
});


it('T05 T11 unavailable checkpoint keeps Applied and exposes an observation failure separately', async () => {
  const h = await harness();
  const recovering = h.session.resync();
  const refused = assert.rejects(recovering, /native transcript commit awaits/);
  const request = await nextRequest(h, 'session/checkpoint');
  h.transport.respondError(request.id, {code: -32000, message: 'observation unavailable: native transcript commit awaits its observation owner', data: {kind: 'observation_unavailable', diagnostic: 'native transcript commit awaits its observation owner'}});
  await refused;
  assert.equal(h.session.state.cursor, '0');
  assert.equal(h.session.observationPhase, 'unavailable');
  assert.match(h.session.observationError!, /Observation source unavailable/);
  assert.equal(h.session.state.attempt, undefined);
  assert.equal(h.transport.transportCount('session/subscribe'), 1);
  assert.equal(h.transport.transportCount('turn/start'), 0);
  h.client.close();
});
