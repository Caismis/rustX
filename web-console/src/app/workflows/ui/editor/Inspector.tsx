import { encodeField, type ArrayEdit, type FieldAddress } from '../../document/fields';
import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import clsx from 'clsx';
import type { WorkflowDiagnostic, WorkflowVocabulary } from '../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../locale/react';
import { IconCheckOutline14, IconCloseOutline16, IconWarningOutline16 } from '../../../../presentation/primitives/icons';
import { deleteNodes, renameNode } from '../../document/commands';
import { nodePresentation, putNote, removeNotes } from '../../document/geometry';
import { nodeField } from '../../document/fields';
import { locate, type DiagnosticLocation } from '../../document/diagnostics';
import { blockAt, nodeAt, startOf, type BlockPath, type WorkflowDocument } from '../../document/model';
import type { WorkflowScopePort } from '../../port';
import type { CheckResult } from '../../machines/editor';
import { IconBlock, IconEntry, KindIcon } from '../icons';
import type { Duplicate, Selection } from './selection';
import { BlockForm } from './forms/BlockForm';
import { NodePanel } from './panels/NodePanel';
import { DraftTextField, KeyField, NumberField, SelectField } from './forms/fields';
import { readInspectorWidth, writeInspectorWidth } from './view-store';
import { viewId } from './selection';
import css from './Editor.module.css';

type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => void;
export type CheckState = 'not_run' | 'passed' | 'failed' | 'outdated';
export function checkState(check: CheckResult | undefined, generation: number): CheckState {
  if (!check) return 'not_run';
  if (check.generation !== generation) return 'outdated';
  return check.report.structure.status === 'valid' ? 'passed' : 'failed';
}

/** A single Settings surface; specialized panels own node semantics and the
 * editor machine owns all deferred text, history and save state. */
