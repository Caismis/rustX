import type { ArrayEdit } from '../../../document/fields';
import { useTranslation } from '../../../../../locale/react';
import { setWorkflow } from '../../../document/commands';
import { setDisplay } from '../../../document/presentation';
import { DEFAULT_WORKFLOW_ICON, IconPicker } from '../../WorkflowIcon';
import { blockField, workflowField } from '../../../document/fields';
import { blockAt, type BlockPath, type WorkflowDocument } from '../../../document/model';
import { Section, TextField } from './fields';
import { InputFields } from './InputFields';
import { SchemaEditor } from './SchemaEditor';
import { NextStep } from '../NextStep';
import type { Selection } from '../selection';
import css from '../Editor.module.css';

type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => void;

/** As Dify's Start panel, the Start node shows only the block's input fields
 * and its next steps. Settings show the Workflow's own fields
 * (root only) and the block's input and output contract. */
export function BlockForm({ document, block, edit, onSelect, start = false }: { document: WorkflowDocument; block: BlockPath; edit: Edit; onSelect: (selection: Selection) => void; start?: boolean }) {
  const tx = useTranslation();
  const owner = blockAt(document, block)!;
  const input = <InputFields label={tx('workflows:inspector.block-input')} value={owner.input} address={blockField(block, ['input'])} edit={edit} />;
  if (start) return <>
    {input}
    <NextStep document={document} block={block} edit={edit} onSelect={onSelect} />
  </>;
  const timeout = document.timeout_ms ?? '';
  const display = document.editor?.display ?? { title: '' };
  const editDisplay = (fields: Partial<typeof display>, session?: string) => edit(current => {
    const previous = current.editor?.display ?? { title: '' }, next = { ...previous, ...fields };
    if (!next.description) delete next.description;
    return setDisplay(current, next);
  }, session);
  return <>
    {block.length === 0 && <Section title={tx('workflows:inspector.display-title')}>
      <TextField label={tx('workflows:inspector.display-name')} value={display.title} field={workflowField('editor.display')}
        invalid={!display.title.trim() && !!document.editor?.display} hint={!display.title.trim() && document.editor?.display ? tx('workflows:dialog.title-required') : tx('workflows:dialog.title-hint')}
        onChange={(title, session) => editDisplay({ title }, session)} />
      <TextField label={tx('workflows:inspector.display-description')} value={display.description ?? ''} multiline field={workflowField('editor.display.description')}
        onChange={(description, session) => editDisplay({ description }, session)} />
      <div className={css.field}>
        <span className={css.fieldLabel}>{tx('workflows:inspector.display-icon')}</span>
        <IconPicker value={display.icon ?? DEFAULT_WORKFLOW_ICON} onChange={icon => editDisplay({ icon })} />
      </div>
    </Section>}
    {block.length === 0 && <Section title={tx('workflows:inspector.workflow-title')}>
      <TextField label={tx('workflows:inspector.workflow-model-description')} hint={tx('workflows:dialog.description-hint')} value={document.description} multiline field={workflowField('description')}
        onChange={(description, session) => edit(current => setWorkflow(current, { description, timeout_ms: current.timeout_ms }), session)} />
      <TextField label={tx('workflows:inspector.workflow-timeout')} hint={tx('workflows:inspector.workflow-timeout-hint')} value={timeout} mono field={workflowField('timeout_ms')}
        onChange={(value, session) => { if (/^\d*$/.test(value)) edit(current => setWorkflow(current, { description: current.description, timeout_ms: value === '' ? undefined : value }), session); }} />
    </Section>}
    <Section title={tx('workflows:inspector.block-title')}>
      {input}
      <SchemaEditor label={tx('workflows:inspector.block-output')} value={owner.output} address={blockField(block, ['output'])} edit={edit} />
    </Section>
  </>;
}
