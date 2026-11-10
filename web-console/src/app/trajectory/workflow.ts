import type { CheckpointReadCut, TraceRecord, WorkflowTraceNode } from '../../../../protocol/app-server/v78';
export type { WorkflowTraceNode } from '../../../../protocol/app-server/v78';
import { sameTraceStep } from '../../../../protocol/app-server/trace-identity';
import type { TraceFieldReader } from '../../client/trace';
import type { InspectableDisplayItem, TrajectoryLedgerRow } from './layout';

export interface WorkflowNodes {
  root: TraceRecord; nodes: WorkflowTraceNode[]; cut: CheckpointReadCut; reader: TraceFieldReader; loading: boolean; error?: string;
}
export const isWorkflowCall = (record: TraceRecord) => record.kind === 'tool' && record.tool?.tool_id?.startsWith('tool-workflow-') === true;
export const workflowCollection = (root: TraceRecord) => root.collections.find(value => value.domain.endsWith('.workflow.nodes'));

/** Native call identity and complete Step scope, never a displayed name or adjacency. */
export function workflowCalls(records: readonly TraceRecord[]): Map<string, TraceRecord> {
  const result = new Map<string, TraceRecord>();
  for (const call of records.filter(isWorkflowCall)) {
    const matches = records.filter(root => root.kind === 'workflow' && root.originating_tool_call_id != null
      && root.originating_tool_call_id === call.tool?.call_id && sameTraceStep(root.location, call.location));
    if (matches.length === 1) result.set(call.id, matches[0]!);
  }
  // Scheduled executions have no outer Tool and keep their own real entry.
  for (const root of records) if (root.kind === 'workflow' && root.originating_tool_call_id == null && workflowCollection(root)) result.set(root.id, root);
  return result;
}

export function withWorkflowNodes(rows: TrajectoryLedgerRow[], calls: ReadonlyMap<string, TraceRecord>, opened: ReadonlySet<string>, loaded: ReadonlyMap<string, WorkflowNodes>): TrajectoryLedgerRow[] {
  return rows.flatMap(row => {
    const root = row.item && calls.get(row.item.owner_record_id);
    if (!root || !opened.has(root.id)) return [row];
    return [row, ...(loaded.get(root.id)?.nodes ?? []).map((node, index, nodes) => {
      const item: InspectableDisplayItem = { type: 'WorkflowNode', record: root, owner_record_id: root.id, parent_record_id: row.item!.owner_record_id,
        display_key: JSON.stringify(['workflow-node', root.id, node.id]), label: node.instance.node, preview: node.output_preview ?? '', facet: 'overview',
        workflowNode: node, workflowSource: loaded.get(root.id)! };
      return { display_key: item.display_key, kind: 'semantic' as const, height: 30 as const, turn: row.turn, turnStart: false, workflowLast: index === nodes.length - 1, item };
    })];
  });
}
