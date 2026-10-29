import { CommandRejected } from '../../document/commands';
import { duplicateFragment, fragmentOwnsField, type PastedFragment } from '../../document/clipboard';
import { projectScene } from './scene';
import '@xyflow/react/dist/base.css';
  import { useCallback, useEffect, useId, useMemo, useState, useSyncExternalStore, type ReactNode } from 'react';
import { ReactFlowProvider } from '@xyflow/react';
import { useSelector } from '@xstate/react';
import clsx from 'clsx';
import { useTranslation } from '../../../../locale/react';
import { Button } from '../../../../presentation/primitives/Button';
import { writeClipboard } from '../../../../presentation/primitives/clipboard';
import { Menu } from '../../../../presentation/primitives/Menu';
import { Modal } from '../../../../presentation/primitives/Modal';
import { IconCheckOutline16, IconEllipsisOutline16 } from '../../../../presentation/primitives/icons';
import type { WorkspaceAuthority } from '../../../../workspaces/authority';
import { locate, type DiagnosticLocation } from '../../document/diagnostics';
import { describeField, renameAddress, type ArrayEdit, type FieldAddress } from '../../document/fields';
import { blockAt, nodeAt, sameBlockPath, type BlockPath, type WorkflowDocument } from '../../document/model';
import { isDirty, type EditorNotice, type FieldDraft, type WorkflowEditorActor } from '../../machines/editor';
import { editorHolds, type WorkflowSurfaceActor } from '../../machines/surface';
import { scopeKey } from '../../port';
import { IconArrowLeft, IconExplain, IconFile, IconFolder, IconRedo, IconUndo } from '../icons';
import { WorkflowIcon } from '../WorkflowIcon';
import { FailureText, shortRevision } from '../notices';
import { Canvas, keyboardOwned, type Problems, type Selection } from './Canvas';
import { viewId } from './selection';
import { CheckResults, Inspector } from './Inspector';
import { DraftOwner, FieldCommits } from './forms/drafts';
import { viewKey } from './view-store';
import css from './Editor.module.css';

/** Problems a native diagnostic marks in the block being edited: its own
 * node or edge, or the node owning the nested block it is in. */
function problemsIn(located: { location: DiagnosticLocation; severity: 'error' | 'warning' }[]): Problems {
  const nodes = new Map<string, 'error' | 'warning' | 'nested'>(), edges = new Set<string>();
  for (const { location, severity } of located) {
    for (let index = 0; index < location.block.length; index++) nodes.set(viewId(location.block.slice(0, index), 'node', location.block[index].node), 'nested');
    if (location.node) nodes.set(viewId(location.block, 'node', location.node), severity);
    if (location.edge !== undefined) edges.add(viewId(location.block, 'edge', String(location.edge)));
  }
  return { nodes, edges };
}

