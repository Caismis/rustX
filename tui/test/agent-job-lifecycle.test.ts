import assert from "node:assert/strict";
import { test } from "node:test";
import { describeRpcError } from "../src/protocol/app-server.ts";
import { METHOD_RESPONSE_LOSS_CLASS } from "../src/app-server/client.ts";
import { reduce, replaceFromSnapshot } from "../src/presentation/projection.ts";
import { cycleSubagentSelection, hasSubagentSelection } from "../src/ui/subagent-navigation.ts";
import { renderSubagentDetail } from "../src/ui/components/activity.ts";
import { backgroundExecution, runtimeCursor, snapshot, subagent } from "./support/fixtures.ts";
import { harness, nextRequest } from "./support/app-server-harness.ts";

test("one selected Agent survives active, stopping, inactive, resume and reconnect", () => {
  const first = subagent("reviewer", "frozen-profile", "active", {
    agent_id: "agent-stable", activation_id: "activation-a", current_activation: "activation-a",
  });
  let state = replaceFromSnapshot(snapshot({ agents: [first] }), runtimeCursor(1));
  const selected = cycleSubagentSelection(state.agents, undefined, 1);
  const stages = [
    { ...first, state: "stopping" as const, activation_state: "stopping" as const },
    { ...first, state: "inactive" as const, current_activation: null, activation_state: "interrupted" as const },
    { ...first, activation_id: "activation-b", current_activation: "activation-b" },
  ];
  for (const [index, agent] of stages.entries()) {
    state = reduce(state, { cursor: runtimeCursor(index + 2), event: { type: "agent_updated", agent } });
    assert.equal(state.agents.length, 1);
    assert.equal(hasSubagentSelection(state.agents, selected), true);
    assert.equal(state.agents[0]?.child_conversation_id, first.child_conversation_id);
    assert.equal(state.agents[0]?.profile_digest, first.profile_digest);
  }
  const reconnected = replaceFromSnapshot(snapshot({ agents: [stages[2]!] }), runtimeCursor(4));
  assert.deepEqual(reconnected.agents, state.agents);
  assert.equal(hasSubagentSelection(reconnected.agents, selected), true);
  assert.match(renderSubagentDetail(state.agents[0]!), /activation-b/);
});

test("one finite Job projects active to terminal and reconstructs terminal on reconnect", () => {
  const job = backgroundExecution("exec_c8536561-1a50-7edc-a396-b3a459465efb", "running");
  let state = replaceFromSnapshot(snapshot({ jobs: [job] }), runtimeCursor(1));
  const terminal = { ...job, state: "succeeded" as const };
  state = reduce(state, { cursor: runtimeCursor(2), event: { type: "job_updated", job: terminal } });
  assert.deepEqual(state.jobs, [terminal]);
  assert.deepEqual(replaceFromSnapshot(snapshot({ jobs: [terminal] }), runtimeCursor(2)).jobs, state.jobs);
});

for (const state of ["active", "inactive"] as const) {
  test(`send-message with ${state} projection invokes the same atomic owner operation`, async () => {
    const h = await harness(snapshot({ agents: [subagent("worker", "frozen", state)] }));
    const sending = h.dispatcher.submit("/send-message agent-child follow up");
    const request = await nextRequest(h, "agent/sendMessage");
    assert.deepEqual(request.params, { target: h.target, agent_id: "agent-child", message: "follow up" });
    h.transport.respond(request.id, { type: "agent_message", agent_id: "agent-child", activation_id: "activation-next", resumed: state === "inactive" });
    await sending;
    assert.equal(h.transport.transportCount("agent/status"), 0);
    assert.equal(h.transport.transportCount("turn/start"), 0);
    h.client.close();
  });
}

test("wait-agent retains the returned captured activation while an Agent has resumed", async () => {
  const h = await harness();
  const waiting = h.session.waitAgent("agent-child");
  const request = await nextRequest(h, "agent/wait");
  const resumed = subagent("worker", "frozen", "active", { activation_id: "activation-b", current_activation: "activation-b" });
  h.session.updateState(state => ({ ...state, agents: [resumed] }));
  h.transport.respond(request.id, { type: "agent_wait", agent_id: "agent-child", activation_id: "activation-a", outcome: "succeeded", agent: resumed });
  const settled = await waiting;
  assert.equal(settled.activation_id, "activation-a");
  assert.equal(h.session.state.agents[0]?.current_activation, "activation-b");
  assert.equal(h.transport.transportCount("agent/wait"), 1);
  h.client.close();
});

