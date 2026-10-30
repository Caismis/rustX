import clsx from 'clsx';
import { lazy, Suspense, useState, type ReactNode } from 'react';
import { Menu } from '@base-ui/react/menu';
import { IconChevronDownOutline14, IconEditOutline16, IconQuestionOutline14 } from '../../../../../presentation/primitives/icons';
import { schemaObject, schemaTarget } from '../../../document/schema';
import { Switch } from '../../../../../presentation/primitives/Switch';
import { Tooltip } from '../../../../../presentation/primitives/Tooltip';
import { CommandRejected, freshIdentifier, renameAgentRoute, replaceNode, replaceNodeExits } from '../../../document/commands';
import { blockAt, nodeAt, type NodeDefinition, type WorkflowDocument } from '../../../document/model';
import { orderedKeys } from '../../../document/presentation';
import { IconPlus } from '../../icons';
import { PromptCard } from '../forms/PromptCard';
import { PromptEditor, useInsertVariable } from '../forms/PromptEditor';
import { promptText } from '../summary';
import { KindIcon } from '../../icons';
import { useTranslation } from '../../../../../locale/react';
import { encodeField, within, type FieldAddress } from '../../../document/fields';
import { ModelSelection } from '../forms/ModelSelection';
import { DraftTextField, JsonField } from '../forms/fields';
import { useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

const StructuredOutputDialog = lazy(() => import('../forms/structured-output/StructuredOutputDialog').then(module => ({ default: module.StructuredOutputDialog })));

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
        <StructuredFields schema={schema} document={props.document} address={at('output', 'schema')} />
      </div>}
    </>}
    {configuring && schema !== undefined && <Suspense fallback={null}><StructuredOutputDialog document={props.document} address={at('output', 'schema')} root={/* i18n-raw: native output field name */ 'structured_output'} edit={props.edit} onClose={() => setConfiguring(false)} /></Suspense>}
  </section>;
}

type Strategy = NonNullable<Extract<NodeDefinition, { type: 'model' }>['on_error']>;
export type Retry = NonNullable<Extract<NodeDefinition, { type: 'model' }>['retry']>;
const STRATEGIES = ['stop', 'default_value', 'fail_branch'] as const;
const STRATEGY_TEXT = { stop: 'stop', default_value: 'default', fail_branch: 'branch' } as const;

/** As Dify's retry on failure and error handling, under the outputs. */
function ModelFailure(props: PanelProps) {
  const { node, at, update } = useNode(props, 'model');
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
      // Without a fail branch the node has no fail port, so its connections go.
      return replaceNodeExits(document, props.block, props.id, next ? { ...rest, on_error: next } : rest);
    });
  };
  return <>
    <RetrySettings retry={retry} setRetry={setRetry} />
    <ErrorStrategy strategy={strategy.type} choose={choose}>
      {strategy.type === 'default_value' && <div className={css.failureDefaults}>
        <DraftTextField label={/* i18n-raw: native output field name */ 'text'} value={strategy.text} address={at('on_error', 'text')} multiline />
        {node.output.type === 'json_schema' && <JsonField label={/* i18n-raw: native output field name */ 'structured_output'} value={strategy.structured_output ?? {}} rows={4} address={at('on_error', 'structured_output')} />}
      </div>}
    </ErrorStrategy>
  </>;
}

/** As Dify's retry on failure: a switch, then how often and how far apart. */
export function RetrySettings({ retry, setRetry }: { retry?: Retry | null; setRetry: (retry?: Retry) => void }) {
  const tx = useTranslation();
  return <section className={css.group} aria-label={tx('workflows:failure.retry')}>
    <h3 className={css.groupHeading}><span className={css.headingText}>{tx('workflows:failure.retry')}</span>
      <Switch checked={!!retry} label={tx('workflows:failure.retry')} onChange={on => setRetry(on ? { max_retries: 3, interval_ms: 1000 } : undefined)} /></h3>
    {retry && <div className={css.retryFields}>
      <RetryField label={tx('workflows:failure.max-retries')} unit={tx('workflows:failure.times')} min={1} max={10} step={1} value={retry.max_retries} onChange={max_retries => setRetry({ ...retry, max_retries })} />
      <RetryField label={tx('workflows:failure.interval')} unit={tx('workflows:failure.ms')} min={100} max={5000} step={100} value={retry.interval_ms} onChange={interval_ms => setRetry({ ...retry, interval_ms })} />
    </div>}
  </section>;
}

/** As Dify's error handling: the strategy menu, what it leaves behind, and
 * the fail branch's card. */
