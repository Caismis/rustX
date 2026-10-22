import { useEffect, useState } from 'react';
import { useSelector } from '@xstate/react';
import { Button } from '../../../../presentation/primitives/Button';
import { Modal } from '../../../../presentation/primitives/Modal';
import { useTranslation } from '../../../../locale/react';
import type { WorkflowFileSummary, WorkflowLibrary } from '../../../../../../protocol/app-server/v51';
import type { WorkflowScopePort } from '../../port';
import type { LibraryOperation, WorkflowLibraryActor } from '../../machines/library';
import { FailureText } from '../notices';
import css from '../Workflows.module.css';

/** The native identity rule, checked early for presentation; native decides. */
const ID = /^[a-z][a-z0-9_-]{0,63}$/;
export function idProblem(id: string, taken: ReadonlySet<string>) {
  if (!ID.test(id)) return 'workflows:dialog.id-invalid' as const;
  if (id === 'workflow_output') return 'workflows:dialog.id-reserved' as const;
  if (taken.has(id)) return 'workflows:dialog.id-taken' as const;
  return undefined;
}

/** The exact scope a confirmation was opened for: its port, the listing it
 * shows and the scope's label. */
export interface BoundTarget { readonly port: WorkflowScopePort; readonly listing: WorkflowLibrary; readonly scopeLabel: string }

export type LibraryDialog =
  | { kind: 'create'; bound: BoundTarget }
  | { kind: 'copy'; file: WorkflowFileSummary; bound: BoundTarget }
  | { kind: 'delete'; file: WorkflowFileSummary; bound: BoundTarget };

/** One confirmation surface for one file operation. It shows the exact
 * scope and target before anything is sent, and stays open until native
 * settles the operation it sent. */
