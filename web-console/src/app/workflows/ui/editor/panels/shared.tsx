import type { WorkflowVocabulary } from '../../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../../locale/react';
import { CommandRejected, replaceNode } from '../../../document/commands';
import { encodeField, nodeField, type ArrayEdit, type FieldAddress, type FieldStep } from '../../../document/fields';
import { blockAt, nodeAt, sameBlockPath, type BlockPath, type NodeDefinition, type WorkflowDocument } from '../../../document/model';
import { schemaObject } from '../../../document/schema';
import type { WorkflowScopePort } from '../../../port';
import { useCandidateAnalysis } from '../forms/analysis';
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

export function OutputDeclaration({ block, id }: Pick<PanelProps, 'block' | 'id'>) {
  const tx = useTranslation(), analysis = useCandidateAnalysis();
  const report = analysis?.report?.analysis;
  const output = report?.outputs.find(value => value.producer.node === id && sameBlockPath(value.producer.block, block));
  const status = !analysis ? 'loading' : analysis.failed ? 'failed' : !output && report?.truncated ? 'truncated' : 'incomplete';
  const schema = schemaObject(output?.schema), fields = schemaObject(schema?.properties), entries = Object.entries(fields ?? {});
  return <Section title={tx('workflows:inspector.derived-output')}>
    {output?.schema ? <>
      <div className={css.outputVariables}>
        {entries.length ? entries.slice(0, 8).map(([name, value]) => <div key={name} className={css.outputVariable}><span aria-hidden>{'{x}'}</span><b>{name}</b><code>{String(schemaObject(value)?.type ?? '?')}</code></div>)
          : <div className={css.outputVariable}><span aria-hidden>{'{x}'}</span><b>{tx('workflows:card.output')}</b><code>{String(schema?.type ?? '?')}</code></div>}
        {entries.length > 8 && <small className={css.note}>{tx('workflows:card.more', { p0: String(entries.length - 8) })}</small>}
      </div>
      <details className={css.outputSchema}><summary>{tx('workflows:schema.json')}</summary><pre className={css.codebox} tabIndex={0}>{JSON.stringify(output.schema, null, 2)}</pre></details>
      <p className={css.note}>{tx('workflows:inspector.derived')}</p>
    </>
      : <p role={status === 'failed' ? 'alert' : 'status'} className={css.note}>{tx(`workflows:inspector.derived-${status}`)}</p>}
  </Section>;
}
