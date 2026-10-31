import type { WorkflowAnalysisQuery, WorkflowAnalysisReport, WorkflowConsumerAddress, WorkflowConsumerField, WorkflowExpressionStep } from '../../../../../protocol/app-server/v62';
import { sameAnalysisQuery } from '../../../../../protocol/app-server/workflow-analysis';
import { blockAt, nodeAt, sameDocument, type WorkflowDocument } from './model';
import type { FieldAddress } from './fields';
import { targetKey, type WorkflowScopePort, type WorkflowTarget } from '../port';

/** Translate an authored field address only. Native owns all scope/type facts. */
export function consumerAddress(document: WorkflowDocument, field: FieldAddress): WorkflowConsumerAddress | undefined {
  const block = blockAt(document, field.block), node = block && field.node && nodeAt(block, field.node);
  if (!node || !field.node) return undefined;
  const parts = [...field.path];
  const first = parts.shift();
  let consumer: WorkflowConsumerField;
  const key = () => { const part = parts.shift(); return typeof part === 'object' ? part.key : undefined; };
  if (first === 'messages' && node.type === 'model') {
    // Every variable of a message shares its scope, inserted or not.
    const index = parts.shift(); if (typeof index !== 'number' || !node.messages[index]) return undefined;
    return { address: { block: [...field.block], node: field.node }, field: { kind: 'message', index }, path: [] };
  } else if (first === 'task' && node.type === 'agent') {
    return { address: { block: [...field.block], node: field.node }, field: { kind: 'task' }, path: [] };
  } else if (first === 'arguments' && node.type === 'tool') consumer = { kind: 'arguments' };
  else if (first === 'subject' && node.type === 'review' && parts.shift() === 'value') consumer = { kind: 'review_subject' };
  else if (first === 'context' && node.type === 'review') {
    const index = parts.shift(); if (typeof index !== 'number') return undefined; consumer = { kind: 'review_context', index };
  } else if (first === 'cases' && node.type === 'branch') {
    const index = parts.shift(); if (typeof index !== 'number' || parts.shift() !== 'condition' || !node.cases[index]) return undefined;
    consumer = { kind: 'condition', case_id: node.cases[index].id };
  }
  else if (first === 'output' && node.type === 'return') consumer = { kind: 'return_output' };
  else if (node.type === 'loop' && first === 'loop_variables') {
    const index = parts.shift(); if (typeof index !== 'number' || parts.shift() !== 'value') return undefined; consumer = { kind: 'loop_variable', index };
  } else if (node.type === 'loop' && first === 'break_condition') consumer = { kind: 'break_condition' };
  else if (node.type === 'iteration' && first === 'iterator') consumer = { kind: 'iteration_iterator' };
  else if (node.type === 'iteration' && first === 'output') consumer = { kind: 'iteration_output' };
  else if (node.type === 'assigner' && first === 'items') {
    const index = parts.shift(); if (typeof index !== 'number' || parts.shift() !== 'value') return undefined; consumer = { kind: 'assigner_value', index };
  } else return undefined;
  const path: WorkflowExpressionStep[] = [];
  while (parts.length) {
    const part = parts.shift();
    if (part === 'fields') { const name = key(); if (name === undefined) return undefined; path.push({ kind: 'field', key: name }); }
    else if (part === 'items' || part === 'parts' || part === 'predicates') {
      const index = parts.shift(); if (typeof index !== 'number') return undefined; path.push({ kind: part === 'predicates' ? 'predicate_at' : 'item', index });
    } else if (part === 'value' || part === 'left' || part === 'right' || part === 'predicate') path.push({ kind: part });
    else return undefined;
  }
  return { address: { block: [...field.block], node: field.node }, field: consumer, path };
}

export interface AnalysisInput {
  port: WorkflowScopePort; target: WorkflowTarget; document: WorkflowDocument; generation: number; query?: WorkflowAnalysisQuery;
}

/** Exact request ownership. New queries, generations, documents or authorities
 * revoke old completions even when the underlying transport cannot cancel. */
export class WorkflowAnalysisSession {
  private sequence = 0;
  cancel() { this.sequence++; }
  async request(input: AnalysisInput, current: () => AnalysisInput | undefined): Promise<WorkflowAnalysisReport | undefined> {
    const sequence = ++this.sequence;
    const owns = () => {
      const now = current();
      return sequence === this.sequence && !!now && input.port === now.port && targetKey(input.target) === targetKey(now.target)
        && input.generation === now.generation && input.document === now.document && sameDocument(input.query, now.query);
    };
    let report: WorkflowAnalysisReport;
    try { report = await input.port.analyze(input.target.id, input.document, String(input.generation), input.query); }
    catch (error) { if (owns()) throw error; return undefined; }
    if (!owns() || report.id !== input.target.id || report.generation !== String(input.generation) || !sameAnalysisQuery(report.query, input.query)
      || report.target.kind !== input.port.scope.kind || !/^sha256:[a-f0-9]{64}$/.test(report.candidate_revision)) return undefined;
    return report;
  }
}
