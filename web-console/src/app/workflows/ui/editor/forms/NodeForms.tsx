import { useId } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../../../../locale/react';
import type { SubagentInvocationOverride, WorkflowVocabulary } from '../../../../../../../protocol/app-server/v57';
import {
  addBranch, CommandRejected, connect, deleteNodes, disconnect, reconnect, removeBranch, renameBranch, renameNode, replaceNode, setEntry,
} from '../../../document/commands';
import { encodeField, nodeField, type ArrayEdit, type FieldAddress, type FieldStep } from '../../../document/fields';
import { blockAt, deepNodeCount, edgesOf, nativeBlockPath, nodeAt, type BlockPath, type NodeDefinition, type WorkflowDocument } from '../../../document/model';
import { edgePort, outputPorts, type ControlPort } from '../../../document/ports';
import type { WorkflowScopePort } from '../../../port';
import { ModelSelection } from './ModelSelection';
import { AddKey, PredicateEditor, ValueEditor } from './ValueEditor';
import { Field, JsonField, KeyField, Section, SelectField, TextField } from './fields';
import css from '../Editor.module.css';

export type InspectorTab = 'config' | 'input' | 'output';
type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => void;
type Of<K extends NodeDefinition['type']> = Extract<NodeDefinition, { type: K }>;

export function tabsOf(kind: NodeDefinition['type']): readonly InspectorTab[] {
  return kind === 'branch' || kind === 'return' ? ['config'] : ['config', 'input', 'output'];
}

export interface Derived { readonly schema?: unknown; readonly current: boolean }

/** The inspector forms of one node. Each control writes exactly its native
 * field through a structured command against the current document. */
