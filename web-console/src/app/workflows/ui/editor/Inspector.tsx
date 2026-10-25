import type { ArrayEdit } from '../../document/fields';
import { useEffect, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import type { WorkflowDiagnostic, WorkflowVocabulary } from '../../../../../../protocol/app-server/v57';
import { useTranslation } from '../../../../locale/react';
import { IconCheckOutline14, IconCloseOutline16, IconWarningOutline16 } from '../../../../presentation/primitives/icons';
import { disconnect } from '../../document/commands';
import { locate, type DiagnosticLocation } from '../../document/diagnostics';
import { blockAt, edgesOf, nodeAt, type BlockPath, type WorkflowDocument } from '../../document/model';
import { edgePort } from '../../document/ports';
import type { WorkflowScopePort } from '../../port';
import type { CheckResult } from '../../machines/editor';
import { IconBlock, KindIcon } from '../icons';
import type { Selection } from './Canvas';
import { BlockForm } from './forms/BlockForm';
import { derivedKey, NodeForm, tabsOf, type InspectorTab } from './forms/NodeForms';
import css from './Editor.module.css';

type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => void;

export type CheckState = 'not_run' | 'passed' | 'failed' | 'outdated';

export function checkState(check: CheckResult | undefined, generation: number): CheckState {
  if (!check) return 'not_run';
  if (check.generation !== generation) return 'outdated';
  return check.report.structure.status === 'valid' ? 'passed' : 'failed';
}

/** The contextual right inspector. It exists only while something is
 * selected; it never reserves an empty column. */
export function Inspector({ document, block, selection, onSelect, edit, onRenamed, navigate, vocabulary, check, generation, rejection, checks, port }: {
  document: WorkflowDocument; block: BlockPath; selection: NonNullable<Selection>; onSelect: (selection: Selection) => void;
  edit: Edit; onRenamed: (from: string, to: string) => void; navigate: (block: BlockPath) => void; vocabulary?: WorkflowVocabulary; check?: CheckResult; generation: number;
  rejection?: string; checks: ReactNode; port: WorkflowScopePort;
}) {
  const tx = useTranslation();
  const owner = blockAt(document, block)!;
  const node = selection.kind === 'node' ? nodeAt(owner, selection.id) : undefined;
  const [tab, setTab] = useState<InspectorTab>('config');
  const nodeId = selection.kind === 'node' ? selection.id : undefined;
  // A new node opens on Configure; locating a field opens the tab it is on.
  const requested = selection.kind === 'node' ? selection.tab : undefined;
  useEffect(() => setTab(requested ?? 'config'), [nodeId, requested, requested && selection]);
  const state = checkState(check, generation);
  const outputs = check?.report.structure.status === 'valid' ? check.report.structure.outputs : undefined;
  let head, body;
  if (selection.kind === 'node' && node) {
    const tabs = tabsOf(node.type);
    head = <div className={css.inspectTitle}>
      <span className={clsx(css.nodeIcon, css[`kind_${node.type}`])}><KindIcon kind={node.type} size={17} /></span>
      <span className={css.nodeText}><span className={css.nodeTitle}>{selection.id}</span><span className={css.nodeType}>{tx(`workflows:kind.${node.type}`)} · {selection.id}</span></span>
    </div>;
    body = <>
      {tabs.length > 1 && <div className={css.inspectTabs} role="tablist">
        {tabs.map(name => <button key={name} type="button" role="tab" aria-selected={tab === name} className={clsx(css.inspectTab, tab === name && css.inspectTabActive)} onClick={() => setTab(name)}>{tx(`workflows:inspector.tab-${name}`)}</button>)}
      </div>}
      <div className={css.inspectBody} role={tabs.length > 1 ? 'tabpanel' : undefined}>
        <NodeForm port={port} document={document} block={block} id={selection.id} tab={tabs.includes(tab) ? tab : 'config'} vocabulary={vocabulary} edit={edit} onRenamed={onRenamed} navigate={navigate} rejected={rejection}
          derived={outputs ? { schema: Object.hasOwn(outputs, derivedKey(block, selection.id)) ? outputs[derivedKey(block, selection.id)] : undefined, current: state === 'passed' } : undefined} />
      </div>
    </>;
  } else if (selection.kind === 'edge' && edgesOf(owner)[selection.index]) {
    const edge = edgesOf(owner)[selection.index];
    head = <div className={css.inspectTitle}><span className={css.nodeText}><span className={css.nodeTitle}>{edge.from} → {edge.to}</span><span className={css.nodeType}>{tx(`workflows:port.${edgePort(edge)}`)}</span></span></div>;
    body = <div className={css.inspectBody}>
      <button type="button" className={clsx(css.smallButton, css.dangerText)} onClick={() => { edit(current => disconnect(current, block, [selection.index])); onSelect(undefined); }}>{tx('workflows:inspector.remove', { p0: `${edge.from} → ${edge.to}` })}</button>
    </div>;
  } else if (selection.kind === 'check') {
    head = <div className={css.inspectTitle}><span className={css.nodeText}><span className={css.nodeTitle}>{tx('workflows:check.problems')}</span></span></div>;
    body = <div className={css.inspectBody}>{checks}</div>;
  } else {
    head = <div className={css.inspectTitle}>
      <span className={css.nodeIcon}><IconBlock size={17} /></span>
      <span className={css.nodeText}><span className={css.nodeTitle}>{block.length ? tx('workflows:inspector.block-title') : tx('workflows:inspector.workflow-title')}</span><span className={css.nodeType}>{block.length ? block.map(step => step.branch ?? 'body').join(' / ') : tx('workflows:canvas.root')}</span></span>
    </div>;
    body = <div className={css.inspectBody}><BlockForm document={document} block={block} edit={edit} /></div>;
  }
  return <aside className={css.inspector} aria-label={tx('workflows:inspector.label')} onKeyDown={event => { if (event.key === 'Escape' && !(event.target as Element).closest('textarea, input, select')) onSelect(undefined); }}>
    <header className={css.inspectHead}>
      {head}
      <button type="button" className={css.iconButton} aria-label={tx('workflows:inspector.close')} onClick={() => onSelect(undefined)}><IconCloseOutline16 /></button>
    </header>
    {body}
    <footer className={css.inspectFooter}>
      <button type="button" className={clsx(css.checkStatus, css[`check_${state}`])} onClick={() => onSelect({ kind: 'check' })}>
        {state === 'passed' ? <IconCheckOutline14 /> : state === 'failed' ? <IconWarningOutline16 /> : null}
        {tx(state === 'passed' ? 'workflows:check.passed' : state === 'failed' ? 'workflows:check.failed' : state === 'outdated' ? 'workflows:check.outdated' : 'workflows:check.not-run')}
      </button>
      <span className={css.muted}>{tx('workflows:inspector.footer-scope')}</span>
    </footer>
  </aside>;
}

/** Native check results: structure and dependencies as separate facts. */
export function CheckResults({ check, generation, global, workspaces, context, setContext, run, go }: {
  check?: CheckResult; generation: number; global: boolean;
  workspaces: readonly { id: string; displayName: string; displayPath: string }[]; context?: string; setContext: (id: string | undefined) => void;
  run: () => void; go: (location: DiagnosticLocation) => void;
}) {
  const tx = useTranslation();
  const state = checkState(check, generation);
  const diagnostic = (value: WorkflowDiagnostic) => <li className={css.problem}>
    <code>{value.path}</code><span>{value.reason}</span>
    {value.detail && <small>{value.detail}</small>}
    {value.line != null && <small>{tx('workflows:check.line', { p0: value.line, p1: value.column ?? 0 })}</small>}
    {value.path !== '$' && <button type="button" className={css.linkButton} onClick={() => go(locate(value.path))}>{tx('workflows:check.go')}</button>}
  </li>;
  const dependencies = check && (!global || check.context === context) ? check.report.dependencies : undefined;
  const name = (path: string) => workspaces.find(row => row.displayPath === path)?.displayName ?? path;
  return <>
    <p className={clsx(css.checkStatus, css[`check_${state}`])}>{tx(state === 'passed' ? 'workflows:check.passed' : state === 'failed' ? 'workflows:check.failed' : state === 'outdated' ? 'workflows:check.outdated' : 'workflows:check.not-run')}</p>
    <h4 className={css.groupHeading}>{tx('workflows:check.structure')}</h4>
    {check?.report.structure.status === 'invalid' && <ul className={css.problems}>{diagnostic(check.report.structure.diagnostic)}</ul>}
    <h4 className={css.groupHeading}>{tx('workflows:check.dependencies')}</h4>
    {global && <label className={css.field}><span className={css.fieldLabel}>{tx('workflows:check.dependencies-context')}</span>
      <select className={css.input} value={context ?? ''} onChange={event => setContext(event.target.value || undefined)}>
        <option value="">{tx('workflows:check.dependencies-not-checked')}</option>
        {workspaces.map(row => <option key={row.id} value={row.id}>{row.displayName}</option>)}
      </select></label>}
    {(!dependencies || dependencies.status === 'not_checked') && <p className={css.note}>{global && !context ? tx('workflows:check.dependencies-choose') : tx('workflows:check.dependencies-not-checked')}</p>}
    {dependencies?.status === 'admitted' && <p className={css.note}>{tx('workflows:check.dependencies-admitted', { p0: name(dependencies.context) })}</p>}
    {dependencies?.status === 'requires_attempt' && <p className={css.note}>{tx('workflows:check.dependencies-requires-attempt', { p0: dependencies.model_nodes.join(', '), p1: name(dependencies.context) })}</p>}
    {dependencies?.status === 'unavailable' && <p className={css.note}>{tx('workflows:check.dependencies-unavailable', { p0: name(dependencies.context), p1: dependencies.reason })}</p>}
    {dependencies?.status === 'blocked' && <>
      <p className={css.note}>{tx('workflows:check.dependencies-blocked', { p0: dependencies.diagnostics.length, p1: name(dependencies.context) })}</p>
      <ul className={css.problems}>{dependencies.diagnostics.map((item, index) => <li key={index} className={css.problem}>
        <code>{item.path}</code><span>{item.reason.kind.replaceAll('_', ' ')}{'detail' in item.reason && item.reason.detail && typeof item.reason.detail === 'object' ? ` · ${JSON.stringify(item.reason.detail)}` : ''}</span>
        <button type="button" className={css.linkButton} onClick={() => go(locate(item.path))}>{tx('workflows:check.go')}</button>
      </li>)}</ul>
    </>}
    <p className={css.note}>{tx('workflows:check.dependency-note')}</p>
    <button type="button" className={css.smallButton} onClick={run}>{tx('workflows:editor.check')}</button>
  </>;
}
