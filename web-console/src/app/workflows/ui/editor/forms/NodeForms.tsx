import { useId } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../../../../locale/react';
import type { SubagentInvocationOverride, WorkflowVocabulary } from '../../../../../../../protocol/app-server/v51';
import {
  addBranch, connect, deleteNodes, disconnect, reconnect, removeBranch, renameBranch, replaceNode, setEntry,
} from '../../../document/commands';
import { blockAt, deepNodeCount, edgesOf, nativeBlockPath, nodeAt, type BlockPath, type NodeDefinition, type WorkflowDocument } from '../../../document/model';
import { edgePort, outputPorts, type ControlPort } from '../../../document/ports';
import { AddKey, PredicateEditor, ValueEditor } from './ValueEditor';
import { useFieldDraft } from './drafts';
import { Field, IdentifierField, JsonField, Section, SelectField, TextField } from './fields';
import css from '../Editor.module.css';

export type InspectorTab = 'config' | 'input' | 'output';
type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string) => void;
type Of<K extends NodeDefinition['type']> = Extract<NodeDefinition, { type: K }>;

export function tabsOf(kind: NodeDefinition['type']): readonly InspectorTab[] {
  return kind === 'branch' || kind === 'return' ? ['config'] : ['config', 'input', 'output'];
}

export interface Derived { readonly schema?: unknown; readonly current: boolean }

/** The inspector forms of one node. Each control writes exactly its native
 * field through a structured command against the current document. */