export default function WorkflowEditor({ surface, editor, authority, scopeLabel }: {
  surface: WorkflowSurfaceActor; editor: WorkflowEditorActor; authority: WorkspaceAuthority; scopeLabel: string;
}) {
  const tx = useTranslation();
  const snapshot = useSelector(editor, value => value);
  const context = snapshot.context;
  const { target, document, base, block, check, generation, notice } = context;
  const catalog = useSyncExternalStore(authority.subscribe, authority.getCatalog);
  const [selection, choose] = useState<Selection>();
  const [fieldFocus, setFieldFocus] = useState<{ address?: FieldAddress; field?: string; epoch: number }>();
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const toggleContainer = useCallback((path: BlockPath, id: string) => setCollapsed(current => {
    const next = new Set(current), key = viewId(path, 'node', id);
    if (next.has(key)) next.delete(key); else next.add(key);
    return next;
  }), []);
  // Another selection ends the current edit session (one undo step each).
  const setSelection = useCallback((next: Selection) => {
    if (next && 'path' in next && next.path) editor.send({ type: 'NAVIGATE', block: next.path });
    setFieldFocus(undefined);
    choose(next);
    editor.send({ type: 'BOUNDARY' });
  }, [editor]);
  const commits = useMemo(() => new FieldCommits(), []);
  const owner$ = useMemo(() => ({ editor, commits }), [editor, commits]);
  const [dependencyContext, setDependencyContext] = useState<string>();
  const [explaining, setExplaining] = useState(false);
  const [reloading, setReloading] = useState(false);
  const [menu, setMenu] = useState(false);
  const [layoutEpoch, setLayoutEpoch] = useState(0);
  const dirty = isDirty(context);
  const editable = !!document && !!base?.document;
  const saving = snapshot.matches({ ready: 'saving' });
  const checking = snapshot.matches({ ready: 'checking' });
  const key = useMemo(() => viewKey(target.endpoint, scopeKey(target.scope), target.id), [target]);
  const edit = useCallback((apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => editor.send({ type: 'EDIT', apply, coalesce, array }), [editor]);
  const duplicate = useCallback((path: BlockPath, ids: readonly string[], notes: readonly string[]): PastedFragment | undefined => {
    let result: PastedFragment | undefined;
    edit(current => {
      if (Object.values(editor.getSnapshot().context.drafts).some(draft => fragmentOwnsField(path, ids, notes, draft.address))) throw new CommandRejected('pending_input');
      // Accepted selection reveals a private owner. Measure that same scene,
      // retaining every unrelated collapsed container and leaving rejected edits alone.
      const visible = new Set(collapsed);
      for (let index = 0; index < path.length; index++) visible.delete(viewId(path.slice(0, index), 'node', path[index].node));
      const positions = new Map(projectScene(current, visible).nodes.filter(node => node.data.view === 'node' && sameBlockPath(node.data.path, path)).map(node => [node.data.local, node.position]));
      result = duplicateFragment(current, path, ids, notes, positions);
      return result.document;
    });
    if (!result || editor.getSnapshot().context.document !== result.document) return;
    const first = result.nodes[0] ? { kind: 'node' as const, id: result.nodes[0], path } : result.notes[0] ? { kind: 'note' as const, id: result.notes[0], path } : undefined;
    if (first) setSelection(first);
    return result;
  }, [edit, editor, setSelection, collapsed]);
  const currentState = useCallback(() => editor.getSnapshot().context, [editor]);
  const navigate = useCallback((next: BlockPath) => setSelection({ kind: 'start', path: next }), [setSelection]);
  // Save and Check act on what the author sees: every shown field's valid
  // unapplied text is applied first; text that cannot be applied refuses.
  const runCheck = useCallback((explain = false) => { commits.flush(); editor.send({ type: 'CHECK', explain, context: target.scope.kind === 'user' ? dependencyContext : undefined }); }, [editor, commits, target.scope.kind, dependencyContext]);
  const save = useCallback(() => { commits.flush(); editor.send({ type: 'SAVE' }); }, [editor, commits]);
  // Accepted view changes are delivered in order, including several renames
  // or undo steps in one render batch; no snapshot effect drops an intermediate
  // identity change or replays an old position restoration on remount.
  const [viewEpoch, setViewEpoch] = useState(0);
  useEffect(() => {
    const renamed = editor.on('view.rename', ({ rename }) => {
      if (rename.from.path.length === 0) {
        choose(current => {
          if (!current || !('path' in current)) return current;
          const name = rename.to.path.length ? (rename.to.path.at(-1) as { key: string }).key : rename.to.node!;
          const address = renameAddress({ block: current.path ?? block, node: current.kind === 'node' ? current.id : undefined, path: [], role: 'value' }, rename.from, name);
          return current.kind === 'node' ? { ...current, path: address.block, id: address.node! } : { ...current, path: address.block };
        });
        setViewEpoch(value => value + 1);
      }
    });
    return () => { renamed.unsubscribe(); };
  }, [editor, key, block]);

  // A browser unload may not drop unsaved edits silently.
  useEffect(() => {
    const guard = (event: BeforeUnloadEvent) => { if (editorHolds(editor)) event.preventDefault(); };
    window.addEventListener('beforeunload', guard);
    return () => window.removeEventListener('beforeunload', guard);
  }, [editor]);
  // A selection that no longer exists in the edited block closes.
  const owner = document ? blockAt(document, block) : undefined;
  useEffect(() => {
    if (selection?.kind === 'node' && document) {
      const selectedBlock = blockAt(document, selection.path ?? block);
      if (!selectedBlock || !nodeAt(selectedBlock, selection.id)) choose(undefined);
    }
    if (selection?.kind === 'note' && !document?.editor?.notes?.some(note => note.id === selection.id)) choose(undefined);
  }, [document, block, selection]);

  const located = useMemo(() => {
    const items: { location: DiagnosticLocation; severity: 'error' | 'warning' }[] = [];
    const current = check && check.generation === generation ? check : undefined;
    if (current?.report.structure.status === 'invalid') items.push({ location: locate(current.report.structure.diagnostic.path), severity: 'error' });
    if (current?.report.dependencies.status === 'blocked' && (target.scope.kind !== 'user' || current.context === dependencyContext)) for (const item of current.report.dependencies.diagnostics) items.push({ location: locate(item.path), severity: 'warning' });
    if (notice?.kind === 'failure' && notice.failure.kind === 'invalid') items.push({ location: locate(notice.failure.diagnostic.path), severity: 'error' });
    if (notice?.kind === 'incomplete') for (const item of notice.identities) items.push({ location: { block: item.block, node: item.node, field: item.field }, severity: 'error' });
    return items;
  }, [check, generation, notice, dependencyContext, target.scope.kind]);
  const problems = useMemo(() => problemsIn(located), [located]);
  const go = useCallback((location: DiagnosticLocation) => {
    navigate(location.block);
    setSelection(location.node ? { kind: 'node', id: location.node, path: location.block } : location.edge !== undefined ? { kind: 'edge', index: location.edge, path: location.block } : { kind: 'block', path: location.block });
    setFieldFocus(current => ({ field: location.field, epoch: (current?.epoch ?? 0) + 1 }));
  }, [navigate]);
  /** Expand the owning view and disclosure, then focus its exact draft field. */
  const show = useCallback((address: FieldAddress) => {
    const current = editor.getSnapshot().context.document;
    const container = current && blockAt(current, address.block);
    const node = container && address.node !== undefined ? nodeAt(container, address.node) : undefined;
    navigate(address.block);
    setSelection(address.domain?.kind === 'note' ? { kind: 'note', id: address.domain.id, path: address.block } : node ? { kind: 'node', id: address.node!, path: address.block } : { kind: 'block', path: address.block });
    setFieldFocus(current => ({ address, epoch: (current?.epoch ?? 0) + 1 }));
  }, [editor, navigate]);

  const blockLabel = block.length ? block.map(step => `${step.node} / body`).join(' › ') : tx('workflows:canvas.root');
  const fileState = !base?.source.present ? tx('workflows:editor.file-state-missing') : dirty ? tx('workflows:editor.file-state-unsaved') : tx('workflows:editor.file-state-saved');
  const workspaces = catalog?.workspaces ?? [];

  // Editor shortcuts apply while the editor is shown, wherever focus rests
  // outside text fields and other dialogs.
  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      const mod = event.metaKey || event.ctrlKey;
      if (!mod || event.isComposing || (event.target as Element | null)?.closest?.('[role="dialog"], [role="alertdialog"]')) return;
      const key = event.key.toLowerCase();
      if (key === 's') { event.preventDefault(); save(); return; }
      if (keyboardOwned(event.target)) return;
      if (key === 'z') { event.preventDefault(); editor.send({ type: event.shiftKey ? 'REDO' : 'UNDO' }); }
      else if (key === 'y') { event.preventDefault(); editor.send({ type: 'REDO' }); }
    };
    window.document.addEventListener('keydown', onKeyDown);
    return () => window.document.removeEventListener('keydown', onKeyDown);
  }, [editor, save]);

  let content: ReactNode;
  if (snapshot.matches('loading')) content = <p className={css.state} role="status">{tx('workflows:editor.loading')}</p>;
  else if (snapshot.matches('failed')) content = <div className={css.state} role="alert">
    <p>{tx('workflows:editor.read-failed', { p0: context.loadFailure?.kind === 'failed' ? context.loadFailure.message : context.loadFailure?.kind ?? '' })}</p>
    <Button variant="outline" size="sm" onClick={() => editor.send({ type: 'RELOAD' })}>{tx('workflows:library.retry')}</Button>
  </div>;
  else if (!base?.source.present) content = <div className={css.state} role="alert"><p>{tx('workflows:editor.missing')}</p></div>;
  else if (!document || !owner) content = <div className={css.state} role="alert">
    <p>{tx('workflows:editor.not-editable')}</p>
    {base.source.diagnostic && <ul className={css.problems}><li className={css.problem}><code>{base.source.diagnostic.path}</code><span>{base.source.diagnostic.reason}</span>
      {base.source.diagnostic.line != null && <small>{tx('workflows:check.line', { p0: base.source.diagnostic.line, p1: base.source.diagnostic.column ?? 0 })}</small>}
      {base.source.diagnostic.correction && <small>{base.source.diagnostic.correction}</small>}</li></ul>}
  </div>;
  else content = <div className={css.body}>
    <ReactFlowProvider>
      <div className={css.stage}>
        <nav className={css.crumbs} aria-label={tx('workflows:canvas.block-path')}>
          <IconFolder size={14} />
          <button type="button" className={css.crumb} aria-current={block.length === 0 ? 'location' : undefined} onClick={() => navigate([])}>{tx('workflows:canvas.root')}</button>
          {block.map((step, index) => <span key={index} className={css.crumbStep}><span aria-hidden>/</span>
            <button type="button" className={css.crumb} aria-current={index === block.length - 1 ? 'location' : undefined} onClick={() => navigate(block.slice(0, index + 1))}>{step.node} / {/* i18n-raw: native Loop body field name */ 'body'}</button>
          </span>)}
          {block.length === 0 && <><span aria-hidden>/</span><span className={css.crumbFile}>{target.id}</span></>}
        </nav>
        <Canvas document={document} path={block} viewKey={key} viewEpoch={viewEpoch} selection={selection} onSelect={setSelection} currentState={currentState} organizeEpoch={layoutEpoch} collapsed={collapsed} setCollapsed={setCollapsed}
          problems={problems} edit={edit} editable={editable && !saving} blockLabel={blockLabel} drafts={context.drafts} onDuplicate={duplicate} />
      </div>
    </ReactFlowProvider>
    {/* As Dify: a selected connection only highlights; it opens no panel. */}
    {selection && selection.kind !== 'edge' && <DraftOwner.Provider value={owner$}><Inspector port={context.port} document={document} block={block} selection={selection} onSelect={setSelection} edit={edit} navigate={navigate} collapsed={collapsed} toggleContainer={toggleContainer} focus={fieldFocus} onDuplicate={duplicate}
      vocabulary={base.source.vocabulary} check={check} generation={generation} rejection={context.rejection ? tx(`workflows:rejection.${context.rejection}`) : undefined}
      checks={<CheckResults check={check} generation={generation} global={target.scope.kind === 'user'} workspaces={workspaces} context={dependencyContext} setContext={setDependencyContext} run={() => runCheck()} go={go} />} /></DraftOwner.Provider>}
  </div>;

  return <section className={css.editor} aria-labelledby="workflow-editor-title">
    <header className={css.head}>
      <div className={css.headMain}>
        <div className={css.crumbLine}>
          <button type="button" className={css.iconButton} aria-label={tx('workflows:editor.back')} title={tx('workflows:editor.back')} onClick={() => surface.send({ type: 'BACK' })}><IconArrowLeft /></button>
          <span className={css.muted}>{tx('workflows:library.title')}</span><span aria-hidden className={css.sep}>/</span>
          <span className={css.muted}>{target.scope.kind === 'user' ? tx('workflows:scope.global') : tx('workflows:scope.workspace-value', { p0: scopeLabel })}</span><span aria-hidden className={css.sep}>/</span>
          {document?.editor?.display && <WorkflowIcon icon={document.editor.display.icon} size={16} />}
          <h1 id="workflow-editor-title" className={css.title}>{document?.editor?.display?.title.trim() || target.id}</h1>
        </div>
        <p className={css.fileLine}><IconFile size={14} /><code title={base?.source.path}>{base?.source.relative_path ?? /* i18n-raw: native canonical file location */ `.agents/workflows/${target.id}.yaml`}</code><span aria-hidden className={css.sep}>│</span><span>{fileState}</span>
          {base && <span className={css.revision} title={base.revision}>{shortRevision(base.revision)}</span>}</p>
      </div>
      <div className={css.actions}>
        <span className={css.saved} role="status"><b className={clsx(css.dot, dirty && css.dotDirty)} />{dirty ? tx('workflows:editor.dirty') : tx('workflows:editor.clean')}</span>
        <button type="button" className={css.iconButton} aria-label={tx('workflows:editor.undo')} title={tx('workflows:editor.undo')} disabled={!context.past.length} onClick={() => editor.send({ type: 'UNDO' })}><IconUndo /></button>
        <button type="button" className={css.iconButton} aria-label={tx('workflows:editor.redo')} title={tx('workflows:editor.redo')} disabled={!context.future.length} onClick={() => editor.send({ type: 'REDO' })}><IconRedo /></button>
        <Button variant="outline" size="sm" icon={<IconCheckOutline16 />} disabled={!editable || checking} onClick={() => { runCheck(); setSelection({ kind: 'check' }); }}>{checking ? tx('workflows:editor.checking') : tx('workflows:editor.check')}</Button>
        <Button variant="outline" size="sm" icon={<IconExplain />} disabled={!editable} onClick={() => { runCheck(true); setExplaining(true); }}>{tx('workflows:editor.explain')}</Button>
        <Button variant="primary" size="sm" icon={<IconFile />} disabled={!editable || saving || !dirty || !!context.uncertain} onClick={save}>{saving ? tx('workflows:editor.saving') : tx('workflows:editor.save')}</Button>
        <Menu open={menu} onClose={() => setMenu(false)} align="end" autoFocus onSelect={id => {
          setMenu(false);
          if (id === 'settings') setSelection({ kind: 'block' });
          else if (id === 'layout') setLayoutEpoch(value => value + 1);
          else if (id === 'reload') { if (dirty) setReloading(true); else editor.send({ type: 'RELOAD' }); }
        }} items={[
          { id: 'settings', label: tx('workflows:editor.workflow-settings'), disabled: !editable },
          { id: 'layout', label: tx('workflows:editor.reset-layout') },
          { type: 'separator', id: 'separator' },
          { id: 'reload', label: tx('workflows:editor.reload'), disabled: saving },
        ]} anchor={<button type="button" className={css.moreButton} aria-label={tx('workflows:editor.more')} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(value => !value)}><IconEllipsisOutline16 /></button>} />
      </div>
    </header>
    <EditorNotices notice={notice} uncertain={!!context.uncertain} receipt={!!context.receipt && !notice} reconcile={() => editor.send({ type: 'RECONCILE' })} dismiss={() => editor.send({ type: 'DISMISS' })} discard={() => editor.send({ type: 'DISCARD_DRAFTS' })}
      drafts={context.drafts} show={show} discardField={field => editor.send({ type: 'DISCARD_DRAFT', field })} />
    {content}
    <Modal open={reloading} onClose={() => setReloading(false)} title={tx('workflows:editor.reload-title')} closeLabel={tx('workflows:dialog.close')} description={tx('workflows:editor.reload-body')}
      footer={<><Button variant="outline" onClick={() => setReloading(false)}>{tx('workflows:dialog.cancel')}</Button><Button variant="primary" onClick={() => { setReloading(false); editor.send({ type: 'RELOAD' }); }}>{tx('workflows:editor.reload-confirm')}</Button></>} />
    <ExplainDialog open={explaining} close={() => setExplaining(false)} editor={editor} />
  </section>;
}

