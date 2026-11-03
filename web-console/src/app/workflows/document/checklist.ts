import type { WorkflowCheckReport, WorkflowDependencyFailure } from '../../../../../protocol/app-server/v63';
import { locate } from './diagnostics';
import { nodePresentation } from './geometry';
import { blockAt, blockKey, childSteps, edgesOf, isContainer, nodeAt, startOf, type Block, type BlockPath, type NodeDefinition, type Value, type WorkflowDocument } from './model';

/** One problem, as Dify's checklist shows it: what is wrong, and the field
 * a go-to-fix focuses. */
export interface ChecklistMessage {
  /** A required field, by its Dify label key, a native reason as written,
   * or a typed dependency failure the editor words. */
  readonly text: { kind: 'required'; field: ChecklistField } | { kind: 'unconnected' } | { kind: 'native'; reason: string } | { kind: 'dependency'; failure: WorkflowDependencyFailure };
  readonly field?: string;
}
export type ChecklistField = 'model' | 'prompt' | 'agent' | 'task' | 'tool' | 'input' | 'output' | 'assigned' | 'variable' | 'value' | 'if' | 'elif';

/** One node with problems, or the workflow itself (`node` absent). */
export interface ChecklistItem {
  readonly block: BlockPath;
  readonly node?: string;
  readonly type?: NodeDefinition['type'];
  readonly title?: string;
  readonly messages: readonly ChecklistMessage[];
}

const empty = (value: Value | null | undefined) => !value || value.type === 'reference' && value.path.length === 0;
const required = (field: ChecklistField, path: string): ChecklistMessage => ({ text: { kind: 'required', field }, field: path });

/** As each Dify node's `checkValid`: the first empty required field of
 * one node. Each rule is one the native compiler also rejects, so this list
 * only names more problems at once than the compiler's first. */
function nodeProblems(node: NodeDefinition): ChecklistMessage[] {
  switch (node.type) {
    case 'model': {
      if (node.model?.mode === 'explicit' && !node.model.model) return [required('model', 'model.model')];
      const index = node.messages.findIndex(message => message.role !== 'system' && !message.content?.length);
      return index < 0 ? [] : [required('prompt', `messages.${index}.content`)];
    }
    case 'agent': return !node.profile ? [required('agent', 'profile')] : !node.task.length ? [required('task', 'task')] : [];
    case 'tool': return !node.selector.name || node.selector.origin === 'source' && !node.selector.source_id ? [required('tool', 'selector')] : [];
    case 'branch': {
      const index = node.cases.findIndex(item => (item.condition.type === 'and' || item.condition.type === 'or') && !item.condition.predicates.length);
      return index < 0 ? [] : [required(index ? 'elif' : 'if', `cases.${index}.condition`)];
    }
    case 'iteration': return empty(node.iterator) ? [required('input', 'iterator')] : empty(node.output) ? [required('output', 'output')] : [];
    case 'assigner': {
      const index = node.items.findIndex(item => !item.variable.length);
      return !node.items.length ? [required('assigned', 'items')] : index < 0 ? [] : [required('assigned', `items.${index}.variable`)];
    }
    case 'return': {
      const index = node.outputs.findIndex(output => !output.variable || !output.value_selector.length);
      return index < 0 ? [] : [required(node.outputs[index].variable ? 'value' : 'variable', `outputs.${index}`)];
    }
    default: return [];
  }
}

/** Nodes a run of the block can reach from its Start, over every exit. */
function reachable(block: Block): Set<string> {
  const start = startOf(block), seen = new Set<string>(start ? [start] : []), queue = start ? [start] : [];
  for (let id = queue.shift(); id !== undefined; id = queue.shift())
    for (const edge of edgesOf(block)) if (edge.from === id && !seen.has(edge.to)) { seen.add(edge.to); queue.push(edge.to); }
  return seen;
}

/** Dify's checklist of one document: every node's own problems, then the
 * native check's, each node once and in canvas order. `report` is the
 * native check of exactly this document, when it has completed. */
export function checklist(document: WorkflowDocument, report?: WorkflowCheckReport): ChecklistItem[] {
  const items = new Map<string, { block: BlockPath; node?: string; type?: NodeDefinition['type']; title?: string; messages: ChecklistMessage[] }>();
  const keyOf = (block: BlockPath, node?: string) => `${blockKey(block)}:${node ?? ''}`;
  const itemFor = (block: BlockPath, node?: string) => {
    const key = keyOf(block, node);
    let item = items.get(key);
    if (!item) {
      const definition = node === undefined ? undefined : lookup(document, block, node);
      item = { block, node, type: definition?.type, title: node === undefined ? undefined : nodePresentation(document, block, node)?.title || undefined, messages: [] };
      items.set(key, item);
    }
    return item;
  };
  const walk = (block: Block, path: BlockPath) => {
    const live = reachable(block);
    for (const [id, node] of Object.entries(block.nodes)) {
      const own = nodeProblems(node);
      if (!live.has(id)) own.push({ text: { kind: 'unconnected' } });
      if (own.length) itemFor(path, id).messages.push(...own);
      for (const step of childSteps(id, node)) if (isContainer(node)) walk(node.body, [...path, step]);
    }
  };
  walk(document.block, []);
  // The native check names its first problem. A node already listed keeps
  // its own messages: they name the same problem in the author's words.
  const native = (path: string, text: ChecklistMessage['text']) => {
    const location = locate(path), key = keyOf(location.block, location.node);
    if (location.node && !lookup(document, location.block, location.node)) return;
    if (items.get(key)?.messages.length) return;
    itemFor(location.block, location.node).messages.push({ text, field: location.field });
  };
  if (report?.structure.status === 'invalid') native(report.structure.diagnostic.path, { kind: 'native', reason: report.structure.diagnostic.reason });
  if (report?.dependencies.status === 'blocked') for (const item of report.dependencies.diagnostics) native(item.path, { kind: 'dependency', failure: item.reason });
  return [...items.values()].filter(item => item.messages.length);
}

function lookup(document: WorkflowDocument, block: BlockPath, id: string): NodeDefinition | undefined {
  const owner = blockAt(document, block);
  return owner && nodeAt(owner, id);
}
