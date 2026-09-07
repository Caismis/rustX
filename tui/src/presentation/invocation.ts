import type { ToolInvocationId } from "../protocol/types.ts";

/** Caller correlation is descriptive; it never creates a canonical tool card. */
export function invocationLabel(id: ToolInvocationId): string {
  if (id.caller === "agent") return `call ${id.call_id}`;
  const { block, node, visit } = id.node;
  return `workflow ${block.definition.workflow_id} · ${[...block.definition.blocks, node].join("/")} · ${block.run.invocation}:${visit}`;
}
