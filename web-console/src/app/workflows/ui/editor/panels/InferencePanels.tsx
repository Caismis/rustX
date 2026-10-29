import { useId, useState } from 'react';
import { Menu } from '@base-ui/react/menu';
import { IconChevronDownOutline14, IconEditOutline16, IconQuestionOutline14 } from '../../../../../presentation/primitives/icons';
import { Button } from '../../../../../presentation/primitives/Button';
import { Modal } from '../../../../../presentation/primitives/Modal';
import { schemaObject } from '../../../document/schema';
import { Switch } from '../../../../../presentation/primitives/Switch';
import { Tooltip } from '../../../../../presentation/primitives/Tooltip';
import { CommandRejected, disconnect, replaceNode } from '../../../document/commands';
import { blockAt, edgesOf, nodeAt, type NodeDefinition } from '../../../document/model';
import { edgePort } from '../../../document/ports';
import { IconPlus } from '../../icons';
import { PromptCard } from '../forms/PromptCard';
import { PromptEditor, promptText, useInsertVariable } from '../forms/PromptEditor';
import { useTranslation } from '../../../../../locale/react';
import { encodeField } from '../../../document/fields';
import { ModelSelection } from '../forms/ModelSelection';
import { SchemaEditor } from '../forms/SchemaEditor';
import { DraftTextField, JsonField, Section, TextField } from '../forms/fields';
import { Bindings, useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

export function ModelPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'model');
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
    <ModelOutputs {...props} />
    <ModelFailure {...props} />
  </>;
}

/** As Dify's LLM output variables: `text`, `reasoning_content` and `usage`,
 * then `structured_output` while the structured output switch is on. */
function ModelOutputs(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'model');
  const [open, setOpen] = useState(true), [configuring, setConfiguring] = useState(false);
  const schema = node.output.type === 'json_schema' ? node.output.schema : undefined;
  const fixed = [['text', 'string', 'workflows:prompt.output-text'], ['reasoning_content', 'string', 'workflows:prompt.output-reasoning'], ['usage', 'object', 'workflows:prompt.output-usage']] as const;
  return <section className={css.group} aria-label={tx('workflows:prompt.output-variables')}>
    <h3 className={css.groupHeading}>
      <button type="button" className={css.outputToggle} aria-expanded={open} onClick={() => setOpen(value => !value)}>{tx('workflows:prompt.output-variables')}<IconChevronDownOutline14 /></button>
      <span className={css.structuredToggle}>
        {tx('workflows:prompt.structured')}
        <Tooltip label={tx('workflows:prompt.structured-hint')} side="bottom" maxWidth={240}><span className={css.promptHint} tabIndex={0} aria-label={tx('workflows:prompt.structured-hint')}><IconQuestionOutline14 /></span></Tooltip>
        <Switch checked={!!schema} label={tx('workflows:prompt.structured')} onChange={on => update(current => ({ ...current, output: on ? { type: 'json_schema', schema: { type: 'object', properties: {}, required: [], additionalProperties: false } } : { type: 'text' } }), at('output'))} />
      </span>
    </h3>
    {open && <>
      <dl className={css.outputVars}>
        {fixed.map(([name, type, about]) => <div key={name}><dt><b>{name}</b><code>{type}</code></dt><dd>{tx(about)}</dd></div>)}
      </dl>
      {schema !== undefined && <div className={css.structuredOutput}>
        <div className={css.structuredHead}><b>{/* i18n-raw: native output field name */ 'structured_output'}</b><code>{/* i18n-raw: JSON Schema type name */ 'object'}</code>
          <button type="button" className={css.smallButton} onClick={() => setConfiguring(true)}><IconEditOutline16 />{tx('workflows:prompt.configure')}</button>
        </div>
        {Object.keys(schemaObject(schemaObject(schema)?.properties) ?? {}).length ? <SchemaTree schema={schema} /> : <p className={css.structuredEmpty}>{tx('workflows:prompt.structured-empty')}</p>}
      </div>}
    </>}
    {configuring && schema !== undefined && <Modal open onClose={() => setConfiguring(false)} title={tx('workflows:prompt.structured')} closeLabel={tx('workflows:dialog.close')} className={css.schemaDialog}
      footer={<Button variant="primary" onClick={() => setConfiguring(false)}>{tx('workflows:prompt.done')}</Button>}>
      <SchemaEditor label={tx('workflows:inspector.model-output-schema')} value={schema} address={at('output', 'schema')} edit={props.edit} />
    </Modal>}
  </section>;
}

type Strategy = NonNullable<Extract<NodeDefinition, { type: 'model' }>['on_error']>;
const STRATEGIES = ['stop', 'default_value', 'fail_branch'] as const;
const STRATEGY_TEXT = { stop: 'stop', default_value: 'default', fail_branch: 'branch' } as const;