export function NodeForm({ document, block, id, tab, vocabulary, derived, edit, onRenamed, navigate, rejected, port }: {
  document: WorkflowDocument; block: BlockPath; id: string; tab: InspectorTab; vocabulary?: WorkflowVocabulary;
  derived?: Derived; port: WorkflowScopePort; edit: Edit; onRenamed: (from: string, to: string) => void; navigate: (block: BlockPath) => void; rejected?: string;
}) {
  const tx = useTranslation();
  const owner = blockAt(document, block)!;
  const node = nodeAt(owner, id);
  const listId = useId();
  if (!node) return null;
  /** The address of one of this node's fields. */
  const at = (...path: FieldStep[]) => nodeField(block, id, path);
  /** Update this node from its value in the document being edited. */
  const update = <K extends NodeDefinition['type']>(kind: K, change: (node: Of<K>) => Of<K>, field: FieldAddress, array?: ArrayEdit) =>
    edit(current => replaceNode(current, block, id, change(currentNode(current, block, id, kind))), array ? undefined : encodeField(field), array);
  /** Remove or append one Review context item, as one known array edit. */
  const resizeContext = (resize: (current: Of<'review'>['context']) => Of<'review'>['context'], array: ArrayEdit) => node.type === 'review' && update('review', current => {
    if (current.context.length !== node.context.length) throw new CommandRejected('missing_field');
    return { ...current, context: resize(current.context) };
  }, at('context'), array);
  const derivedSection = (title: string) => <Section title={title}>
    {derived?.schema !== undefined
      ? <><pre className={css.codebox}>{JSON.stringify(derived.schema, null, 2)}</pre><p className={css.note}>{derived.current ? tx('workflows:inspector.derived') : tx('workflows:inspector.derived-outdated')}</p></>
      : <p className={css.note}>{tx('workflows:inspector.derived-unknown')}</p>}
  </Section>;

  if (tab === 'config') return <>
    <KeyField label={tx('workflows:inspector.node-id')} hint={tx('workflows:inspector.rename-hint')} value={id} error={rejected} address={nodeField(block, id, [], 'key')}
      rename={(current, from, to) => renameNode(current, block, from, to)} onRenamed={to => onRenamed(id, to)} />
    <div className={css.row}>
      {owner.entry === id
        ? <span className={css.note}>{tx('workflows:inspector.is-entry')}</span>
        : <button type="button" className={css.smallButton} onClick={() => edit(current => setEntry(current, block, id))}>{tx('workflows:inspector.set-entry')}</button>}
      <button type="button" className={clsx(css.smallButton, css.dangerText)} onClick={() => edit(current => deleteNodes(current, block, [id]))}>{tx('workflows:inspector.delete-node')}</button>
    </div>
    {node.type === 'agent' && <>
      <TextField label={tx('workflows:inspector.agent-profile')} hint={tx('workflows:inspector.agent-profile-hint')} value={node.profile} mono list={listId} invalid={!node.profile} field={encodeField(at('profile'))}
        onChange={(profile, session) => edit(current => replaceNode(current, block, id, { ...currentNode(current, block, id, 'agent'), profile }), session)} />
      <datalist id={listId}>{vocabulary?.profiles.map(name => <option key={name} value={name} />)}</datalist>
      <TextField label={tx('workflows:inspector.agent-task')} value={node.task} multiline field={encodeField(at('task'))}
        onChange={(task, session) => edit(current => replaceNode(current, block, id, { ...currentNode(current, block, id, 'agent'), task }), session)} />
      <OverrideField node={node} address={at('override')} onChange={invocation => update('agent', current => {
        const { override: _previous, ...rest } = current;
        return invocation === undefined ? rest : { ...rest, override: invocation };
      }, at('override'))} />
    </>}
    {node.type === 'model' && <>
      <ModelSelection key={encodeField(at('model'))} policy={node.model ?? { mode: 'session' }} address={at('model')} port={port}
        onChange={(model, field) => update('model', current => ({ ...current, model }), at('model', ...field))} />
      <TextField label={tx('workflows:inspector.model-instruction')} hint={tx('workflows:inspector.model-instruction-hint')} value={node.instruction} multiline invalid={!node.instruction.trim()} field={encodeField(at('instruction'))}
        onChange={(instruction, session) => edit(current => replaceNode(current, block, id, { ...currentNode(current, block, id, 'model'), instruction }), session)} />
    </>}
    {node.type === 'tool' && <>
      <SelectField label={tx('workflows:inspector.tool-origin')} value={node.selector.origin}
        options={[{ value: 'builtin', label: tx('workflows:inspector.tool-origin-builtin') }, { value: 'source', label: tx('workflows:inspector.tool-origin-source') }]}
        onChange={origin => update('tool', current => ({ ...current, selector: origin === 'builtin' ? { origin, name: current.selector.name } : { origin, source_id: current.selector.origin === 'source' ? current.selector.source_id : '', name: current.selector.name } }), at('selector'))} />
      {node.selector.origin === 'source' && <TextField label={tx('workflows:inspector.tool-source')} value={node.selector.source_id} mono field={encodeField(at('selector', 'source_id'))}
        onChange={(source_id, session) => edit(current => {
          const tool = currentNode(current, block, id, 'tool');
          return replaceNode(current, block, id, { ...tool, selector: { origin: 'source', source_id, name: tool.selector.name } });
        }, session)} />}
      <TextField label={tx('workflows:inspector.tool-name')} value={node.selector.name} mono list={node.selector.origin === 'builtin' ? listId : undefined} invalid={!node.selector.name} field={encodeField(at('selector', 'name'))}
        onChange={(name, session) => edit(current => {
          const tool = currentNode(current, block, id, 'tool');
          return replaceNode(current, block, id, { ...tool, selector: { ...tool.selector, name } });
        }, session)} />
      <datalist id={listId}>{vocabulary?.builtin_tools.map(name => <option key={name} value={name} />)}</datalist>
    </>}
    {node.type === 'branch' && <>
      <PredicateEditor label={tx('workflows:inspector.branch-condition')} address={at('condition')} predicate={node.condition} edit={edit} />
      <p className={css.note}>{tx('workflows:inspector.branch-ports')}</p>
    </>}
    {node.type === 'parallel' && <Section title={tx('workflows:inspector.parallel-branches')} aside={String(Object.keys(node.branches).length)}>
      {Object.entries(node.branches).sort(([a], [b]) => a.localeCompare(b)).map(([key, branch]) => <div key={key} className={css.branchRow}>
        <KeyField value={key} address={nodeField(block, id, ['branches', { key }], 'key')} label={tx('workflows:inspector.parallel-rename', { p0: key })} compact={tx('workflows:inspector.parallel-rename', { p0: key })}
          rename={(current, from, to) => renameBranch(current, block, id, from, to)} />
        <span className={css.branchMeta}>{tx('workflows:inspector.parallel-branch-nodes', { p0: deepNodeCount(branch.block) })}</span>
        <button type="button" className={css.linkButton} onClick={() => navigate([...block, { node: id, branch: key }])}>{tx('workflows:inspector.parallel-edit')} →</button>
        <button type="button" className={css.pathRemove} aria-label={tx('workflows:inspector.parallel-remove', { p0: key })} disabled={Object.keys(node.branches).length === 1}
          onClick={() => edit(current => removeBranch(current, block, id, key))}>×</button>
      </div>)}
      <AddKey existing={Object.keys(node.branches)} label={tx('workflows:inspector.parallel-new-key')} action={tx('workflows:inspector.parallel-add')} onAdd={key => edit(current => addBranch(current, block, id, key))} />
      <p className={css.note}>{tx('workflows:inspector.parallel-note')}</p>
    </Section>}
    {node.type === 'review' && <>
      <ValueEditor label={tx('workflows:inspector.review-subject')} address={at('subject', 'value')} value={node.subject.value} edit={edit} />
      <Section title={tx('workflows:inspector.review-context')} aside={String(node.context.length)}>
        {node.context.map((item, index) => <div key={index} className={css.nestedRow}>
          <ValueEditor label={`[${index}]`} address={at('context', index)} value={item} edit={edit} />
          <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: `[${index}]` })} onClick={() => resizeContext(current => current.filter((_, position) => position !== index), { array: at('context'), removed: index })}>×</button>
        </div>)}
        <button type="button" className={css.smallButton} onClick={() => resizeContext(current => [...current, { type: 'reference', path: ['args'] }], { array: at('context'), inserted: node.context.length })}>{tx('workflows:inspector.add-item')}</button>
      </Section>
      <p className={css.note}>{tx('workflows:inspector.review-note')}</p>
    </>}
    {node.type === 'loop' && <>
      <Field label={tx('workflows:inspector.loop-max')}>{fieldId => <input id={fieldId} type="number" min={1} max={256} className={css.input} value={node.max_iterations}
        onChange={event => { const value = Number(event.target.value); if (Number.isSafeInteger(value) && value >= 0) update('loop', current => ({ ...current, max_iterations: value }), at('max_iterations')); }} />}</Field>
      <PredicateEditor label={tx('workflows:inspector.loop-until')} address={at('until')} predicate={node.until} edit={edit} />
      <p className={css.note}>{tx('workflows:inspector.loop-until-hint')}</p>
      <ValueEditor label={tx('workflows:inspector.loop-carry')} address={at('carry')} value={node.carry} edit={edit} />
      <Section title={tx('workflows:inspector.loop-body')} aside={tx('workflows:inspector.parallel-branch-nodes', { p0: deepNodeCount(node.body) })}>
        <button type="button" className={css.linkButton} onClick={() => navigate([...block, { node: id }])}>{tx('workflows:inspector.loop-edit')} →</button>
      </Section>
      <p className={css.note}>{tx('workflows:inspector.loop-ports')}</p>
    </>}
    {node.type === 'return' && <>
      <ValueEditor label={tx('workflows:inspector.return-output')} address={at('output')} value={node.output} edit={edit} />
      <p className={css.note}>{tx('workflows:inspector.return-note')}</p>
    </>}
    <Successors document={document} block={block} id={id} edit={edit} />
  </>;

  if (tab === 'input') return <>
    {(node.type === 'agent' || node.type === 'model') && <Section title={tx('workflows:inspector.agent-input')} aside={String(Object.keys(node.input ?? {}).length)}>
      {Object.entries(node.input ?? {}).map(([key, binding]) => <div key={key} className={css.nestedRow}>
        <ValueEditor label={key} address={at('input', { key })} value={binding} edit={edit} />
        <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: key })}
          onClick={() => update(node.type, current => ({ ...current, input: Object.fromEntries(Object.entries(current.input ?? {}).filter(([name]) => name !== key)) }), at('input'))}>×</button>
      </div>)}
      <AddKey existing={Object.keys(node.input ?? {})} onAdd={key => update(node.type, current => Object.hasOwn(current.input ?? {}, key) ? current : ({ ...current, input: { ...current.input, [key]: { type: 'reference', path: ['args'] } } }), at('input'))} />
    </Section>}
    {node.type === 'tool' && <ValueEditor label={tx('workflows:inspector.tool-arguments')} address={at('arguments')} value={node.arguments} edit={edit} />}
    {node.type === 'parallel' && <Section title={tx('workflows:inspector.parallel-input')}>
      {Object.entries(node.branches).sort(([a], [b]) => a.localeCompare(b)).map(([key, branch]) => <ValueEditor key={key} label={key} address={at('branches', { key }, 'input')} value={branch.input} edit={edit} />)}
    </Section>}
    {node.type === 'review' && <p className={css.note}>{tx('workflows:inspector.review-note')}</p>}
    {node.type === 'loop' && <ValueEditor label={tx('workflows:inspector.loop-input')} address={at('input')} value={node.input} edit={edit} />}
  </>;

  return <>
    {node.type === 'agent' && <JsonField label={tx('workflows:inspector.agent-output')} hint={tx('workflows:inspector.schema-hint')} value={node.output} rows={10} address={at('output')} />}
    {node.type === 'model' && <>
      <SelectField label={tx('workflows:inspector.model-output-mode')} value={node.output.type}
        options={[{ value: 'text', label: tx('workflows:inspector.model-text') }, { value: 'json_schema', label: tx('workflows:inspector.model-json-schema') }]}
        onChange={type => update('model', current => ({ ...current, output: type === 'text' ? { type } : { type, schema: current.output.type === 'json_schema' ? current.output.schema : { type: 'object', properties: {}, required: [], additionalProperties: false } } }), at('output'))} />
      {node.output.type === 'json_schema'
        ? <JsonField label={tx('workflows:inspector.model-output-schema')} hint={tx('workflows:inspector.schema-hint')} value={node.output.schema} rows={10} address={at('output', 'schema')} />
        : <p className={css.note}>{tx('workflows:inspector.model-text-hint')}</p>}
      {derivedSection(tx('workflows:inspector.parallel-output'))}
    </>}
    {node.type === 'tool' && <>
      <SelectField label={tx('workflows:inspector.tool-result')} value={node.result.type}
        options={[{ value: 'json', label: tx('workflows:inspector.tool-result-json') }, { value: 'text', label: tx('workflows:inspector.tool-result-text') }]}
        onChange={type => update('tool', current => ({ ...current, result: type === 'json' ? { type, part: current.result.part, schema: current.result.type === 'json' ? current.result.schema : { type: 'object' } } : { type, part: current.result.part } }), at('result'))} />
      <Field label={tx('workflows:inspector.tool-part')}>{fieldId => <input id={fieldId} type="number" min={0} className={css.input} value={node.result.part}
        onChange={event => { const part = Number(event.target.value); if (Number.isSafeInteger(part) && part >= 0) update('tool', current => ({ ...current, result: { ...current.result, part } }), at('result', 'part')); }} />}</Field>
      {node.result.type === 'json' && <JsonField label={tx('workflows:inspector.tool-schema')} hint={tx('workflows:inspector.schema-hint')} value={node.result.schema} rows={8} address={at('result', 'schema')} />}
    </>}
    {(node.type === 'parallel' || node.type === 'loop' || node.type === 'review') && derivedSection(tx('workflows:inspector.parallel-output'))}
  </>;
}

