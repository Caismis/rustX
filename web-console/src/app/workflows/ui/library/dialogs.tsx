import { useState } from 'react';
import { useSelector } from '@xstate/react';
import { Button } from '../../../../presentation/primitives/Button';
import { Modal } from '../../../../presentation/primitives/Modal';
import { useTranslation } from '../../../../locale/react';
import type { LibraryConfirmation, WorkflowLibraryActor } from '../../machines/library';
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

/** One confirmation surface for one file operation, opened by the library
 * machine from one accepted observation. It shows that observation's scope
 * and target before anything is sent, submits only the author's values,
 * and stays open until native settles the operation it sent. */
export function LibraryDialog({ confirmation, scopeLabel, library }: {
  confirmation: LibraryConfirmation; scopeLabel: string; library: WorkflowLibraryActor;
}) {
  const tx = useTranslation();
  const busy = useSelector(library, snapshot => snapshot.context.pending?.confirmation === confirmation.seq);
  const blocked = busy || !!confirmation.uncertain;
  const [id, setId] = useState('');
  const [description, setDescription] = useState('');
  const [touched, setTouched] = useState(false);
  const { listing } = confirmation.observation;
  const failure = confirmation.failure;
  const directory = listing.directory;
  const taken = new Set(listing.files.flatMap(file => file.id ? [file.id] : []));
  const submit = () => library.send({ type: 'SUBMIT', seq: confirmation.seq, id, description });
  const dismiss = () => { if (!busy) library.send({ type: 'CANCEL', seq: confirmation.seq }); };
  const problem = confirmation.kind === 'delete' ? undefined : idProblem(id, taken);
  const relative = (name: string) => `.agents/workflows/${name}.yaml`;
  const facts = (target: string) => <dl className={css.facts}>
    <dt>{tx('workflows:dialog.scope')}</dt><dd>{scopeLabel}</dd>
    <dt>{tx('workflows:dialog.target')}</dt><dd><code>{target}</code></dd>
    <dt>{tx('workflows:dialog.directory')}</dt><dd><code>{directory}</code></dd>
  </dl>;
  const idField = <label className={css.field}>
    <span>{tx('workflows:dialog.id')}</span>
    <input value={id} autoFocus spellCheck={false} aria-invalid={touched && !!problem} aria-describedby="workflow-id-hint"
      onChange={event => { setId(event.target.value.trim()); setTouched(true); }} disabled={blocked} />
    <small id="workflow-id-hint" className={touched && problem ? css.error : undefined}>{touched && problem ? tx(problem) : tx('workflows:dialog.id-hint')}</small>
  </label>;
  const error = failure && <p className={css.error} role="alert">{confirmation.uncertain
    ? tx('workflows:library.operation-uncertain', { p0: tx(`workflows:library.${confirmation.kind === 'create' ? 'new' : confirmation.kind}`) })
    : <FailureText failure={failure} />}</p>;
  if (confirmation.kind === 'create') {
    const ready = !problem && description.trim() !== '';
    return <Modal open onClose={dismiss} title={tx('workflows:dialog.new-title')} closeLabel={tx('workflows:dialog.close')} description={tx('workflows:dialog.new-body')} className={css.dialog}
      footer={<><Button variant="outline" onClick={dismiss} disabled={busy}>{tx('workflows:dialog.cancel')}</Button>
        <Button variant="primary" disabled={!ready || blocked} onClick={submit}>{busy ? tx('workflows:dialog.creating') : tx('workflows:dialog.create')}</Button></>}>
      <form className={css.form} onSubmit={event => { event.preventDefault(); if (ready && !blocked) submit(); }}>
        {idField}
        <label className={css.field}>
          <span>{tx('workflows:dialog.description')}</span>
          <textarea rows={2} value={description} onChange={event => setDescription(event.target.value)} disabled={blocked} />
          <small>{touched && !description.trim() ? tx('workflows:dialog.description-required') : tx('workflows:dialog.description-hint')}</small>
        </label>
        {facts(relative(id || '<id>'))}
        {error}
      </form>
    </Modal>;
  }
  const file = confirmation.file!;
  if (confirmation.kind === 'copy') {
    return <Modal open onClose={dismiss} title={tx('workflows:dialog.copy-title')} closeLabel={tx('workflows:dialog.close')} description={tx('workflows:dialog.copy-body', { p0: file.relative_path })} className={css.dialog}
      footer={<><Button variant="outline" onClick={dismiss} disabled={busy}>{tx('workflows:dialog.cancel')}</Button>
        <Button variant="primary" disabled={!!problem || blocked} onClick={submit}>{busy ? tx('workflows:dialog.copying') : tx('workflows:dialog.copy')}</Button></>}>
      <form className={css.form} onSubmit={event => { event.preventDefault(); if (!problem && !blocked) submit(); }}>
        <dl className={css.facts}><dt>{tx('workflows:dialog.copy-source')}</dt><dd><code>{file.relative_path}</code></dd>
          <dt>{tx('workflows:dialog.revision')}</dt><dd><code>{file.revision.slice(0, 19)}</code></dd></dl>
        {idField}
        {facts(relative(id || '<id>'))}
        {error}
      </form>
    </Modal>;
  }
  return <Modal open onClose={dismiss} title={tx('workflows:dialog.delete-title')} closeLabel={tx('workflows:dialog.close')} description={tx('workflows:dialog.delete-body')} className={css.dialog}
    footer={<><Button variant="outline" onClick={dismiss} disabled={busy}>{tx('workflows:dialog.cancel')}</Button>
      <Button variant="primary" className={css.danger} disabled={blocked} onClick={submit}>{busy ? tx('workflows:dialog.deleting') : tx('workflows:dialog.delete')}</Button></>}>
    <div className={css.form}>
      {facts(file.relative_path)}
      <dl className={css.facts}><dt>{tx('workflows:dialog.revision')}</dt><dd><code>{file.revision.slice(0, 19)}</code></dd></dl>
      {file.shadows_global && <p className={css.warning}>{tx('workflows:dialog.delete-override')}</p>}
      <p className={css.muted}>{tx('workflows:dialog.delete-selection')}</p>
      {error}
    </div>
  </Modal>;
}
