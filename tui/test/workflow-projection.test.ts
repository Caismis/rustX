import assert from "node:assert/strict";
import { it } from "node:test";
import { readFileSync } from "node:fs";
import { isKnownRuntimeClientEvent } from "../src/protocol/types.ts";
import type { WorkflowRunView, WorkflowInstanceView } from "../src/protocol/types.ts";
import { reduce, replaceFromSnapshot } from "../src/presentation/projection.ts";
import { workflowDetails, workflowStatus } from "../src/ui/components/workflow-details.ts";
import { snapshot, runtimeCursor } from "./support/fixtures.ts";

const id = { conversation_id: "conv-test", attempt_id: "attempt", invocation: 1 };
it("Rust v22 fixture is accepted at the transport boundary and folded unchanged", () => {
  const event: unknown = JSON.parse(readFileSync(new URL("../../tests/fixtures/runtime-client/workflow-v22.json", import.meta.url), "utf8"));
  assert.ok(isKnownRuntimeClientEvent(event));
  assert.equal(event.type, "workflows_updated");
  if (event.type !== "workflows_updated") throw new Error("fixture kind");
  const state = reduce(replaceFromSnapshot(snapshot(), runtimeCursor(0)), { cursor: runtimeCursor(1), event });
  assert.deepEqual(state.workflows, event.workflows);
  assert.equal(state.workflows.runs[0]!.instances[0]!.checks_passed, false);
  assert.deepEqual(state.workflows.runs[0]!.instances[0]!.block.invocations, [0, 2]);
});
function row(node: string, branch: string): WorkflowInstanceView {
  return { block: { run: id, definition: { workflow_id: "review", blocks: ["parallel", branch] }, invocations: [0, 0] },
    node, visit: 0, kind: "review", state: { type: "waiting", reason: "review" }, child: null, invocation: null, tool_id: null,
    interaction: { conversation_id: "conv-test", interaction_id: node }, iteration: null, iterations_max: null,
    loop_exit: null, candidate: { run: id, version: 2, content: "old" }, checks_passed: null, review_accepted: true };
}
function run(): WorkflowRunView {
  return { id, workflow_id: "review", program_digest: "a".repeat(64), resource_revision: 1, tool_call_id: "outer",
    state: { type: "running" }, instances: [row("human", "left"), { ...row("worker", "right"), state: { type: "running" }, child: "child-right", interaction: null, review_accepted: null }],
    omitted_instances: 0, steps_consumed: 4, steps_max: 20, agents_consumed: 1, candidate_users: 0,
    candidate: { run: id, version: 3, content: "new" }, handoff: { state: "retained", path: "/candidate", truncated: false } };
}

it("native replacement event and reconnect snapshot converge without canonical writes", () => {
  const before = replaceFromSnapshot(snapshot(), runtimeCursor(0));
  const workflows = { revision: 9, runs: [run()], omitted_runs: 2 };
  const after = reduce(before, { cursor: runtimeCursor(1), event: { type: "workflows_updated", workflows } });
  assert.deepEqual(after, replaceFromSnapshot(snapshot({ workflows }), runtimeCursor(1)));
  assert.deepEqual(after.transcript, before.transcript);
  assert.deepEqual(after.pendingInteractions, before.pendingInteractions);
  const frozen = JSON.stringify(after);
  for (let i = 0; i < 50; i++) workflowDetails(after.workflows.runs[0]!);
  assert.equal(JSON.stringify(after), frozen, "expand/focus formatting cannot mutate any native fact");
});

it("Parallel human wait and child coexist and old acceptance is historical", () => {
  const text = workflowDetails(run()).join("\n");
  assert.match(text, /human · waiting for review/);
  assert.match(text, /worker · running.*child-right/);
  assert.match(text, /candidate v2 · historical/);
  assert.match(text, /candidate v3/);
  assert.match(text, /root HITL/);
});

it("draining is not terminal cancellation and Loop exhaustion is not business acceptance", () => {
  assert.match(workflowStatus({ type: "draining" }), /requested.*draining/);
  assert.doesNotMatch(workflowStatus({ type: "draining" }), /cancelled/);
  assert.equal(workflowStatus({ type: "settled", outcome: "cancelled" }), "execution cancelled");
  const value = run();
  value.state = { type: "settled", outcome: "completed" };
  value.instances = [{ ...row("repair", "body"), kind: "loop", state: { type: "settled", outcome: "completed" }, interaction: null,
    review_accepted: null, checks_passed: false, iteration: 3, iterations_max: 3, loop_exit: "exhausted" }];
  const text = workflowDetails(value).join("\n");
  assert.match(text, /execution completed/);
  assert.match(text, /iteration 3\/3 · exhausted.*business checks failed/);
  assert.doesNotMatch(text, /task accepted|✓/);
});

it("nested concrete blocks render under their owner and truncated orphans stay visible", () => {
  const value = run();
  const root = { run: id, definition: { workflow_id: "review", blocks: [] }, invocations: [0] };
  const fork = { ...row("parallel", "unused"), block: root, kind: "parallel" as const };
  const human = row("human", "left");
  const repair = { ...row("repair", "right"), kind: "loop" as const };
  const iteration = { ...row("fix", "unused"), block: {
    ...root, definition: { workflow_id: "review", blocks: ["parallel", "right", "repair", "body"] }, invocations: [0, 0, 2],
  } };
  // Deliberately store children after an unrelated successor, like native admission order.
  value.instances = [fork, { ...fork, node: "final", kind: "return" }, human, repair, iteration];
  const text = workflowDetails(value).join("\n");
  assert.ok(text.indexOf("parallel ·") < text.indexOf("human ·"));
  assert.ok(text.indexOf("repair ·") < text.indexOf("fix ·"));
  assert.ok(text.indexOf("fix ·") < text.indexOf("final ·"));
  assert.match(text, /\n {12}fix ·/);
  value.instances = [iteration];
  value.omitted_instances = 4;
  assert.match(workflowDetails(value).join("\n"), /fix ·[\s\S]*4 execution instances omitted/);
});
