import type { WorkflowVocabulary } from '../../../../../../../protocol/app-server/v73';
import { CommandRejected, replaceNode } from '../../../document/commands';
import { encodeField, nodeField, type ArrayEdit, type FieldAddress, type FieldStep } from '../../../document/fields';
import { blockAt, nodeAt, type BlockPath, type NodeDefinition, type WorkflowDocument } from '../../../document/model';
import type { WorkflowScopePort } from '../../../port';
import type { Selection } from '../selection';

export type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => void;
export interface PanelProps { document: WorkflowDocument; block: BlockPath; id: string; edit: Edit; port: WorkflowScopePort; vocabulary?: WorkflowVocabulary; navigate: (path: BlockPath) => void; onSelect: (selection: Selection) => void }
type Of<K extends NodeDefinition['type']> = Extract<NodeDefinition, { type: K }>;
export function useNode<K extends NodeDefinition['type']>(props: PanelProps, kind: K) {
  const { document, block, id, edit } = props;
  const node = nodeAt(blockAt(document, block)!, id) as Of<K>;
  const at = (...path: FieldStep[]) => nodeField(block, id, path);
  const update = (change: (node: Of<K>) => Of<K>, field: FieldAddress, array?: ArrayEdit, session?: string) => edit(current => {
    const owner = blockAt(current, block), node = owner && nodeAt(owner, id);
    if (node?.type !== kind) throw new CommandRejected('missing_node');
    return replaceNode(current, block, id, change(node as Of<K>));
  }, array ? undefined : session ?? encodeField(field), array);
  return { node, at, update };
}