function EditorNotices({ notice, uncertain, receipt, reconcile, dismiss, discard, drafts, show, discardField }: {
  notice?: EditorNotice; uncertain: boolean; receipt: boolean; reconcile: () => void; dismiss: () => void; discard: () => void;
  drafts: Readonly<Record<string, FieldDraft>>; show: (address: FieldAddress) => void; discardField: (field: string) => void;
}) {
  const tx = useTranslation();
  if (!notice && !uncertain) return receipt ? <p className={css.noticeQuiet} role="status">{tx('workflows:notice.saved')}</p> : null;
  let text: ReactNode = null;
  if (notice?.kind === 'failure') text = <FailureText failure={notice.failure} operation={tx(`workflows:notice.operation-${notice.operation}`)} />;
  else if (notice?.kind === 'committed_unconfirmed') text = tx('workflows:notice.committed-unconfirmed');
  else if (notice?.kind === 'changed_after_save') text = tx('workflows:notice.changed-after-save', { p0: shortRevision(notice.revision) });
  else if (notice?.kind === 'not_committed') text = tx('workflows:notice.not-committed');
  else if (notice?.kind === 'reconciled') text = tx('workflows:notice.reconciled');
  else if (notice?.kind === 'changed_externally') text = tx('workflows:notice.changed-externally', { p0: shortRevision(notice.revision) });
  else if (notice?.kind === 'incomplete') text = tx('workflows:notice.incomplete', { p0: notice.identities.map(item => [...item.block.map(step => step.node), item.node].join(' / ')).join(', ') });
  else if (notice?.kind === 'pending_fields') text = tx('workflows:notice.pending-fields', { p0: Object.keys(drafts).length });
  else if (uncertain) text = tx('workflows:notice.uncertain');
  const serious = notice?.kind === 'failure' || uncertain || notice?.kind === 'changed_externally' || notice?.kind === 'changed_after_save' || notice?.kind === 'incomplete' || notice?.kind === 'pending_fields';
  return <div className={clsx(css.notice, serious && css.noticeSerious)} role={serious ? 'alert' : 'status'}>
    <span>{text}</span>
    {uncertain && <Button size="sm" variant="outline" onClick={reconcile}>{tx('workflows:notice.reconcile')}</Button>}
    {notice?.kind === 'pending_fields' && <Button size="sm" variant="outline" onClick={discard}>{tx('workflows:notice.discard-fields')}</Button>}
    {notice && <Button size="sm" onClick={dismiss}>{tx('workflows:notice.dismiss')}</Button>}
    {notice?.kind === 'pending_fields' && <ul className={css.pendingFields} aria-label={tx('workflows:notice.pending-list')}>
      {Object.entries(drafts).map(([field, draft]) => <li key={field}>
        <code>{describeField(draft.address)}</code>
        {draft.conflict === 'missing' ? <DetachedText draft={draft} discard={() => discardField(field)} /> : <>
          <Button size="sm" variant="outline" onClick={() => show(draft.address)}>{tx('workflows:notice.show-field')}</Button>
          <Button size="sm" variant="outline" onClick={() => discardField(field)}>{tx('workflows:notice.discard-field')}</Button>
        </>}
      </li>)}
    </ul>}
  </div>;
}