export function Inspector({ document, block, selection, onSelect, edit, navigate, vocabulary, check, generation, rejection, checks, port, collapsed, toggleContainer, focus, onDuplicate }: {
  document: WorkflowDocument; block: BlockPath; selection: NonNullable<Selection>; onSelect: (selection: Selection) => void;
  edit: Edit; navigate: (block: BlockPath) => void; vocabulary?: WorkflowVocabulary; check?: CheckResult; generation: number;
  rejection?: string; checks: ReactNode; port: WorkflowScopePort;
  collapsed: ReadonlySet<string>; toggleContainer: (path: BlockPath, id: string) => void;
  focus?: { address?: FieldAddress; field?: string; epoch: number };
  onDuplicate: Duplicate;
}) {
  const tx = useTranslation(), path = 'path' in selection ? selection.path ?? block : block;
  const root = useRef<HTMLElement>(null);
  const selectionKey = JSON.stringify(selection);
  useEffect(() => {
    if (!focus || !root.current) return;
    const controls = [...root.current.querySelectorAll<HTMLElement>('[data-workflow-field]')];
    const exact = focus.address && encodeField(focus.address);
    const control = exact ? controls.find(control => control.dataset.workflowField === exact) : controls.find(control => {
      try {
        const tuple = JSON.parse(control.dataset.workflowField!);
        const path = tuple[0] === 'workflow' ? [tuple[1]] : tuple[2];
        return Array.isArray(path) && path.length && focus.field?.startsWith(String(typeof path[0] === 'object' ? path[0].key : path[0]));
      } catch { return false; }
    });
    if (!control) return;
    for (let parent = control.parentElement; parent && parent !== root.current; parent = parent.parentElement) if (parent instanceof HTMLDetailsElement) parent.open = true;
    control.scrollIntoView?.({ block: 'nearest' }); control.focus({ preventScroll: true });
  }, [focus, selectionKey]);
  const owner = blockAt(document, path), node = selection.kind === 'node' && owner ? nodeAt(owner, selection.id) : undefined;
  const [width, setWidth] = useState(readInspectorWidth), [resizing, setResizing] = useState(false);
  const resize = useRef({ x: 0, width });
  const changeWidth = (value: number) => { const next = Math.max(320, Math.min(640, value)); setWidth(next); writeInspectorWidth(next); };
  useEffect(() => {
    if (!resizing) return;
    const move = (event: PointerEvent) => changeWidth(resize.current.width + resize.current.x - event.clientX);
    const stop = () => setResizing(false);
    window.addEventListener('pointermove', move); window.addEventListener('pointerup', stop, { once: true });
    return () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', stop); };
  }, [resizing]);
  const state = checkState(check, generation);
  let head: ReactNode, body: ReactNode, description: ReactNode;
  if (selection.kind === 'node' && node && owner) {
    const id = selection.id, metadata = nodePresentation(document, path, id);
    head = <div className={css.inspectTitle}>
      <span className={clsx(css.nodeIcon, css[`kind_${node.type}`])}><KindIcon kind={node.type} size={17} /></span>
      <span className={css.nodeText}><DraftTextField compact label={tx('workflows:panel.title')} placeholder={tx('workflows:panel.title-placeholder')} value={metadata?.title || id} address={{ ...nodeField(path, id, ['title']), domain: { kind: 'node_presentation' } }} />{node.type !== 'branch' && <span className={css.nodeType}>{tx(`workflows:kind.${node.type}`)}</span>}</span>
    </div>;
    description = <DraftTextField bare label={tx('workflows:panel.description')} placeholder={tx('workflows:panel.description-placeholder')} value={metadata?.description ?? ''} multiline address={{ ...nodeField(path, id, ['description']), domain: { kind: 'node_presentation' } }} />;
    body = <>
      {node.type === 'loop' && <button type="button" className={css.smallButton} onClick={() => toggleContainer(path, id)}>{tx(collapsed.has(viewId(path, 'node', id)) ? 'workflows:canvas.expand' : 'workflows:canvas.collapse', { p0: metadata?.title ?? id })}</button>}
      <NodePanel document={document} block={path} id={id} port={port} vocabulary={vocabulary} edit={edit} navigate={navigate} onSelect={onSelect} />
      <details className={css.advanced}><summary>{tx('workflows:panel.identity')}</summary>
        <KeyField label={tx('workflows:inspector.node-id')} hint={tx('workflows:inspector.rename-hint')} value={id} address={nodeField(path, id, [], 'key')} rename={(document, from, to) => renameNode(document, path, from, to)} />
      </details>
      <div className={css.row}><button type="button" className={css.smallButton} onClick={() => onDuplicate(path, [id], [])}>{tx('workflows:canvas.duplicate')}</button>
        <button type="button" className={clsx(css.smallButton, css.dangerText)} onClick={() => edit(document => deleteNodes(document, path, [id]))}>{tx('workflows:inspector.delete-node')}</button></div>
    </>;
  } else if (selection.kind === 'check') {
    head = <span className={css.nodeTitle}>{tx('workflows:check.problems')}</span>; body = checks;
  } else if (selection.kind === 'note') {
    const note = document.editor?.notes?.find(note => note.id === selection.id);
    head = <span className={css.nodeTitle}>{tx('workflows:note.title')}</span>;
    body = note && <>
      <DraftTextField label={tx('workflows:note.text')} value={note.text} multiline address={{ block: path, path: ['text'], role: 'value', domain: { kind: 'note', id: note.id } }} />
      <SelectField label={tx('workflows:note.theme')} value={note.theme} options={(['neutral', 'blue', 'green', 'amber', 'rose'] as const).map(value => ({ value, label: tx(`workflows:note.${value}`) }))} onChange={theme => edit(document => putNote(document, { ...note, theme }))} />
      {(['width', 'height'] as const).map(dimension => <NumberField key={dimension} label={tx(`workflows:note.${dimension}`)} value={note.dimensions[dimension]} min={16} max={8192} address={{ block: path, path: ['dimensions', dimension], role: 'value', domain: { kind: 'note', id: note.id } }} />)}
      <div className={css.row}><button type="button" className={css.smallButton} onClick={() => onDuplicate(path, [], [note.id])}>{tx('workflows:canvas.duplicate')}</button>
        <button type="button" className={css.smallButton} onClick={() => { edit(document => removeNotes(document, [note.id])); onSelect(undefined); }}>{tx('workflows:note.delete')}</button></div>
    </>;
  } else {
    const startId = owner && startOf(owner), entry = selection.kind === 'start' && startId ? nodePresentation(document, path, startId) : undefined;
    const startTitle = tx(path.length ? 'workflows:canvas.nested-start' : 'workflows:canvas.start-title');
    const entryField = (field: 'title' | 'description'): FieldAddress => ({ block: path, node: startId, path: [field], role: 'value', domain: { kind: 'node_presentation' } });
    head = selection.kind === 'start'
      // As a node: the title is an inline heading that reverts to its default when cleared.
      ? <div className={css.inspectTitle}><span className={clsx(css.nodeIcon, css.startIcon)}><IconEntry size={17} /></span><span className={css.nodeText}>
        <DraftTextField compact label={tx('workflows:panel.title')} placeholder={startTitle} value={entry?.title || startTitle} address={entryField('title')} />
        <span className={css.nodeType}>{tx(path.length ? 'workflows:panel.body-input' : 'workflows:panel.arguments')}</span></span></div>
      : <div className={css.inspectTitle}><span className={css.nodeIcon}><IconBlock size={17} /></span><span className={css.nodeText}><span className={css.nodeTitle}>{tx(path.length ? 'workflows:inspector.block-title' : 'workflows:inspector.workflow-title')}</span></span></div>;
    if (selection.kind === 'start') description = <DraftTextField bare label={tx('workflows:panel.description')} placeholder={tx('workflows:panel.description-placeholder')} value={entry?.description ?? ''} multiline address={entryField('description')} />;
    body = owner && <BlockForm document={document} block={path} edit={edit} onSelect={onSelect} start={selection.kind === 'start'} />;
  }
  return <aside ref={root} className={css.inspector} style={{ '--inspector-width': `${width}px` } as CSSProperties} aria-label={tx('workflows:inspector.label')} onWheel={event => event.stopPropagation()}
    onKeyDown={event => { if (event.key === 'Escape' && !event.isDefaultPrevented() && !(event.target as Element).closest('textarea, input, select, [role="dialog"], [role="listbox"]')) { event.stopPropagation(); onSelect(undefined); } }}>
    <div className={css.inspectorResize} role="separator" aria-label={tx('workflows:panel.resize')} aria-orientation="vertical" aria-valuemin={320} aria-valuemax={640} aria-valuenow={width} tabIndex={0}
      onPointerDown={event => { event.preventDefault(); resize.current = { x: event.clientX, width }; setResizing(true); }}
      onKeyDown={event => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); changeWidth(width + (event.key === 'ArrowLeft' ? 20 : -20)); } }} />
    {/* As Dify's node panel: the description sits under the title, in the header. */}
    <header className={css.inspectHead}>
      <div className={css.inspectHeadRow}>{head}<button type="button" className={css.iconButton} aria-label={tx('workflows:inspector.close')} onClick={() => onSelect(undefined)}><IconCloseOutline16 /></button></div>
      {description && <div className={css.inspectDescription} key={selection.kind === 'node' ? `node:${JSON.stringify(path)}:${selection.id}` : `${selection.kind}:${JSON.stringify(path)}`}>{description}</div>}
    </header>
    <div className={clsx(css.inspectBody, 'nowheel')} key={selection.kind === 'node' || selection.kind === 'note' ? `${selection.kind}:${JSON.stringify(path)}:${selection.id}` : `${selection.kind}:${JSON.stringify(path)}`}>
      {rejection && <p role="alert" className={css.hintError}>{rejection}</p>}{body}
    </div>
    <footer className={css.inspectFooter}><button type="button" className={clsx(css.checkStatus, css[`check_${state}`])} onClick={() => onSelect({ kind: 'check' })}>
      {state === 'passed' ? <IconCheckOutline14 /> : state === 'failed' ? <IconWarningOutline16 /> : null}{tx(state === 'passed' ? 'workflows:check.passed' : state === 'failed' ? 'workflows:check.failed' : state === 'outdated' ? 'workflows:check.outdated' : 'workflows:check.not-run')}
    </button><span className={css.muted}>{tx('workflows:inspector.footer-scope')}</span></footer>
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
    {!!check?.report.presentation?.length && <ul className={css.problems}>{check.report.presentation.map(item => <li key={item.path} className={css.problem}><code>{item.path}</code><span>{item.reason}</span></li>)}</ul>}
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