test("response loss cannot silently retarget an Agent wait", () => {
  assert.equal(METHOD_RESPONSE_LOSS_CLASS["agent/wait"], "side_effecting");
  assert.equal(METHOD_RESPONSE_LOSS_CLASS["job/wait"], "read");
});

test("interrupt forwards stable identity and never removes the resumable Agent row", async () => {
  const agent = subagent("worker", "frozen");
  const h = await harness(snapshot({ agents: [agent] }));
  const interrupting = h.dispatcher.submit("/interrupt-agent agent-child");
  const request = await nextRequest(h, "agent/interrupt");
  assert.deepEqual(request.params, { target: h.target, agent_id: "agent-child" });
  h.transport.respond(request.id, { type: "agent_wait", agent_id: agent.agent_id, activation_id: agent.activation_id, outcome: "cancelled", agent: { ...agent, state: "inactive", current_activation: null, activation_state: "cancelled" } });
  await interrupting;
  assert.deepEqual(h.session.state.agents, [agent], "control responses do not invent lifecycle events");
  h.client.close();
});

for (const [command, method] of [["/job-status", "job/status"], ["/job-wait", "job/wait"], ["/job-cancel", "job/cancel"]] as const) {
  test(`${command} addresses one finite Job without optimistic lifecycle changes`, async () => {
    const job = backgroundExecution("exec_c8536561-1a50-7edc-a396-b3a459465efb", "running");
    const h = await harness(snapshot({ jobs: [job] }));
    const controlling = h.dispatcher.submit(`${command} ${job.job_id}`);
    const request = await nextRequest(h, method);
    assert.deepEqual(request.params, { target: h.target, job_id: job.job_id });
    assert.deepEqual(h.session.state.jobs, [job]);
    h.transport.respond(request.id, { type: "job", job: { ...job, state: "cancelled" } });
    await controlling;
    assert.deepEqual(h.session.state.jobs, [job], "only owner notifications or snapshots update lifecycle projection");
    h.client.close();
  });
}


test("Stopping rejection is transient and leaves resume admission to the owner", () => {
  assert.match(describeRpcError({ code: -32000, message: "stopping", data: { kind: "agent_stopping", agent_id: "agent-child" } }), /retry after it becomes inactive/);
});

for (const outcome of ["cancelled", "succeeded"] as const) {
  test(`interrupt captures ${outcome} on A without overwriting resumed B`, async () => {
    const first = subagent("worker", "frozen", "active", { activation_id: "activation-a", current_activation: "activation-a" });
    const h = await harness(snapshot({ agents: [first] }));
    const interrupting = h.session.interruptAgent(first.agent_id);
    const request = await nextRequest(h, "agent/interrupt");
    const resumed = { ...first, activation_id: "activation-b", current_activation: "activation-b" };
    h.session.updateState(state => ({ ...state, agents: [resumed] }));
    h.transport.respond(request.id, { type: "agent_wait", agent_id: first.agent_id, activation_id: "activation-a", outcome, agent: resumed });
    const result = await interrupting;
    assert.equal(result.activation_id, "activation-a");
    assert.equal(result.outcome, outcome);
    assert.equal(h.session.state.agents[0]?.current_activation, "activation-b");
    h.client.close();
  });
}

for (const command of ["/wait-agent", "/interrupt-agent"] as const) {
  test(`${command} distinguishes a rolled-back admission from a committed activation`, async () => {
    const agent = subagent("worker", "frozen", "admitting", { current_activation: "reserved-b" });
    const h = await harness(snapshot({ agents: [agent] }));
    const controlling = h.dispatcher.submit(`${command} agent-child`);
    const request = await nextRequest(h, command === "/wait-agent" ? "agent/wait" : "agent/interrupt");
    h.transport.respond(request.id, { type: "agent_wait", agent_id: agent.agent_id, activation_id: "reserved-b", outcome: null, agent: { ...agent, state: "inactive", current_activation: null } });
    assert.match(JSON.stringify(await controlling), /ended before an activation committed/);
    assert.deepEqual(h.session.state.agents, [agent]);
    h.client.close();
  });
}