/** As Dify's retry on failure and error handling, under the outputs. */
function ModelFailure(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'model');
  const retry = node.retry, strategy: Strategy = node.on_error ?? { type: 'stop' };
  const setRetry = (next?: NonNullable<typeof retry>) => update(current => {
    const { retry: _previous, ...rest } = current;
    return next ? { ...rest, retry: next } : rest;
  }, at('retry'));
  const choose = (type: Strategy['type']) => {
    if (type === strategy.type) return;
    props.edit(document => {
      const owner = blockAt(document, props.block), current = owner && nodeAt(owner, props.id);
      if (current?.type !== 'model') throw new CommandRejected('missing_node');
      const { on_error: _previous, ...rest } = current;
      const next: Strategy | undefined = type === 'stop' ? undefined : type === 'fail_branch' ? { type }
        : { type, text: '', ...(current.output.type === 'json_schema' ? { structured_output: {} } : {}) };
      let changed = replaceNode(document, props.block, props.id, next ? { ...rest, on_error: next } : rest);
      // Without a fail branch the node has no fail port, so its connections go.
      const failing = edgesOf(blockAt(changed, props.block)!).flatMap((edge, index) => edge.from === props.id && edgePort(edge) === 'fail' ? [index] : []);
      if (type !== 'fail_branch' && failing.length) changed = disconnect(changed, props.block, failing);
      return changed;
    });
  };
  return <>
    <section className={css.group} aria-label={tx('workflows:failure.retry')}>
      <h3 className={css.groupHeading}><span className={css.headingText}>{tx('workflows:failure.retry')}</span>
        <Switch checked={!!retry} label={tx('workflows:failure.retry')} onChange={on => setRetry(on ? { max_retries: 3, interval_ms: 1000 } : undefined)} /></h3>
      {retry && <div className={css.retryFields}>
        <RetryField label={tx('workflows:failure.max-retries')} unit={tx('workflows:failure.times')} min={1} max={10} step={1} value={retry.max_retries} onChange={max_retries => setRetry({ ...retry, max_retries })} />
        <RetryField label={tx('workflows:failure.interval')} unit={tx('workflows:failure.ms')} min={100} max={5000} step={100} value={retry.interval_ms} onChange={interval_ms => setRetry({ ...retry, interval_ms })} />
      </div>}
    </section>
    <section className={css.group} aria-label={tx('workflows:failure.title')}>
      <h3 className={css.groupHeading}>
        <span className={css.headingText}>{tx('workflows:failure.title')}
          <Tooltip label={tx('workflows:failure.tip')} side="bottom" maxWidth={240}><span className={css.promptHint} tabIndex={0} aria-label={tx('workflows:failure.tip')}><IconQuestionOutline14 /></span></Tooltip></span>
        <Menu.Root modal={false}>
          <Menu.Trigger className={css.strategyTrigger} aria-label={tx('workflows:failure.strategy')}>{tx(`workflows:failure.${STRATEGY_TEXT[strategy.type]}`)}<IconChevronDownOutline14 /></Menu.Trigger>
          <Menu.Portal><Menu.Positioner side="bottom" align="end" sideOffset={4} className={css.branchOperatorPositioner}>
            <Menu.Popup className={css.strategyPopup}>
              {STRATEGIES.map(type => <Menu.Item key={type} className={css.strategyItem} onClick={() => choose(type)}>
                <b>{tx(`workflows:failure.${STRATEGY_TEXT[type]}`)}</b><small>{tx(`workflows:failure.${STRATEGY_TEXT[type]}-desc`)}</small>
              </Menu.Item>)}
            </Menu.Popup>
          </Menu.Positioner></Menu.Portal>
        </Menu.Root>
      </h3>
      {strategy.type === 'default_value' && <div className={css.failureDefaults}>
        <DraftTextField label={/* i18n-raw: native output field name */ 'text'} value={strategy.text} address={at('on_error', 'text')} multiline />
        {node.output.type === 'json_schema' && <JsonField label={/* i18n-raw: native output field name */ 'structured_output'} value={strategy.structured_output ?? {}} rows={4} address={at('on_error', 'structured_output')} />}
      </div>}
      {strategy.type === 'fail_branch' && <div className={css.failBranchCard}><b>{tx('workflows:failure.branch-customize')}</b><p>{tx('workflows:failure.branch-tip')}</p></div>}
    </section>
  </>;
}

/** As Dify's retry fields: a slider with its exact number beside it. */
function RetryField({ label, unit, min, max, step, value, onChange }: { label: string; unit: string; min: number; max: number; step: number; value: number; onChange: (value: number) => void }) {
  const set = (text: string) => { const next = Number(text); if (Number.isInteger(next) && next >= min && next <= max) onChange(next); };
  return <label className={css.retryField}>
    <span>{label}</span>
    <input type="range" min={min} max={max} step={step} value={value} aria-label={label} onChange={event => set(event.target.value)} />
    <span className={css.retryNumber}><input type="number" min={min} max={max} step={step} value={value} aria-label={`${label} (${unit})`} onChange={event => set(event.target.value)} /><small>{unit}</small></span>
  </label>;
}

/** The declared fields of a structured output, as Dify lists them. */
function SchemaTree({ schema }: { schema: unknown }) {
  const tx = useTranslation(), object = schemaObject(schema), properties = schemaObject(object?.properties) ?? {};
  const required = Array.isArray(object?.required) ? object.required : [];
  return <ul className={css.schemaTree}>{Object.entries(properties).map(([name, field]) => {
    const value = schemaObject(field), type = typeof value?.type === 'string' ? value.type : '?';
    return <li key={name}>
      <span><b>{name}</b><code>{type}</code>{required.includes(name) && <small>{tx('workflows:schema.required')}</small>}</span>
      {type === 'object' && <SchemaTree schema={value} />}
    </li>;
  })}</ul>;
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