/** Text kept from a field that no longer exists. No field shows it, so
 * the author reads it here, exactly as typed, and copies what they need
 * before discarding it. Showing or copying it changes nothing. */
function DetachedText({ draft, discard }: { draft: FieldDraft; discard: () => void }) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const [copy, setCopy] = useState<'copied' | 'failed'>();
  const id = useId();
  const location = describeField(draft.address);
  return <>
    <small>{tx('workflows:notice.field-gone')}</small>
    <Button size="sm" variant="outline" aria-expanded={open} aria-controls={id} onClick={() => setOpen(value => !value)}>{tx(open ? 'workflows:notice.hide-text' : 'workflows:notice.view-text')}</Button>
    <Button size="sm" variant="outline" onClick={discard}>{tx('workflows:notice.discard-field')}</Button>
    {open && <div id={id} className={css.keptText}>
      <textarea className={clsx(css.input, css.textarea, css.mono)} readOnly value={draft.text} aria-label={tx('workflows:notice.kept-text', { p0: location })} spellCheck={false} rows={Math.min(8, draft.text.split('\n').length + 1)} />
      <Button size="sm" variant="outline" onClick={() => { setCopy(undefined); void writeClipboard(draft.text).then(ok => setCopy(ok ? 'copied' : 'failed')); }}>{tx('workflows:notice.copy-text')}</Button>
      <span role="status">{copy && tx(copy === 'copied' ? 'workflows:notice.text-copied' : 'workflows:notice.copy-failed')}</span>
    </div>}
  </>;
}

