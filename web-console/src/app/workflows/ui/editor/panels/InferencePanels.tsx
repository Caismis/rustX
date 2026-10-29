import { useId, useState } from 'react';
import { Menu } from '@base-ui/react/menu';
import { IconChevronDownOutline14, IconQuestionOutline14 } from '../../../../../presentation/primitives/icons';
import { Switch } from '../../../../../presentation/primitives/Switch';
import { Tooltip } from '../../../../../presentation/primitives/Tooltip';
import { CommandRejected, replaceNode } from '../../../document/commands';
import { blockAt, nodeAt, type NodeDefinition } from '../../../document/model';
import { IconPlus } from '../../icons';
import { PromptCard } from '../forms/PromptCard';
import { PromptEditor, promptText, useInsertVariable } from '../forms/PromptEditor';
import { useTranslation } from '../../../../../locale/react';
import { encodeField } from '../../../document/fields';
import { ModelSelection } from '../forms/ModelSelection';
import { SchemaEditor } from '../forms/SchemaEditor';
import { JsonField, Section, TextField } from '../forms/fields';
import { Bindings, OutputDeclaration, useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

export function ModelPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'model');
  const structured = node.output.type === 'json_schema';
  return <>
    {/* As Dify's LLM panel: the model and its prompt messages form one block. */}
    <section className={css.group}>
      <ModelSelection key={encodeField(at('model'))} policy={node.model ?? { mode: 'session' }} address={at('model')} port={props.port} onChange={(model, field) => update(current => ({ ...current, model }), at('model', ...field))} />
      <div className={css.promptMessages}>
        {node.messages.map((message, index) => <PromptMessage key={index} {...props} index={index} message={message} />)}
        <button type="button" className={css.promptAddMessage} disabled={node.messages.length >= 32} onClick={() => update(current => {
          // As Dify: a new turn answers the last one.
          const last = current.messages.at(-1);
          return { ...current, messages: [...current.messages, { role: last?.role === 'user' ? 'assistant' : 'user', content: [] }] };
        }, at('messages'), { array: at('messages'), inserted: node.messages.length })}><IconPlus size={13} />{tx('workflows:prompt.add-message')}</button>
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

type Message = Extract<NodeDefinition, { type: 'model' }>['messages'][number];

/** One prompt message as Dify's message editor: SYSTEM first, then turns
 * whose role switches between USER and ASSISTANT. */
function PromptMessage(props: PanelProps & { index: number; message: Message }) {
  const tx = useTranslation(), { at, update } = useNode(props, 'model'), { index, message } = props;
  const [expanded, setExpanded] = useState(false), [session, setSession] = useState(0);
  const content = message.content ?? [], address = at('messages', index, 'content'), field = encodeField(address);
  const role = /* i18n-raw: model message role, as Dify shows it */ message.role.toUpperCase();
  const messages = (change: (messages: Message[]) => Message[], field = at('messages', index)) => update(current => ({ ...current, messages: change(current.messages) }), field);
  const setRole = (next: 'user' | 'assistant') => { if (next !== message.role) messages(all => all.map((item, at) => at === index ? { ...item, role: next } : item), at('messages', index, 'role')); };
  return <PromptEditor parts={content} address={address} edit={props.edit} label={role} expanded={expanded} invalid={index > 0 && !content.length}
    placeholder={tx(index ? 'workflows:prompt.message-placeholder' : 'workflows:prompt.system-placeholder')} variablePlaceholder={tx('workflows:branch.select-variable')}
    onChange={parts => update(current => ({ ...current, messages: current.messages.map((item, at) => at === index ? { ...item, content: parts } : item) }), address, undefined, `${field}:${session}`)}>
    {editable => <PromptCard label={role} hint={tx(`workflows:prompt.${message.role}-hint`)} text={promptText(content)} expanded={expanded} onExpand={() => setExpanded(value => !value)}
      role={index ? <Menu.Root modal={false}>
        <Menu.Trigger className={css.promptRole} aria-label={tx('workflows:prompt.role')}>{role}<IconChevronDownOutline14 /></Menu.Trigger>
        <Menu.Portal><Menu.Positioner side="bottom" align="start" sideOffset={4} className={css.branchOperatorPositioner}>
          <Menu.Popup className={css.branchOperatorPopup}>
            {(['user', 'assistant'] as const).map(value => <Menu.Item key={value} className={css.branchOperatorItem} onClick={() => setRole(value)}>{/* i18n-raw: model message role, as Dify shows it */ value.toUpperCase()}</Menu.Item>)}
          </Menu.Popup>
        </Menu.Positioner></Menu.Portal>
      </Menu.Root> : <b className={css.promptRoleFixed}>{role}</b>}
      tools={<InsertVariable />}
      onRemove={index ? () => props.edit(document => {
        const owner = blockAt(document, props.block), current = owner && nodeAt(owner, props.id);
        if (current?.type !== 'model') throw new CommandRejected('missing_node');
        return replaceNode(document, props.block, props.id, { ...current, messages: current.messages.filter((_, at) => at !== index) });
      }, undefined, { array: at('messages'), removed: index }) : undefined}>
      <div onFocus={() => setSession(value => value + 1)}>{editable}</div>
    </PromptCard>}
  </PromptEditor>;
}

function InsertVariable() {
  const tx = useTranslation(), insert = useInsertVariable();
  return <button type="button" className={css.promptTool} aria-label={tx('workflows:prompt.insert-variable')} title={tx('workflows:prompt.insert-variable')}
    onMouseDown={event => event.preventDefault()} onClick={insert}><span aria-hidden className={css.promptVariableIcon}>{'{x}'}</span></button>;
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