export function ErrorStrategy({ strategy, choose, note, children }: { strategy: Strategy['type']; choose: (type: Strategy['type']) => void; note?: string; children?: ReactNode }) {
  const tx = useTranslation();
  return <section className={css.group} aria-label={tx('workflows:failure.title')}>
    <h3 className={css.groupHeading}>
      <span className={css.headingText}>{tx('workflows:failure.title')}
        <Tooltip label={tx('workflows:failure.tip')} side="bottom" maxWidth={240}><span className={css.promptHint} tabIndex={0} aria-label={tx('workflows:failure.tip')}><IconQuestionOutline14 /></span></Tooltip></span>
      <Menu.Root modal={false}>
        <Menu.Trigger className={css.strategyTrigger} aria-label={tx('workflows:failure.strategy')}>{tx(`workflows:failure.${STRATEGY_TEXT[strategy]}`)}<IconChevronDownOutline14 /></Menu.Trigger>
        <Menu.Portal><Menu.Positioner side="bottom" align="end" sideOffset={4} className={css.branchOperatorPositioner}>
          <Menu.Popup className={css.strategyPopup}>
            {STRATEGIES.map(type => <Menu.Item key={type} className={css.strategyItem} onClick={() => choose(type)}>
              <b>{tx(`workflows:failure.${STRATEGY_TEXT[type]}`)}</b><small>{tx(`workflows:failure.${STRATEGY_TEXT[type]}-desc`)}</small>
            </Menu.Item>)}
          </Menu.Popup>
        </Menu.Positioner></Menu.Portal>
      </Menu.Root>
    </h3>
    {note && <p className={css.note}>{note}</p>}
    {children}
    {strategy === 'fail_branch' && <div className={css.failBranchCard}><b>{tx('workflows:failure.branch-customize')}</b><p>{tx('workflows:failure.branch-tip')}</p></div>}
  </section>;
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
function SchemaTree({ schema, document, address }: { schema: unknown; document: WorkflowDocument; address: FieldAddress }) {
  const tx = useTranslation(), object = schemaObject(schema), properties = schemaObject(object?.properties) ?? {};
  const required = Array.isArray(object?.required) ? object.required : [];
  const target = schemaTarget(address, document), keys = target ? orderedKeys(document, target, Object.keys(properties)) : Object.keys(properties);
  return <ul className={css.schemaTree}>{keys.map(name => {
    const value = schemaObject(properties[name]), type = typeof value?.type === 'string' ? value.type : '?';
    return <li key={name}>
      <span><b>{name}</b><code>{type}</code>{required.includes(name) && <small>{tx('workflows:schema.required')}</small>}</span>
      {type === 'object' && <SchemaTree schema={value} document={document} address={within(address, ['properties', { key: name }])} />}
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
  const tx = useTranslation();
  return <>
    {/* As Dify's Agent panel: the Agent is picked from a list, its task is
        written in the LLM node's prompt editor, then its output variables. */}
    <section className={css.group}>
      <h3 className={css.groupHeading}>
        <span className={css.headingText}>{tx('workflows:panel.agent-config')}
          <Tooltip label={tx('workflows:inspector.agent-profile-hint')} side="bottom" maxWidth={240}><span className={css.promptHint} tabIndex={0} aria-label={tx('workflows:inspector.agent-profile-hint')}><IconQuestionOutline14 /></span></Tooltip></span>
      </h3>
      <ProfileSelection {...props} />
    </section>
    <section className={css.group}><AgentTask {...props} /></section>
    <AgentOutputs {...props} />
    <AgentRoutes {...props} />
    <AgentFailure {...props} />
  </>;
}

type Agent = Extract<NodeDefinition, { type: 'agent' }>;

/** Replace this Agent and drop connections on exits it no longer has. */
function useAgentExits(props: PanelProps) {
  return (change: (agent: Agent) => Agent) => props.edit(document => {
    const owner = blockAt(document, props.block), current = owner && nodeAt(owner, props.id);
    if (current?.type !== 'agent') throw new CommandRejected('missing_node');
    return replaceNodeExits(document, props.block, props.id, change(current));
  });
}

/** As Dify's Agent output routes: the Agent picks one when it finishes,
 * and each route is an exit of its own. */
function AgentRoutes(props: PanelProps) {
  const tx = useTranslation(), { node, at } = useNode(props, 'agent'), exits = useAgentExits(props);
  const routes = node.routes ?? [];
  const withRoutes = (agent: Agent, next: Agent['routes']) => {
    const { routes: _previous, ...rest } = agent;
    return next?.length ? { ...rest, routes: next } : rest;
  };
  const fresh = (taken: readonly { id: string }[]) => freshIdentifier({ input: {}, output: {}, nodes: Object.fromEntries(taken.map(route => [route.id, { type: 'start' }])), edges: [] }, 'route');
  return <section className={css.group} aria-label={tx('workflows:routes.title')}>
    <h3 className={css.groupHeading}>
      <span className={css.headingText}>{tx('workflows:routes.title')}
        <Tooltip label={tx('workflows:routes.tip')} side="bottom" maxWidth={260}><span className={css.promptHint} tabIndex={0} aria-label={tx('workflows:routes.tip')}><IconQuestionOutline14 /></span></Tooltip></span>
      <Switch checked={!!routes.length} label={tx('workflows:routes.title')} onChange={on => exits(agent => withRoutes(agent, on ? [{ id: 'route_1', description: '' }, { id: 'route_2', description: '' }] : undefined))} />
    </h3>
    {!!routes.length && <div className={css.routeList}>
      {routes.map((route, index) => <div key={route.id} className={css.routeItem}>
        <div className={css.routeItemHead}>
          <RouteId value={route.id} label={tx('workflows:routes.id')} onCommit={to => props.edit(document => renameAgentRoute(document, props.block, props.id, route.id, to))} />
          <button type="button" className={css.smallButton} disabled={routes.length < 2} aria-label={tx('workflows:routes.remove', { p0: route.id })} title={tx('workflows:routes.remove', { p0: route.id })}
            onClick={() => exits(agent => withRoutes(agent, (agent.routes ?? []).filter(item => item.id !== route.id)))}>×</button>
        </div>
        <DraftTextField label={tx('workflows:routes.description')} value={route.description} address={at('routes', index, 'description')} multiline />
      </div>)}
      <button type="button" className={css.promptAddMessage} disabled={routes.length >= 32}
        onClick={() => exits(agent => withRoutes(agent, [...(agent.routes ?? []), { id: fresh(agent.routes ?? []), description: '' }]))}><IconPlus size={13} />{tx('workflows:routes.add')}</button>
    </div>}
  </section>;
}

/** A route ID commits on blur or Enter, moving its connections with it. */
function RouteId({ value, label, onCommit }: { value: string; label: string; onCommit: (value: string) => void }) {
  const [draft, setDraft] = useState(value);
  const commit = () => { const next = draft.trim(); if (next && next !== value) onCommit(next); else setDraft(value); };
  return <input className={css.routeIdInput} aria-label={label} value={draft} onChange={event => setDraft(event.target.value)} onBlur={commit}
    onKeyDown={event => { if (event.key === 'Enter') event.currentTarget.blur(); if (event.key === 'Escape') { setDraft(value); event.currentTarget.blur(); } }} />;
}

/** As Dify's Agent error handling. There is no retry: a workflow child
 * writes the shared workspace directly, so another run could repeat it. */
function AgentFailure(props: PanelProps) {
  const tx = useTranslation(), { node, at } = useNode(props, 'agent'), exits = useAgentExits(props);
  const strategy = node.on_error ?? { type: 'stop' as const };
  const choose = (type: Strategy['type']) => {
    if (type === strategy.type) return;
    exits(agent => {
      const { on_error: _previous, ...rest } = agent;
      if (type === 'stop') return rest;
      if (type === 'fail_branch') return { ...rest, on_error: { type } };
      // A starting default the node's own contract can accept once filled in.
      const base = agent.output.type === 'text' ? { text: '' } : {};
      const route = agent.routes?.[0]?.id;
      return { ...rest, on_error: { type, value: route ? { ...base, switch: route } : base } };
    });
  };
  return <ErrorStrategy strategy={strategy.type} choose={choose} note={tx('workflows:failure.agent-no-retry')}>
    {strategy.type === 'default_value' && <div className={css.failureDefaults}>
      <JsonField label={tx('workflows:failure.default-value')} value={strategy.value} rows={4} address={at('on_error', 'value')} />
    </div>}
  </ErrorStrategy>;
}

/** As Dify's Agent output variables: the final reply is `text`; with
 * structured output on, the declared fields are the node's variables. */
function AgentOutputs(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'agent');
  const [open, setOpen] = useState(true), [configuring, setConfiguring] = useState(false);
  const schema = node.output.type === 'json_schema' ? node.output.schema : undefined;
  // Routes add the chosen route's ID to either mode.
  const switchRow = node.routes?.length ? <div><dt><b>{/* i18n-raw: native output field name */ 'switch'}</b><code>{/* i18n-raw: JSON Schema type name */ 'string'}</code></dt><dd>{tx('workflows:prompt.agent-switch')}</dd></div> : undefined;
  return <section className={css.group} aria-label={tx('workflows:prompt.output-variables')}>
    <h3 className={css.groupHeading}>
      <button type="button" className={css.outputToggle} aria-expanded={open} onClick={() => setOpen(value => !value)}>{tx('workflows:prompt.output-variables')}<IconChevronDownOutline14 /></button>
      <span className={css.structuredToggle}>
        {tx('workflows:prompt.structured')}
        <Tooltip label={tx('workflows:prompt.agent-structured-hint')} side="bottom" maxWidth={240}><span className={css.promptHint} tabIndex={0} aria-label={tx('workflows:prompt.agent-structured-hint')}><IconQuestionOutline14 /></span></Tooltip>
        <Switch checked={!!schema} label={tx('workflows:prompt.structured')} onChange={on => update(current => ({ ...current, output: on ? { type: 'json_schema', schema: { type: 'object', properties: {}, required: [], additionalProperties: false } } : { type: 'text' } }), at('output'))} />
      </span>
    </h3>
    {open && (schema === undefined
      ? <dl className={css.outputVars}><div><dt><b>{/* i18n-raw: native output field name */ 'text'}</b><code>{/* i18n-raw: JSON Schema type name */ 'string'}</code></dt><dd>{tx('workflows:prompt.agent-text')}</dd></div>{switchRow}</dl>
      : <div className={css.agentStructured}>
        <div className={css.structuredHead}><b>{props.id}</b><code>{/* i18n-raw: JSON Schema type name */ 'object'}</code>
          <button type="button" className={css.smallButton} onClick={() => setConfiguring(true)}><IconEditOutline16 />{tx('workflows:prompt.configure')}</button>
        </div>
        <StructuredFields schema={schema} document={props.document} address={at('output', 'schema')} />
        {switchRow && <dl className={clsx(css.outputVars, css.agentSwitch)}>{switchRow}</dl>}
      </div>)}
    {configuring && schema !== undefined && <Suspense fallback={null}><StructuredOutputDialog document={props.document} address={at('output', 'schema')} root={props.id} edit={props.edit} onClose={() => setConfiguring(false)} /></Suspense>}
  </section>;
}

/** A structured output's declared fields, or the note that it has none. */
function StructuredFields({ schema, document, address }: { schema: unknown; document: WorkflowDocument; address: FieldAddress }) {
  const tx = useTranslation();
  return Object.keys(schemaObject(schemaObject(schema)?.properties) ?? {}).length ? <SchemaTree schema={schema} document={document} address={address} /> : <p className={css.structuredEmpty}>{tx('workflows:prompt.structured-empty')}</p>;
}

/** The named profile, chosen from those discovered in this scope. */
function ProfileSelection(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'agent'), profiles = props.vocabulary?.profiles ?? [];
  return <Menu.Root modal={false}>
    <Menu.Trigger className={css.modelTrigger} aria-label={tx('workflows:inspector.agent-profile')} aria-invalid={!node.profile || undefined} title={tx('workflows:inspector.agent-profile-hint')}>
      <span className={`${css.modelTriggerIcon} ${css.kind_agent}`}><KindIcon kind="agent" size={14} /></span>
      <span className={css.modelTriggerName}>{node.profile || tx('workflows:inspector.agent-profile-choose')}</span>
      <span className={css.modelTriggerSettings} aria-hidden><IconChevronDownOutline14 /></span>
    </Menu.Trigger>
    <Menu.Portal><Menu.Positioner side="bottom" align="start" sideOffset={4} className={css.branchOperatorPositioner}>
      <Menu.Popup className={css.profilePopup}>
        {profiles.length ? profiles.map(name => <Menu.Item key={name} className={css.strategyItem} aria-checked={name === node.profile} role="menuitemradio"
          onClick={() => { if (name !== node.profile) update(current => ({ ...current, profile: name }), at('profile')); }}><b>{name}</b></Menu.Item>)
          : <p className={css.profileEmpty}>{tx('workflows:inspector.agent-profile-none')}</p>}
      </Menu.Popup>
    </Menu.Positioner></Menu.Portal>
  </Menu.Root>;
}

/** The Agent's task in the prompt editor, with the variables it inserts. */
function AgentTask(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'agent');
  const [expanded, setExpanded] = useState(false), [session, setSession] = useState(0);
  const address = at('task'), field = encodeField(address), label = tx('workflows:inspector.agent-task');
  return <PromptEditor parts={node.task} address={address} edit={props.edit} label={label} expanded={expanded} invalid={!node.task.length}
    placeholder={tx('workflows:prompt.task-placeholder')} variablePlaceholder={tx('workflows:branch.select-variable')}
    onChange={task => update(current => ({ ...current, task }), address, undefined, `${field}:${session}`)}>
    {editable => <PromptCard label={label} hint={tx('workflows:prompt.task-hint')} text={promptText(node.task)} expanded={expanded} onExpand={() => setExpanded(value => !value)}
      role={<b className={css.promptRoleFixed}>{label}</b>} tools={<InsertVariable />}>
      <div onFocus={() => setSession(value => value + 1)}>{editable}</div>
    </PromptCard>}
  </PromptEditor>;
}
