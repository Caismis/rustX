/** Presentation only: native instances, never inferred graph progression. */
import type { WorkflowRunView, WorkflowState, WorkflowInstanceView } from "../../protocol/app-server.ts";

/** Layout by native parent identity; this neither chooses nor follows control edges. */
function treeRows(rows: WorkflowInstanceView[]): Array<[WorkflowInstanceView, number]> {
  const groups = new Map<string, WorkflowInstanceView[]>();
  for (const row of rows) {
    const key = JSON.stringify(row.block);
    const group = groups.get(key) ?? [];
    group.push(row);
    groups.set(key, group);
  }
  const rendered = new Set<string>();
  const result: Array<[WorkflowInstanceView, number]> = [];
  function block(key: string, depth: number): void {
    if (rendered.has(key)) return;
    rendered.add(key);
    for (const row of groups.get(key) ?? []) {
      result.push([row, depth + (row.node === null ? 0 : 1)]);
      if (row.node === null) continue;
      for (const [childKey, childRows] of groups) {
        const child = childRows[0]!.block;
        if (child.definition.blocks.at(-2) !== row.node) continue;
        const parent = { ...child, definition: { ...child.definition, blocks: child.definition.blocks.slice(0, -2) }, invocations: child.invocations.slice(0, -1) };
        if (JSON.stringify(parent) === key) block(childKey, depth + 2);
      }
    }
  }
  for (const [key, group] of groups) if (group[0]!.block.definition.blocks.length === 0) block(key, 0);
  // Native truncation may omit a parent; retain every remaining child row.
  for (const key of groups.keys()) block(key, 0);
  return result;
}

export function workflowStatus(state: WorkflowState): string {
  switch (state.type) {
    case "pending": return "pending admission";
    case "running": return "running";
    case "draining": return "cancellation requested · draining owned work";
    case "waiting": return `waiting for ${state.reason}`;
    case "settled": return `execution ${state.outcome.replaceAll("_", " ")}`;
  }
}

export function workflowDetails(run: WorkflowRunView): string[] {
  const lines = [
    `${run.workflow_id} · ${workflowStatus(run.state)}`,
    `admitted program ${run.program_digest} · resources ${run.resource_revision}`,
    `run ${run.id.origin.type === 'tool' ? run.id.origin.attempt_id : run.id.origin.start_id}/${run.id.invocation} · steps ${run.steps_consumed}/${run.steps_max} · agents ${run.agents_consumed} · models ${run.model_calls_consumed}`,
  ];
  for (const [row, depth] of treeRows(run.instances)) {
    const path = row.block.definition.blocks;
    const prefix = "  ".repeat(depth + 1);
    const label = row.node ?? `block ${path.join("/") || "root"} [${row.block.invocations.join("/")}]`;
    let text = `${prefix}${label} · ${workflowStatus(row.state)}`;
    if (row.iterations_max != null) text += ` · iteration ${row.iteration ?? 0}/${row.iterations_max}`;
    if (row.loop_exit != null) text += ` · ${row.loop_exit}`;
    if (row.child != null) text += ` · child ${row.child} (read-only child inspector)`;
    if (row.auxiliary_call_id != null) text += ` · Auxiliary Call ${row.auxiliary_call_id}`;
    if (row.tool_id != null) text += ` · Tool ${row.tool_id} · visit ${row.visit}`;
    if (row.interaction != null) text += ` · interaction ${row.interaction.interaction_id} (root HITL)`;
    if (row.checks_passed != null) text += ` · business checks ${row.checks_passed ? "passed" : "failed"}`;
    if (row.review_accepted != null) text += ` · Review ${row.review_accepted ? "accepted" : "rejected"}`;
    lines.push(text);
  }
  if (run.omitted_instances > 0) lines.push(`${run.omitted_instances} execution instances omitted by native retention`);
  lines.push("Human responses use the root HITL queue. Cancel stops the foreground attempt.");
  return lines;
}
