import type { WorkflowVocabulary } from '../../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../../locale/react';
import { CommandRejected, replaceNode } from '../../../document/commands';
import { encodeField, nodeField, type ArrayEdit, type FieldAddress, type FieldStep } from '../../../document/fields';
import { blockAt, nodeAt, sameBlockPath, type BlockPath, type NodeDefinition, type Value, type WorkflowDocument } from '../../../document/model';
import { orderedKeys } from '../../../document/presentation';
import type { WorkflowScopePort } from '../../../port';
import { useCandidateAnalysis } from '../forms/analysis';
import { AddKey, ValueEditor } from '../forms/ValueEditor';
import { Section } from '../forms/fields';
import type { Selection } from '../selection';
import css from '../Editor.module.css';

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

export function Bindings(props: PanelProps) {
  const tx = useTranslation();
  const { document, block, id, edit } = props;
  const node = nodeAt(blockAt(document, block)!, id)!;
  if (node.type !== 'agent' && node.type !== 'model') return null;
  const update = (change: (input: Record<string, Value>) => Record<string, Value>) => edit(document => {
    const current = nodeAt(blockAt(document, block)!, id)!;
    if (current.type !== 'agent' && current.type !== 'model') throw new CommandRejected('missing_node');
    return replaceNode(document, block, id, { ...current, input: change(current.input ?? {}) });
  });
  return <Section title={tx('workflows:inspector.agent-input')}>
    {orderedKeys(document, { kind: 'node_input', address: { block: [...block], node: id } }, Object.keys(node.input ?? {})).map(key => <div key={key} className={css.nestedRow}>
      <ValueEditor label={key} address={nodeField(block, id, ['input', { key }])} value={node.input![key]} edit={edit} />
      <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: key })} onClick={() => update(input => Object.fromEntries(Object.entries(input).filter(([name]) => name !== key)))}>×</button>
    </div>)}
    <AddKey existing={Object.keys(node.input ?? {})} address={nodeField(block, id)} add={(document, key) => {
      const current = nodeAt(blockAt(document, block)!, id)!;
      if (current.type !== 'agent' && current.type !== 'model') throw new CommandRejected('missing_node');
      if (Object.hasOwn(current.input ?? {}, key)) throw new CommandRejected('duplicate_identifier');
      return replaceNode(document, block, id, { ...current, input: { ...current.input, [key]: { type: 'reference', path: ['args'] } } });
    }} />
  </Section>;
}
export function OutputDeclaration({ block, id }: Pick<PanelProps, 'block' | 'id'>) {
  const tx = useTranslation(), analysis = useCandidateAnalysis();
  const report = analysis?.report?.analysis;
  const output = report?.outputs.find(value => value.producer.node === id && sameBlockPath(value.producer.block, block));
  const status = !analysis ? 'loading' : analysis.failed ? 'failed' : !output && report?.truncated ? 'truncated' : 'incomplete';
  return <Section title={tx('workflows:inspector.derived-output')}>
    {output?.schema ? <><pre className={css.codebox} tabIndex={0}>{JSON.stringify(output.schema, null, 2)}</pre><p className={css.note}>{tx('workflows:inspector.derived')}</p></>
      : <p role={status === 'failed' ? 'alert' : 'status'} className={css.note}>{tx(`workflows:inspector.derived-${status}`)}</p>}
  </Section>;
}