/** Keyboard-reachable connections: one successor choice per native port. */
function Successors({ document, block, id, edit }: { document: WorkflowDocument; block: BlockPath; id: string; edit: Edit }) {
  const tx = useTranslation();
  const owner = blockAt(document, block)!;
  const ports = outputPorts(nodeAt(owner, id)!.type);
  if (!ports.length) return null;
  const edges = edgesOf(owner);
  const choose = (port: ControlPort, to: string) => edit(current => {
    const index = edgesOf(blockAt(current, block)!).findIndex(edge => edge.from === id && edgePort(edge) === port);
    if (!to) return index < 0 ? current : disconnect(current, block, [index]);
    return index < 0 ? connect(current, block, id, port, to) : reconnect(current, block, index, id, port, to);
  });
  return <Section title={tx('workflows:inspector.successors')}>
    {ports.map(port => {
      const current = edges.find(edge => edge.from === id && edgePort(edge) === port)?.to ?? '';
      return <SelectField key={port} label={tx('workflows:inspector.successor-for', { p0: tx(`workflows:port.${port}`) })} value={current}
        options={[{ value: '', label: tx('workflows:inspector.successor-none') }, ...Object.keys(owner.nodes).filter(other => other !== id).sort().map(other => ({ value: other, label: other }))]}
        onChange={to => choose(port, to)} />;
    })}
  </Section>;
}