export function LibraryDialogs({ dialog, close, library }: {
  dialog: LibraryDialog | undefined; close: () => void; library: WorkflowLibraryActor;
}) {
  const tx = useTranslation();
  const state = useSelector(library, snapshot => ({
    operating: snapshot.matches('operating'), pending: snapshot.context.pending,
    failure: snapshot.context.operationFailure,
  }), (a, b) => a.operating === b.operating && a.pending === b.pending && a.failure === b.failure);
  const [id, setId] = useState('');
  const [description, setDescription] = useState('');
  const [sent, setSent] = useState<LibraryOperation>();
  const [touched, setTouched] = useState(false);
  const failure = state.failure && state.failure.operation === sent ? state.failure.failure : undefined;
  const busy = !!sent && (state.operating || state.pending === sent);
  // The sent operation settled without a failure: the dialog's job is done.
  const settled = !!sent && !busy && !failure;
  useEffect(() => { if (settled) close(); }, [settled, close]);
  if (!dialog) return null;
  const { port, listing, scopeLabel } = dialog.bound;
  const directory = listing.directory;
  const taken = new Set(listing.files.flatMap(file => file.id ? [file.id] : []));
  const submit = (operation: LibraryOperation) => { setSent(operation); library.send({ type: 'OPERATE', operation, port, absent: listing.absent_revision }); };
  const dismiss = () => { if (!busy) close(); };
  const problem = dialog.kind === 'delete' ? undefined : idProblem(id, taken);
  const relative = (name: string) => `.agents/workflows/${name}.yaml`;
  const facts = (target: string) => <dl className={css.facts}>
    <dt>{tx('workflows:dialog.scope')}</dt><dd>{scopeLabel}</dd>
    <dt>{tx('workflows:dialog.target')}</dt><dd><code>{target}</code></dd>
    <dt>{tx('workflows:dialog.directory')}</dt><dd><code>{directory}</code></dd>
  </dl>;
  const idField = <label className={css.field}>
    <span>{tx('workflows:dialog.id')}</span>
    <input value={id} autoFocus spellCheck={false} aria-invalid={touched && !!problem} aria-describedby="workflow-id-hint"
      onChange={event => { setId(event.target.value.trim()); setTouched(true); }} disabled={busy} />
    <small id="workflow-id-hint" className={touched && problem ? css.error : undefined}>{touched && problem ? tx(problem) : tx('workflows:dialog.id-hint')}</small>
  </label>;
  const error = failure && <p className={css.error} role="alert"><FailureText failure={failure} /></p>;
  if (dialog.kind === 'create') {
    const ready = !problem && description.trim() !== '';
    return <Modal open onClose={dismiss} title={tx('workflows:dialog.new-title')} closeLabel={tx('workflows:dialog.close')} description={tx('workflows:dialog.new-body')} className={css.dialog}
      footer={<><Button variant="outline" onClick={dismiss} disabled={busy}>{tx('workflows:dialog.cancel')}</Button>
        <Button variant="primary" disabled={!ready || busy} onClick={() => submit({ kind: 'create', id, description: description.trim() })}>{busy ? tx('workflows:dialog.creating') : tx('workflows:dialog.create')}</Button></>}>
      <form className={css.form} onSubmit={event => { event.preventDefault(); if (ready && !busy) submit({ kind: 'create', id, description: description.trim() }); }}>
        {idField}
        <label className={css.field}>
          <span>{tx('workflows:dialog.description')}</span>
          <textarea rows={2} value={description} onChange={event => setDescription(event.target.value)} disabled={busy} />
          <small>{touched && !description.trim() ? tx('workflows:dialog.description-required') : tx('workflows:dialog.description-hint')}</small>
        </label>
        {facts(relative(id || '<id>'))}
        {error}
      </form>
    </Modal>;
  }
  if (dialog.kind === 'copy') {
    const { file } = dialog;
    return <Modal open onClose={dismiss} title={tx('workflows:dialog.copy-title')} closeLabel={tx('workflows:dialog.close')} description={tx('workflows:dialog.copy-body', { p0: file.relative_path })} className={css.dialog}
      footer={<><Button variant="outline" onClick={dismiss} disabled={busy}>{tx('workflows:dialog.cancel')}</Button>
        <Button variant="primary" disabled={!!problem || busy} onClick={() => submit({ kind: 'copy', source: file.id!, sourceRevision: file.revision, id })}>{busy ? tx('workflows:dialog.copying') : tx('workflows:dialog.copy')}</Button></>}>
      <form className={css.form} onSubmit={event => { event.preventDefault(); if (!problem && !busy) submit({ kind: 'copy', source: file.id!, sourceRevision: file.revision, id }); }}>
        <dl className={css.facts}><dt>{tx('workflows:dialog.copy-source')}</dt><dd><code>{file.relative_path}</code></dd>
          <dt>{tx('workflows:dialog.revision')}</dt><dd><code>{file.revision.slice(0, 19)}</code></dd></dl>
        {idField}
        {facts(relative(id || '<id>'))}
        {error}
      </form>
    </Modal>;
  }
  const { file } = dialog;
  return <Modal open onClose={dismiss} title={tx('workflows:dialog.delete-title')} closeLabel={tx('workflows:dialog.close')} description={tx('workflows:dialog.delete-body')} className={css.dialog}
    footer={<><Button variant="outline" onClick={dismiss} disabled={busy}>{tx('workflows:dialog.cancel')}</Button>
      <Button variant="primary" className={css.danger} disabled={busy} onClick={() => submit({ kind: 'delete', id: file.id!, revision: file.revision })}>{busy ? tx('workflows:dialog.deleting') : tx('workflows:dialog.delete')}</Button></>}>
    <div className={css.form}>
      {facts(file.relative_path)}
      <dl className={css.facts}><dt>{tx('workflows:dialog.revision')}</dt><dd><code>{file.revision.slice(0, 19)}</code></dd></dl>
      {file.shadows_global && <p className={css.warning}>{tx('workflows:dialog.delete-override')}</p>}
      <p className={css.muted}>{tx('workflows:dialog.delete-selection')}</p>
      {error}
    </div>
  </Modal>;
}
