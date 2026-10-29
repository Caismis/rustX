import { useId } from 'react';
import { useTranslation } from '../../../../../locale/react';
import { encodeField } from '../../../document/fields';
import { ModelSelection } from '../forms/ModelSelection';
import { SchemaEditor } from '../forms/SchemaEditor';
import { JsonField, Section, SelectField, TextField } from '../forms/fields';
import { Bindings, OutputDeclaration, useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

export function ModelPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'model');
  return <>
    <ModelSelection key={encodeField(at('model'))} policy={node.model ?? { mode: 'session' }} address={at('model')} port={props.port} onChange={(model, field) => update(current => ({ ...current, model }), at('model', ...field))} />
    <TextField label={tx('workflows:inspector.model-instruction')} hint={tx('workflows:inspector.model-instruction-hint')} value={node.instruction} multiline invalid={!node.instruction.trim()} field={encodeField(at('instruction'))}
      onChange={(instruction, session) => update(current => ({ ...current, instruction }), at('instruction'), undefined, session)} />
    <Bindings {...props} />
    <Section title={tx('workflows:panel.output')}>
      <SelectField label={tx('workflows:inspector.model-output-mode')} value={node.output.type} options={[{ value: 'text', label: tx('workflows:inspector.model-text') }, { value: 'json_schema', label: tx('workflows:inspector.model-json-schema') }]}
        onChange={type => { if (type !== node.output.type) update(current => ({ ...current, output: type === 'text' ? { type } : { type, schema: { type: 'object', properties: {}, required: [], additionalProperties: false } } }), at('output')); }} />
      {node.output.type === 'json_schema' ? <SchemaEditor label={tx('workflows:inspector.model-output-schema')} value={node.output.schema} address={at('output', 'schema')} edit={props.edit} /> : <p className={css.note}>{tx('workflows:inspector.model-text-hint')}</p>}
    </Section>
    <OutputDeclaration {...props} />
  </>;
}

export function AgentPanel(props: PanelProps) {
  const tx = useTranslation(), listId = useId(), { node, at, update } = useNode(props, 'agent');
  return <>
    <TextField label={tx('workflows:inspector.agent-profile')} hint={tx('workflows:inspector.agent-profile-hint')} value={node.profile} list={listId} invalid={!node.profile} field={encodeField(at('profile'))}
      onChange={(profile, session) => update(current => ({ ...current, profile }), at('profile'), undefined, session)} />
    <datalist id={listId}>{props.vocabulary?.profiles.map(name => <option key={name} value={name} />)}</datalist>
    <TextField label={tx('workflows:inspector.agent-task')} value={node.task} multiline field={encodeField(at('task'))} onChange={(task, session) => update(current => ({ ...current, task }), at('task'), undefined, session)} />
    <Bindings {...props} />
    <SchemaEditor label={tx('workflows:inspector.agent-output')} value={node.output} address={at('output')} edit={props.edit} />
    <details className={css.advanced}><summary>{tx('workflows:inspector.agent-override')}</summary>
      <label className={css.check}><input type="checkbox" checked={node.override !== undefined} onChange={event => { const enabled = event.target.checked; update(current => { const { override: _previous, ...rest } = current; return enabled ? { ...rest, override: {} } : rest; }, at('override')); }} />{tx('workflows:inspector.override-enabled')}</label>
      {node.override !== undefined && <JsonField label={tx('workflows:inspector.agent-override')} value={node.override} address={at('override')} />}
      <p className={css.note}>{tx('workflows:inspector.agent-override-hint')}</p>
    </details>
  </>;
}