export function NodeForm({ document, block, id, tab, vocabulary, derived, edit, rename, navigate, rejected }: {
  document: WorkflowDocument; block: BlockPath; id: string; tab: InspectorTab; vocabulary?: WorkflowVocabulary;
  derived?: Derived; edit: Edit; rename: (from: string, to: string) => boolean; navigate: (block: BlockPath) => void; rejected?: string;
}) {
  const tx = useTranslation();
  const owner = blockAt(document, block)!;
  const node = nodeAt(owner, id);
  const listId = useId();
  if (!node) return null;
  /** Update this node from its value in the document being edited. */
  const update = <K extends NodeDefinition['type']>(change: (node: Of<K>) => Of<K>, field: string) =>
    edit(current => replaceNode(current, block, id, change(blockAt(current, block)!.nodes[id] as Of<K>)), fieldKey(block, id, field));
  const derivedSection = (title: string) => <Section title={title}>
    {derived?.schema !== undefined
      ? <><pre className={css.codebox}>{JSON.stringify(derived.schema, null, 2)}</pre><p className={css.note}>{derived.current ? tx('workflows:inspector.derived') : tx('workflows:inspector.derived-outdated')}</p></>
      : <p className={css.note}>{tx('workflows:inspector.derived-unknown')}</p>}
  </Section>;

  if (tab === 'config') return <>
    <IdentifierField label={tx('workflows:inspector.node-id')} hint={tx('workflows:inspector.rename-hint')} value={id} error={rejected} draftKey={fieldKey(block, id, 'id')}
      onCommit={next => rename(id, next)} />
    <div className={css.row}>
      {owner.entry === id
        ? <span className={css.note}>{tx('workflows:inspector.is-entry')}</span>
        : <button type="button" className={css.smallButton} onClick={() => edit(current => setEntry(current, block, id))}>{tx('workflows:inspector.set-entry')}</button>}
      <button type="button" className={clsx(css.smallButton, css.dangerText)} onClick={() => edit(current => deleteNodes(current, block, [id]))}>{tx('workflows:inspector.delete-node')}</button>
    </div>
    {node.type === 'agent' && <>
      <TextField label={tx('workflows:inspector.agent-profile')} hint={tx('workflows:inspector.agent-profile-hint')} value={node.profile} mono list={listId} invalid={!node.profile}
        onChange={(profile, session) => edit(current => replaceNode(current, block, id, { ...(blockAt(current, block)!.nodes[id] as Of<'agent'>), profile }), session)} />
      <datalist id={listId}>{vocabulary?.profiles.map(name => <option key={name} value={name} />)}</datalist>
      <TextField label={tx('workflows:inspector.agent-task')} value={node.task} multiline
        onChange={(task, session) => edit(current => replaceNode(current, block, id, { ...(blockAt(current, block)!.nodes[id] as Of<'agent'>), task }), session)} />
      <OverrideField node={node} draftKey={fieldKey(block, id, 'override')} onChange={invocation => update<'agent'>(current => {
        const { override: _previous, ...rest } = current;
        return invocation === undefined ? rest : { ...rest, override: invocation };
      }, 'override')} />
    </>}
    {node.type === 'tool' && <>
      <SelectField label={tx('workflows:inspector.tool-origin')} value={node.selector.origin}
        options={[{ value: 'builtin', label: tx('workflows:inspector.tool-origin-builtin') }, { value: 'source', label: tx('workflows:inspector.tool-origin-source') }]}
        onChange={origin => update<'tool'>(current => ({ ...current, selector: origin === 'builtin' ? { origin, name: current.selector.name } : { origin, source_id: current.selector.origin === 'source' ? current.selector.source_id : '', name: current.selector.name } }), 'selector')} />
      {node.selector.origin === 'source' && <TextField label={tx('workflows:inspector.tool-source')} value={node.selector.source_id} mono
        onChange={(source_id, session) => edit(current => replaceNode(current, block, id, { ...(blockAt(current, block)!.nodes[id] as Of<'tool'>), selector: { origin: 'source', source_id, name: node.selector.name } }), session)} />}
      <TextField label={tx('workflows:inspector.tool-name')} value={node.selector.name} mono list={node.selector.origin === 'builtin' ? listId : undefined} invalid={!node.selector.name}
        onChange={(name, session) => edit(current => {
          const tool = blockAt(current, block)!.nodes[id] as Of<'tool'>;
          return replaceNode(current, block, id, { ...tool, selector: { ...tool.selector, name } });
        }, session)} />
      <datalist id={listId}>{vocabulary?.builtin_tools.map(name => <option key={name} value={name} />)}</datalist>
    </>}
    {node.type === 'branch' && <>
      <PredicateEditor label={tx('workflows:inspector.branch-condition')} field={fieldKey(block, id, 'condition')} predicate={node.condition} onChange={condition => update<'branch'>(current => ({ ...current, condition }), 'condition')} />
      <p className={css.note}>{tx('workflows:inspector.branch-ports')}</p>
    </>}
    {node.type === 'parallel' && <Section title={tx('workflows:inspector.parallel-branches')} aside={String(Object.keys(node.branches).length)}>
      {Object.entries(node.branches).sort(([a], [b]) => a.localeCompare(b)).map(([key, branch]) => <div key={key} className={css.branchRow}>
        <BranchKey value={key} draftKey={fieldKey(block, id, `branches.${key}.key`)} label={tx('workflows:inspector.parallel-rename', { p0: key })} onCommit={next => edit(current => renameBranch(current, block, id, key, next))} />
        <span className={css.branchMeta}>{tx('workflows:inspector.parallel-branch-nodes', { p0: deepNodeCount(branch.block) })}</span>
        <button type="button" className={css.linkButton} onClick={() => navigate([...block, { node: id, branch: key }])}>{tx('workflows:inspector.parallel-edit')} →</button>
        <button type="button" className={css.pathRemove} aria-label={tx('workflows:inspector.parallel-remove', { p0: key })} disabled={Object.keys(node.branches).length === 1}
          onClick={() => edit(current => removeBranch(current, block, id, key))}>×</button>
      </div>)}
      <AddKey existing={Object.keys(node.branches)} label={tx('workflows:inspector.parallel-new-key')} action={tx('workflows:inspector.parallel-add')} onAdd={key => edit(current => addBranch(current, block, id, key))} />
      <p className={css.note}>{tx('workflows:inspector.parallel-note')}</p>
    </Section>}
    {node.type === 'review' && <>
      <ValueEditor label={tx('workflows:inspector.review-subject')} field={fieldKey(block, id, 'subject.value')} value={node.subject.value} onChange={value => update<'review'>(current => ({ ...current, subject: { ...current.subject, value } }), 'subject')} />
      <Section title={tx('workflows:inspector.review-context')} aside={String(node.context.length)}>
        {node.context.map((item, index) => <div key={index} className={css.nestedRow}>
          <ValueEditor label={`[${index}]`} field={fieldKey(block, id, `context.${index}`)} value={item} onChange={value => update<'review'>(current => ({ ...current, context: current.context.map((entry, at) => at === index ? value : entry) }), `context.${index}`)} />
          <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: `[${index}]` })} onClick={() => update<'review'>(current => ({ ...current, context: current.context.filter((_, at) => at !== index) }), 'context')}>×</button>
        </div>)}
        <button type="button" className={css.smallButton} onClick={() => update<'review'>(current => ({ ...current, context: [...current.context, { type: 'reference', path: ['args'] }] }), 'context')}>{tx('workflows:inspector.add-item')}</button>
      </Section>
      <p className={css.note}>{tx('workflows:inspector.review-note')}</p>
    </>}
    {node.type === 'loop' && <>
      <Field label={tx('workflows:inspector.loop-max')}>{fieldId => <input id={fieldId} type="number" min={1} max={256} className={css.input} value={node.max_iterations}
        onChange={event => { const value = Number(event.target.value); if (Number.isSafeInteger(value) && value >= 0) update<'loop'>(current => ({ ...current, max_iterations: value }), 'max_iterations'); }} />}</Field>
      <PredicateEditor label={tx('workflows:inspector.loop-until')} field={fieldKey(block, id, 'until')} predicate={node.until} onChange={until => update<'loop'>(current => ({ ...current, until }), 'until')} />
      <p className={css.note}>{tx('workflows:inspector.loop-until-hint')}</p>
      <ValueEditor label={tx('workflows:inspector.loop-carry')} field={fieldKey(block, id, 'carry')} value={node.carry} onChange={carry => update<'loop'>(current => ({ ...current, carry }), 'carry')} />
      <Section title={tx('workflows:inspector.loop-body')} aside={tx('workflows:inspector.parallel-branch-nodes', { p0: deepNodeCount(node.body) })}>
        <button type="button" className={css.linkButton} onClick={() => navigate([...block, { node: id }])}>{tx('workflows:inspector.loop-edit')} →</button>
      </Section>
      <p className={css.note}>{tx('workflows:inspector.loop-ports')}</p>
    </>}
    {node.type === 'return' && <>
      <ValueEditor label={tx('workflows:inspector.return-output')} field={fieldKey(block, id, 'output')} value={node.output} onChange={output => update<'return'>(current => ({ ...current, output }), 'output')} />
      <p className={css.note}>{tx('workflows:inspector.return-note')}</p>
    </>}
    <Successors document={document} block={block} id={id} edit={edit} />
  </>;

  if (tab === 'input') return <>
    {node.type === 'agent' && <Section title={tx('workflows:inspector.agent-input')} aside={String(Object.keys(node.input ?? {}).length)}>
      {Object.entries(node.input ?? {}).map(([key, binding]) => <div key={key} className={css.nestedRow}>
        <ValueEditor label={key} field={fieldKey(block, id, `input.${key}`)} value={binding} onChange={value => update<'agent'>(current => ({ ...current, input: { ...current.input, [key]: value } }), `input.${key}`)} />
        <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: key })}
          onClick={() => update<'agent'>(current => ({ ...current, input: Object.fromEntries(Object.entries(current.input ?? {}).filter(([name]) => name !== key)) }), 'input')}>×</button>
      </div>)}
      <AddKey existing={Object.keys(node.input ?? {})} onAdd={key => update<'agent'>(current => ({ ...current, input: { ...current.input, [key]: { type: 'reference', path: ['args'] } } }), 'input')} />
    </Section>}
    {node.type === 'tool' && <ValueEditor label={tx('workflows:inspector.tool-arguments')} field={fieldKey(block, id, 'arguments')} value={node.arguments} onChange={value => update<'tool'>(current => ({ ...current, arguments: value }), 'arguments')} />}
    {node.type === 'parallel' && <Section title={tx('workflows:inspector.parallel-input')}>
      {Object.entries(node.branches).sort(([a], [b]) => a.localeCompare(b)).map(([key, branch]) => <ValueEditor key={key} label={key} field={fieldKey(block, id, `branches.${key}.input`)} value={branch.input}
        onChange={value => update<'parallel'>(current => ({ ...current, branches: { ...current.branches, [key]: { ...current.branches[key], input: value } } }), `branches.${key}.input`)} />)}
    </Section>}
    {node.type === 'review' && <p className={css.note}>{tx('workflows:inspector.review-note')}</p>}
    {node.type === 'loop' && <ValueEditor label={tx('workflows:inspector.loop-input')} field={fieldKey(block, id, 'input')} value={node.input} onChange={value => update<'loop'>(current => ({ ...current, input: value }), 'input')} />}
  </>;

  return <>
    {node.type === 'agent' && <JsonField label={tx('workflows:inspector.agent-output')} hint={tx('workflows:inspector.schema-hint')} value={node.output} rows={10} draftKey={fieldKey(block, id, 'output')}
      onApply={output => update<'agent'>(current => ({ ...current, output: output as Of<'agent'>['output'] }), 'output')} />}
    {node.type === 'tool' && <>
      <SelectField label={tx('workflows:inspector.tool-result')} value={node.result.type}
        options={[{ value: 'json', label: tx('workflows:inspector.tool-result-json') }, { value: 'text', label: tx('workflows:inspector.tool-result-text') }]}
        onChange={type => update<'tool'>(current => ({ ...current, result: type === 'json' ? { type, part: current.result.part, schema: current.result.type === 'json' ? current.result.schema : { type: 'object' } } : { type, part: current.result.part } }), 'result')} />
      <Field label={tx('workflows:inspector.tool-part')}>{fieldId => <input id={fieldId} type="number" min={0} className={css.input} value={node.result.part}
        onChange={event => { const part = Number(event.target.value); if (Number.isSafeInteger(part) && part >= 0) update<'tool'>(current => ({ ...current, result: { ...current.result, part } }), 'result.part'); }} />}</Field>
      {node.result.type === 'json' && <JsonField label={tx('workflows:inspector.tool-schema')} hint={tx('workflows:inspector.schema-hint')} value={node.result.schema} rows={8} draftKey={fieldKey(block, id, 'result.schema')}
        onApply={schema => update<'tool'>(current => ({ ...current, result: { type: 'json', part: current.result.part, schema } }), 'result.schema')} />}
    </>}
    {(node.type === 'parallel' || node.type === 'loop' || node.type === 'review') && derivedSection(tx('workflows:inspector.parallel-output'))}
  </>;
}