/** The trusted static invocation override. Absent keeps the profile's
 * defaults; present replaces exactly the authored dimensions, including an
 * explicit-empty selection. */
function OverrideField({ node, address, onChange }: { node: Of<'agent'>; address: FieldAddress; onChange: (value: SubagentInvocationOverride | undefined) => void }) {
  const tx = useTranslation();
  const present = node.override !== undefined;
  return <Section title={tx('workflows:inspector.agent-override')}>
    <label className={css.check}><input type="checkbox" checked={present} onChange={event => onChange(event.target.checked ? {} : undefined)} />{tx('workflows:inspector.override-enabled')}</label>
    {present && <JsonField label={tx('workflows:inspector.agent-override')} value={node.override} rows={6} address={address} />}
    <p className={css.note}>{tx('workflows:inspector.agent-override-hint')}</p>
  </Section>;
}

/** The node `id` of the document being edited, of kind `K`; a node that
 * is gone, or of another kind now, is not this form's node any more. */
function currentNode<K extends NodeDefinition['type']>(document: WorkflowDocument, block: BlockPath, id: string, expectedKind: K): Of<K> {
  const owner = blockAt(document, block);
  const node = owner && nodeAt(owner, id);
  if (!node || node.type !== expectedKind) throw new CommandRejected('missing_node');
  return node as Of<K>;
}

/** The inspector tab that shows the field at `address` of a node of `kind`. */
export function tabOfField(kind: NodeDefinition['type'], address: FieldAddress): InspectorTab {
  const [first, , third] = address.path;
  if (address.role === 'key') return 'config';
  if (kind === 'agent' || kind === 'model') return first === 'output' ? 'output' : first === 'input' ? 'input' : 'config';
  if (kind === 'tool') return first === 'result' ? 'output' : first === 'arguments' ? 'input' : 'config';
  if (kind === 'parallel') return first === 'branches' && third === 'input' ? 'input' : 'config';
  if (kind === 'loop') return first === 'input' ? 'input' : 'config';
  return 'config';
}

/** The native authored path of a node, the key of its derived output schema. */
export function derivedKey(block: BlockPath, id: string) {
  return `${nativeBlockPath(block)}.nodes.${id}`;
}