/** The redacted static explanation of the exact candidate last checked. */
function ExplainDialog({ open, close, editor }: { open: boolean; close: () => void; editor: WorkflowEditorActor }) {
  const tx = useTranslation();
  const { check, generation, checking } = useSelector(editor, snapshot => ({ check: snapshot.context.check, generation: snapshot.context.generation, checking: snapshot.matches({ ready: 'checking' }) }),
    (a, b) => a.check === b.check && a.generation === b.generation && a.checking === b.checking);
  const report = check && check.explain && check.generation === generation ? check.report : undefined;
  const explanation = report?.explanation;
  const list = (values: readonly string[]) => values.length ? values.join(', ') : tx('workflows:explain.none');
  return <Modal open={open} onClose={close} title={tx('workflows:explain.title')} closeLabel={tx('workflows:dialog.close')} description={tx('workflows:explain.body')} className={css.explain}>
    {checking && <p role="status">{tx('workflows:editor.checking')}</p>}
    {!checking && report?.structure.status === 'invalid' && <p role="alert">{tx('workflows:explain.invalid', { p0: `${report.structure.diagnostic.path}: ${report.structure.diagnostic.reason}` })}</p>}
    {!checking && explanation && <dl className={css.facts}>
      <dt>{tx('workflows:explain.timeout')}</dt><dd>{tx('workflows:explain.milliseconds', { p0: explanation.configured_timeout_ms })}</dd>
      <dt>{tx('workflows:explain.steps')}</dt><dd>{explanation.conservative_steps}</dd>
      <dt>{tx('workflows:explain.agent-runs')}</dt><dd>{explanation.conservative_agent_runs}</dd>
      <dt>{tx('workflows:explain.model-calls')}</dt><dd>{explanation.conservative_model_calls}</dd>
      <dt>{tx('workflows:explain.retained')}</dt><dd>{explanation.conservative_retained_bytes}</dd>
      <dt>{tx('workflows:explain.profiles')}</dt><dd>{list(explanation.profiles)}</dd>
      <dt>{tx('workflows:explain.tools')}</dt><dd>{list(explanation.tools.map(tool => tool.origin === 'builtin' ? tool.name : `${tool.source_id}/${tool.name}`))}</dd>
      <dt>{tx('workflows:explain.blocks')}</dt><dd><ul className={css.explainBlocks}>{Object.entries(explanation.blocks).map(([path, block]) => <li key={path}><code>{path}</code> · {Object.entries(block.nodes).map(([id, node]) => `${id}:${node.kind}`).join(', ')}</li>)}</ul></dd>
    </dl>}
  </Modal>;
}