/** Keyboard-reachable connections: one successor choice per native port. */
function Successors({ document, block, id, edit }: { document: WorkflowDocument; block: BlockPath; id: string; edit: Edit }) {
  const tx = useTranslation();
  const owner = blockAt(document, block)!;
  const ports = outputPorts(owner.nodes[id].type);
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
function OverrideField({ node, draftKey, onChange }: { node: Of<'agent'>; draftKey: string; onChange: (value: SubagentInvocationOverride | undefined) => void }) {
  const tx = useTranslation();
  const present = node.override !== undefined;
  return <Section title={tx('workflows:inspector.agent-override')}>
    <label className={css.check}><input type="checkbox" checked={present} onChange={event => onChange(event.target.checked ? {} : undefined)} />{tx('workflows:inspector.override-enabled')}</label>
    {present && <JsonField label={tx('workflows:inspector.agent-override')} value={node.override} rows={6} draftKey={draftKey} onApply={value => onChange(value as SubagentInvocationOverride)} />}
    <p className={css.note}>{tx('workflows:inspector.agent-override-hint')}</p>
  </Section>;
}

/** A branch key, renamed explicitly on Enter, blur, Save or Check. */
function BranchKey({ value, label, onCommit, draftKey }: { value: string; label: string; onCommit: (value: string) => void; draftKey: string }) {
  const field = useFieldDraft({ key: draftKey, canonical: value, valid: text => text.trim() !== '', commit: text => { if (text.trim() !== value) onCommit(text.trim()); return true; } });
  return <input className={clsx(css.input, css.mono, css.branchKey)} aria-label={label} aria-invalid={field.invalid || undefined} value={field.text} spellCheck={false}
    onChange={event => field.change(event.target.value)} onBlur={field.apply}
    onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); field.apply(); } else if (event.key === 'Escape') field.reset(); }} />;
}

export function derivedKey(block: BlockPath, id: string) {
  return `${nativeBlockPath(block)}.nodes.${id}`;
}

/** The full identity of one field of one node in one lexical block: the
 * owner of its coalesced edit session and of its unapplied text. */
export function fieldKey(block: BlockPath, id: string, field: string) {
  return `${derivedKey(block, id)}.${field}`;
}
