import type { ArrayEdit } from '../../../document/fields';
import { useTranslation } from '../../../../../locale/react';
import { setEntry, setWorkflow } from '../../../document/commands';
import { blockField, workflowField } from '../../../document/fields';
import { blockAt, nodeAt, type BlockPath, type WorkflowDocument } from '../../../document/model';
import { JsonField, Section, SelectField, TextField } from './fields';

type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => void;

/** The Workflow's own fields (root only) and one block's contract: entry,
 * input schema and output schema. */
export function BlockForm({ document, block, edit }: { document: WorkflowDocument; block: BlockPath; edit: Edit }) {
  const tx = useTranslation();
  const owner = blockAt(document, block)!;
  const timeout = document.timeout_ms ?? '';
  return <>
    {block.length === 0 && <Section title={tx('workflows:inspector.workflow-title')}>
      <TextField label={tx('workflows:inspector.workflow-description')} value={document.description} multiline field={workflowField('description')}
        onChange={(description, session) => edit(current => setWorkflow(current, { description, timeout_ms: current.timeout_ms }), session)} />
      <TextField label={tx('workflows:inspector.workflow-timeout')} hint={tx('workflows:inspector.workflow-timeout-hint')} value={timeout} mono field={workflowField('timeout_ms')}
        onChange={(value, session) => { if (/^\d*$/.test(value)) edit(current => setWorkflow(current, { description: current.description, timeout_ms: value === '' ? undefined : value }), session); }} />
    </Section>}
    <Section title={tx('workflows:inspector.block-title')}>
      <SelectField label={tx('workflows:inspector.block-entry')} value={nodeAt(owner, owner.entry) ? owner.entry : ''}
        options={[...(nodeAt(owner, owner.entry) ? [] : [{ value: '', label: owner.entry || '—' }]), ...Object.keys(owner.nodes).sort().map(id => ({ value: id, label: id }))]}
        onChange={id => { if (id) edit(current => setEntry(current, block, id)); }} />
      <JsonField label={tx('workflows:inspector.block-input')} hint={tx('workflows:inspector.schema-hint')} value={owner.input} rows={8} address={blockField(block, ['input'])} />
      <JsonField label={tx('workflows:inspector.block-output')} hint={tx('workflows:inspector.schema-hint')} value={owner.output} rows={8} address={blockField(block, ['output'])} />
    </Section>
  </>;
}
