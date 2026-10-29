import { useId, useState } from 'react';
import clsx from 'clsx';
import { IconQuestionOutline14, IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { Switch } from '../../../../../presentation/primitives/Switch';
import { Tooltip } from '../../../../../presentation/primitives/Tooltip';
import { CommandRejected, replaceNode } from '../../../document/commands';
import { blockAt, nodeAt } from '../../../document/model';
import { orderedKeys } from '../../../document/presentation';
import { AddKey, ValueEditor } from '../forms/ValueEditor';
import { PromptCard } from '../forms/PromptCard';
import { ReferencePicker } from '../forms/ReferencePicker';
import { useTranslation } from '../../../../../locale/react';
import { encodeField } from '../../../document/fields';
import { ModelSelection } from '../forms/ModelSelection';
import { SchemaEditor } from '../forms/SchemaEditor';
import { JsonField, Section, TextField } from '../forms/fields';
import { Bindings, OutputDeclaration, useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

export function ModelPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'model');
  const [session, setSession] = useState(0), [expanded, setExpanded] = useState(false);
  const field = encodeField(at('instruction')), input = node.input ?? {};
  const keys = orderedKeys(props.document, { kind: 'node_input', address: { block: [...props.block], node: props.id } }, Object.keys(input));
  const structured = node.output.type === 'json_schema';
  return <>
    {/* As Dify's LLM panel: the model and its prompt messages form one block. */}
    <section className={css.group}>
      <ModelSelection key={encodeField(at('model'))} policy={node.model ?? { mode: 'session' }} address={at('model')} port={props.port} onChange={(model, field) => update(current => ({ ...current, model }), at('model', ...field))} />
      {/* As Dify's prompt messages: the instruction is the system prompt, and
          the bound variables are sent as the user message. */}
      <div className={css.promptMessages}>
        <PromptCard role={/* i18n-raw: model message role, as Dify shows it */ 'SYSTEM'} hint={tx('workflows:prompt.system-hint')} text={node.instruction} expanded={expanded} onExpand={() => setExpanded(value => !value)}>
          <textarea data-workflow-field={field} aria-label={tx('workflows:inspector.model-instruction')} aria-invalid={!node.instruction.trim() || undefined}
            className={clsx(css.promptInput, expanded && css.promptInputExpanded)} value={node.instruction} placeholder={tx('workflows:prompt.system-placeholder')}
            onFocus={() => setSession(value => value + 1)} onChange={event => { const instruction = event.target.value; update(current => ({ ...current, instruction }), at('instruction'), undefined, `${field}:${session}`); }} />
        </PromptCard>
        <PromptCard role={/* i18n-raw: model message role, as Dify shows it */ 'USER'} hint={tx('workflows:prompt.user-hint')}>
          {keys.map(key => {
            const value = input[key], address = at('input', { key });
            return <div key={key} className={css.promptBinding} role="group" aria-label={key}>
              <span className={css.promptBindingKey}>{key}</span>
              <div className={css.promptBindingValue}>{value.type === 'reference'
                ? <ReferencePicker address={address} path={value.path} edit={props.edit} token placeholder={tx('workflows:branch.select-variable')} />
                : <ValueEditor label={key} address={address} value={value} edit={props.edit} />}</div>
              <button type="button" className={css.promptBindingRemove} aria-label={tx('workflows:inspector.remove', { p0: key })} title={tx('workflows:inspector.remove', { p0: key })}
                onClick={() => update(current => ({ ...current, input: Object.fromEntries(Object.entries(current.input ?? {}).filter(([name]) => name !== key)) }), at('input'))}><IconTrashOutline16 /></button>
            </div>;
          })}
          <AddKey existing={keys} address={at()} label={tx('workflows:prompt.variable-name')} action={tx('workflows:prompt.add-variable')} add={(document, key) => {
            const current = nodeAt(blockAt(document, props.block)!, props.id);
            if (current?.type !== 'model') throw new CommandRejected('missing_node');
            if (Object.hasOwn(current.input ?? {}, key)) throw new CommandRejected('duplicate_identifier');
            return replaceNode(document, props.block, props.id, { ...current, input: { ...current.input, [key]: { type: 'reference', path: ['args'] } } });
          }} />
        </PromptCard>
      </div>
    </section>
    {/* As Dify's output variables: the structured output switch heads them. */}
    <Section title={tx('workflows:prompt.output-variables')} aside={<span className={css.structuredToggle}>
      {tx('workflows:prompt.structured')}
      <Tooltip label={tx('workflows:prompt.structured-hint')} side="bottom" maxWidth={240}><span className={css.promptHint} tabIndex={0} aria-label={tx('workflows:prompt.structured-hint')}><IconQuestionOutline14 /></span></Tooltip>
      <Switch checked={structured} label={tx('workflows:prompt.structured')} onChange={on => update(current => ({ ...current, output: on ? { type: 'json_schema', schema: { type: 'object', properties: {}, required: [], additionalProperties: false } } : { type: 'text' } }), at('output'))} />
    </span>}>
      {structured && node.output.type === 'json_schema' ? <SchemaEditor label={tx('workflows:inspector.model-output-schema')} value={node.output.schema} address={at('output', 'schema')} edit={props.edit} />
        : <p className={css.note}>{tx('workflows:inspector.model-text-hint')}</p>}
    </Section>
    <OutputDeclaration {...props} />
  </>;
}

export function AgentPanel(props: PanelProps) {
  const tx = useTranslation(), listId = useId(), { node, at, update } = useNode(props, 'agent');
  return <>
    <Section title={tx('workflows:panel.agent-config')}>
      <TextField label={tx('workflows:inspector.agent-profile')} hint={tx('workflows:inspector.agent-profile-hint')} value={node.profile} list={listId} invalid={!node.profile} field={encodeField(at('profile'))}
        onChange={(profile, session) => update(current => ({ ...current, profile }), at('profile'), undefined, session)} />
      <datalist id={listId}>{props.vocabulary?.profiles.map(name => <option key={name} value={name} />)}</datalist>
    </Section>
    <Section title={tx('workflows:panel.prompt')}>
      <TextField label={tx('workflows:inspector.agent-task')} value={node.task} multiline field={encodeField(at('task'))} onChange={(task, session) => update(current => ({ ...current, task }), at('task'), undefined, session)} />
    </Section>
    <Bindings {...props} />
    <Section title={tx('workflows:panel.output')}><SchemaEditor label={tx('workflows:inspector.agent-output')} value={node.output} address={at('output')} edit={props.edit} /></Section>
    <Section title={tx('workflows:panel.advanced')}>
      <details className={css.advanced}><summary>{tx('workflows:inspector.agent-override')}</summary>
        <label className={css.check}><input type="checkbox" checked={node.override !== undefined} onChange={event => { const enabled = event.target.checked; update(current => { const { override: _previous, ...rest } = current; return enabled ? { ...rest, override: {} } : rest; }, at('override')); }} />{tx('workflows:inspector.override-enabled')}</label>
        {node.override !== undefined && <JsonField label={tx('workflows:inspector.agent-override')} value={node.override} address={at('override')} />}
        <p className={css.note}>{tx('workflows:inspector.agent-override-hint')}</p>
      </details>
    </Section>
  </>;
}
