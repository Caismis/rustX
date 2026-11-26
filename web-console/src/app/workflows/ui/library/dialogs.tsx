import { useEffect, useState } from 'react';
import { useSelector } from '@xstate/react';
import { Button } from '../../../../presentation/primitives/Button';
import { Modal } from '../../../../presentation/primitives/Modal';
import { useTranslation } from '../../../../locale/react';
import type { WorkflowFileDocument } from '../../../../../../protocol/app-server/v80';
import type { LibraryConfirmation, WorkflowLibraryActor } from '../../machines/library';
import { setDisplay } from '../../document/presentation';
import { classifyFailure, type WorkflowFailure } from '../../port';
import { FailureText } from '../notices';
import { DEFAULT_WORKFLOW_ICON, IconPicker } from '../WorkflowIcon';
import { catalogCss } from '../../../../presentation/catalog/catalogCss';
import { Input as TextInput } from '@/presentation/primitives/Input';
import { Textarea } from '@/components/ui/textarea';

/** The native identity rule, checked early for presentation; native decides. */
const ID = /^[a-z][a-z0-9_-]{0,63}$/;
export function idProblem(id: string, taken: ReadonlySet<string>) {
  if (!ID.test(id)) return 'workflows:dialog.id-invalid' as const;
  if (id === 'workflow_output') return 'workflows:dialog.id-reserved' as const;
  if (taken.has(id)) return 'workflows:dialog.id-taken' as const;
  return undefined;
}

/** An ID suggested from an ASCII display name; empty when none fits. */
export function suggestedId(title: string) {
  const slug = title.toLowerCase().replace(/[^a-z0-9_-]+/g, '-').replace(/^[^a-z]+|-+$/g, '').slice(0, 64).replace(/-+$/, '');
  return ID.test(slug) ? slug : '';
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
  const [touched, setTouched] = useState(false);
  const { listing } = confirmation.observation;
  const directory = listing.directory;
  const taken = new Set(listing.files.flatMap(file => file.id ? [file.id] : []));
  const submit = () => library.send({ type: 'SUBMIT', seq: confirmation.seq, id });
  const dismiss = () => { if (!busy) library.send({ type: 'CANCEL', seq: confirmation.seq }); };
  const problem = confirmation.kind === 'delete' ? undefined : idProblem(id, taken);
  const facts = (target: string) => <Facts scopeLabel={scopeLabel} target={target} directory={directory} />;
  const idField = <label className={"flex flex-col gap-[6px] text-[12px] [color:var(--dsw-alias-label-secondary)] [&_input]:px-[10px] [&_input]:py-[8px] [&_input]:[border:0.5px_solid_var(--dsw-alias-border-l3)] [&_input]:rounded-[var(--dsw-radius-sm)] [&_input]:bg-[var(--dsw-alias-bg-layer-1)] [&_input]:[color:var(--dsw-alias-label-primary)] [&_input]:[font:inherit] [&_input]:text-[13px] [&_input]:[font-family:var(--ds-font-family-code)] [&_textarea]:px-[10px] [&_textarea]:py-[8px] [&_textarea]:[border:0.5px_solid_var(--dsw-alias-border-l3)] [&_textarea]:rounded-[var(--dsw-radius-sm)] [&_textarea]:bg-[var(--dsw-alias-bg-layer-1)] [&_textarea]:[color:var(--dsw-alias-label-primary)] [&_textarea]:[font:inherit] [&_textarea]:text-[13px] [&_textarea]:resize-none [&_textarea]:[field-sizing:content] [&_textarea]:min-h-[calc(3lh+18px)] [&_textarea]:max-h-[calc(8lh+18px)] [&_textarea]:leading-normal [&_::placeholder]:[color:var(--dsw-alias-label-tertiary)] [&_::placeholder]:[font-family:inherit] [&_small]:[color:var(--dsw-alias-label-tertiary)] wf-field"}>
    <span>{tx('workflows:dialog.id')}</span>
    <TextInput value={id} data-modal-autofocus spellCheck={false} placeholder={tx('workflows:dialog.id-placeholder')} aria-invalid={touched && !!problem} aria-describedby="workflow-id-hint"
      onChange={event => { setId(event.target.value.trim()); setTouched(true); }} disabled={blocked} />
    <small id="workflow-id-hint" className={touched && problem ? "[color:var(--dsw-alias-state-error-primary)]!" : undefined}>{touched && problem ? tx(problem) : tx('workflows:dialog.id-hint')}</small>
  </label>;
  const error = <OperationError confirmation={confirmation} />;
  if (confirmation.kind === 'create' || confirmation.kind === 'edit') return <InfoDialog confirmation={confirmation} scopeLabel={scopeLabel} library={library} busy={busy} />;
  const file = confirmation.file!;
  if (confirmation.kind === 'copy') {
    return <Modal open onClose={dismiss} title={tx('workflows:dialog.copy-title')} closeLabel={tx('workflows:dialog.close')} description={tx('workflows:dialog.copy-body', { p0: file.relative_path })} className={"w-[min(480px,100%)]"}
      footer={<><Button variant="outline" onClick={dismiss} disabled={busy}>{tx('workflows:dialog.cancel')}</Button>
        <Button variant="primary" disabled={!!problem || blocked} onClick={submit}>{busy ? tx('workflows:dialog.copying') : tx('workflows:dialog.copy')}</Button></>}>
      <form className={"flex flex-col gap-[14px] px-[24px]"} onSubmit={event => { event.preventDefault(); if (!problem && !blocked) submit(); }}>
        <dl className={"grid grid-cols-[auto_1fr] gap-x-[14px] gap-y-[6px] m-0 text-[12px] [&_dt]:[color:var(--dsw-alias-label-tertiary)] [&_dd]:m-0 [&_dd]:min-w-0 [&_dd]:wrap-anywhere [&_dd]:[color:var(--dsw-alias-label-secondary)] [&_code]:text-[11.5px]"}><dt>{tx('workflows:dialog.copy-source')}</dt><dd><code>{file.relative_path}</code></dd>
          <dt>{tx('workflows:dialog.revision')}</dt><dd><code>{file.revision.slice(0, 19)}</code></dd></dl>
        {idField}
        {facts(relative(id || '<id>'))}
        {error}
      </form>
    </Modal>;
  }
  return <Modal open onClose={dismiss} title={tx('workflows:dialog.delete-title')} closeLabel={tx('workflows:dialog.close')} description={tx('workflows:dialog.delete-body')} className={"w-[min(480px,100%)]"}
    footer={<><Button variant="outline" onClick={dismiss} disabled={busy}>{tx('workflows:dialog.cancel')}</Button>
      <Button variant="primary" className={"bg-[var(--dsw-alias-state-error-primary)]! [color:var(--dsw-alias-label-primary-inverted)]!"} disabled={blocked} onClick={submit}>{busy ? tx('workflows:dialog.deleting') : tx('workflows:dialog.delete')}</Button></>}>
    <div className={"flex flex-col gap-[14px] px-[24px]"}>
      {facts(file.relative_path)}
      <dl className={"grid grid-cols-[auto_1fr] gap-x-[14px] gap-y-[6px] m-0 text-[12px] [&_dt]:[color:var(--dsw-alias-label-tertiary)] [&_dd]:m-0 [&_dd]:min-w-0 [&_dd]:wrap-anywhere [&_dd]:[color:var(--dsw-alias-label-secondary)] [&_code]:text-[11.5px]"}><dt>{tx('workflows:dialog.revision')}</dt><dd><code>{file.revision.slice(0, 19)}</code></dd></dl>
      {file.shadows_global && <p className={"m-0 text-[12px] [color:var(--dsw-alias-state-warn-primary)]"}>{tx('workflows:dialog.delete-override')}</p>}
      <p className={catalogCss.muted}>{tx('workflows:dialog.delete-selection')}</p>
      {error}
    </div>
  </Modal>;
}

const relative = (name: string) => `.agents/workflows/${name}.yaml`;

function Facts({ scopeLabel, target, directory }: { scopeLabel: string; target: string; directory: string }) {
  const tx = useTranslation();
  return <dl className={"grid grid-cols-[auto_1fr] gap-x-[14px] gap-y-[6px] m-0 text-[12px] [&_dt]:[color:var(--dsw-alias-label-tertiary)] [&_dd]:m-0 [&_dd]:min-w-0 [&_dd]:wrap-anywhere [&_dd]:[color:var(--dsw-alias-label-secondary)] [&_code]:text-[11.5px]"}>
    <dt>{tx('workflows:dialog.scope')}</dt><dd>{scopeLabel}</dd>
    <dt>{tx('workflows:dialog.target')}</dt><dd><code>{target}</code></dd>
    <dt>{tx('workflows:dialog.directory')}</dt><dd><code>{directory}</code></dd>
  </dl>;
}

function OperationError({ confirmation }: { confirmation: LibraryConfirmation }) {
  const tx = useTranslation(), failure = confirmation.failure;
  return failure ? <p className={"[color:var(--dsw-alias-state-error-primary)]!"} role="alert">{confirmation.uncertain
    ? tx('workflows:library.operation-uncertain', { p0: tx(`workflows:library.${confirmation.kind === 'create' ? 'new' : confirmation.kind}`) })
    : <FailureText failure={failure} />}</p> : null;
}

/** The people- and model-facing info of a Workflow: creating one, or
 * editing an existing file's name, description and icon. An edit first reads
 * the whole document (the listing bounds descriptions), and replaces it
 * against the revision it read; its ID and file never change. */
function InfoDialog({ confirmation, scopeLabel, library, busy }: { confirmation: LibraryConfirmation; scopeLabel: string; library: WorkflowLibraryActor; busy: boolean }) {
  const tx = useTranslation();
  const editing = confirmation.kind === 'edit', file = confirmation.file;
  const [source, setSource] = useState<{ revision: string; document: WorkflowFileDocument } | { failure: WorkflowFailure }>();
  useEffect(() => {
    if (!editing || !file?.id) return;
    let live = true;
    confirmation.observation.port.read(file.id).then(
      read => { if (live) setSource(read.document ? { revision: read.revision, document: read.document } : { failure: { kind: 'failed', message: read.diagnostic?.reason ?? 'missing' } }); },
      error => { if (live) setSource({ failure: classifyFailure(error, false) }); });
    return () => { live = false; };
  }, [editing, file?.id, confirmation.observation.port]);
  const dismiss = () => { if (!busy) library.send({ type: 'CANCEL', seq: confirmation.seq }); };
  const title = tx(editing ? 'workflows:dialog.edit-title' : 'workflows:dialog.new-title');
  if (editing && (!source || 'failure' in source)) return <Modal open onClose={dismiss} title={title} closeLabel={tx('workflows:dialog.close')} className={"w-[min(800px,100%)]"}
    footer={<Button variant="outline" onClick={dismiss}>{tx('workflows:dialog.cancel')}</Button>}>
    {source ? <p className={"[color:var(--dsw-alias-state-error-primary)]!"} role="alert"><FailureText failure={source.failure} /></p> : <p className={catalogCss.muted} role="status">{tx('workflows:dialog.loading')}</p>}
  </Modal>;
  return <InfoForm confirmation={confirmation} scopeLabel={scopeLabel} library={library} busy={busy} title={title} source={source && !('failure' in source) ? source : undefined} />;
}

function InfoForm({ confirmation, scopeLabel, library, busy, title: heading, source }: {
  confirmation: LibraryConfirmation; scopeLabel: string; library: WorkflowLibraryActor; busy: boolean; title: string;
  source?: { revision: string; document: WorkflowFileDocument };
}) {
  const tx = useTranslation();
  const blocked = busy || !!confirmation.uncertain;
  const shown = source?.document.editor?.display;
  const [id, setId] = useState(confirmation.file?.id ?? '');
  const [description, setDescription] = useState(source?.document.description ?? '');
  const [title, setTitle] = useState(shown?.title ?? '');
  const [summary, setSummary] = useState(shown?.description ?? '');
  const [icon, setIcon] = useState(shown?.icon ?? DEFAULT_WORKFLOW_ICON);
  const [touched, setTouched] = useState(false);
  const [idEdited, setIdEdited] = useState(false);
  const { listing } = confirmation.observation;
  const taken = new Set(listing.files.flatMap(file => file.id ? [file.id] : []));
  const display = { title: title.trim(), icon, ...(summary.trim() ? { description: summary.trim() } : {}) };
  const problem = source ? undefined : idProblem(id, taken);
  const ready = !problem && description.trim() !== '' && title.trim() !== '';
  const submit = () => library.send(source
    ? { type: 'SUBMIT', seq: confirmation.seq, revision: source.revision, document: setDisplay({ ...source.document, description: description.trim() }, display) }
    : { type: 'SUBMIT', seq: confirmation.seq, id, description, display });
  const dismiss = () => { if (!busy) library.send({ type: 'CANCEL', seq: confirmation.seq }); };
  const action = source ? (busy ? 'workflows:dialog.saving' : 'workflows:dialog.save') : (busy ? 'workflows:dialog.creating' : 'workflows:dialog.create');
  return <Modal open onClose={dismiss} title={heading} closeLabel={tx('workflows:dialog.close')} description={tx(source ? 'workflows:dialog.edit-body' : 'workflows:dialog.new-body')} className={"w-[min(800px,100%)]"}
    footer={<><Button variant="outline" onClick={dismiss} disabled={busy}>{tx('workflows:dialog.cancel')}</Button>
      <Button variant="primary" disabled={!ready || blocked} onClick={submit}>{tx(action)}</Button></>}>
    <form className={"flex flex-col gap-[14px] px-[24px]"} onSubmit={event => { event.preventDefault(); if (ready && !blocked) submit(); }}>
      <div className={"grid grid-cols-2 gap-[28px] max-[720px]:grid-cols-1 max-[720px]:gap-[20px] [&>*]:min-w-0"}>
        <section className={"flex flex-col gap-[14px] min-w-0"}>
          <p className={"mt-0 mb-[-4px] pb-[8px] [border-bottom:0.5px_solid_var(--dsw-alias-border-l2,var(--dsw-alias-border-l3))] text-[12px] font-medium [color:var(--dsw-alias-label-secondary)]"}>{tx('workflows:dialog.for-people')}</p>
          <div className={"flex gap-[12px] items-start [&_.wf-field]:flex-1 [&_.wf-field]:min-w-0"}>
            <IconPicker value={icon} onChange={setIcon} disabled={blocked} className={"mt-[19px]"} />
            <label className={"flex flex-col gap-[6px] text-[12px] [color:var(--dsw-alias-label-secondary)] [&_input]:px-[10px] [&_input]:py-[8px] [&_input]:[border:0.5px_solid_var(--dsw-alias-border-l3)] [&_input]:rounded-[var(--dsw-radius-sm)] [&_input]:bg-[var(--dsw-alias-bg-layer-1)] [&_input]:[color:var(--dsw-alias-label-primary)] [&_input]:[font:inherit] [&_input]:text-[13px] [&_input]:[font-family:var(--ds-font-family-code)] [&_textarea]:px-[10px] [&_textarea]:py-[8px] [&_textarea]:[border:0.5px_solid_var(--dsw-alias-border-l3)] [&_textarea]:rounded-[var(--dsw-radius-sm)] [&_textarea]:bg-[var(--dsw-alias-bg-layer-1)] [&_textarea]:[color:var(--dsw-alias-label-primary)] [&_textarea]:[font:inherit] [&_textarea]:text-[13px] [&_textarea]:resize-none [&_textarea]:[field-sizing:content] [&_textarea]:min-h-[calc(3lh+18px)] [&_textarea]:max-h-[calc(8lh+18px)] [&_textarea]:leading-normal [&_::placeholder]:[color:var(--dsw-alias-label-tertiary)] [&_::placeholder]:[font-family:inherit] [&_small]:[color:var(--dsw-alias-label-tertiary)] wf-field"}>
              <span>{tx('workflows:dialog.title')}</span>
              <TextInput className={"[font-family:inherit]"} value={title} data-modal-autofocus maxLength={128} disabled={blocked} placeholder={tx('workflows:dialog.title-placeholder')}
                onChange={event => { setTitle(event.target.value); setTouched(true); if (!source && !idEdited) setId(suggestedId(event.target.value)); }} />
              <small>{touched && !title.trim() ? tx('workflows:dialog.title-required') : tx('workflows:dialog.title-hint')}</small>
            </label>
          </div>
          <label className={"flex flex-col gap-[6px] text-[12px] [color:var(--dsw-alias-label-secondary)] [&_input]:px-[10px] [&_input]:py-[8px] [&_input]:[border:0.5px_solid_var(--dsw-alias-border-l3)] [&_input]:rounded-[var(--dsw-radius-sm)] [&_input]:bg-[var(--dsw-alias-bg-layer-1)] [&_input]:[color:var(--dsw-alias-label-primary)] [&_input]:[font:inherit] [&_input]:text-[13px] [&_input]:[font-family:var(--ds-font-family-code)] [&_textarea]:px-[10px] [&_textarea]:py-[8px] [&_textarea]:[border:0.5px_solid_var(--dsw-alias-border-l3)] [&_textarea]:rounded-[var(--dsw-radius-sm)] [&_textarea]:bg-[var(--dsw-alias-bg-layer-1)] [&_textarea]:[color:var(--dsw-alias-label-primary)] [&_textarea]:[font:inherit] [&_textarea]:text-[13px] [&_textarea]:resize-none [&_textarea]:[field-sizing:content] [&_textarea]:min-h-[calc(3lh+18px)] [&_textarea]:max-h-[calc(8lh+18px)] [&_textarea]:leading-normal [&_::placeholder]:[color:var(--dsw-alias-label-tertiary)] [&_::placeholder]:[font-family:inherit] [&_small]:[color:var(--dsw-alias-label-tertiary)] wf-field"}>
            <span>{tx('workflows:dialog.summary')}</span>
            <Textarea rows={3} value={summary} maxLength={1024} onChange={event => setSummary(event.target.value)} disabled={blocked} placeholder={tx('workflows:dialog.summary-placeholder')} />
          </label>
        </section>
        <section className={"flex flex-col gap-[14px] min-w-0"}>
          <p className={"mt-0 mb-[-4px] pb-[8px] [border-bottom:0.5px_solid_var(--dsw-alias-border-l2,var(--dsw-alias-border-l3))] text-[12px] font-medium [color:var(--dsw-alias-label-secondary)]"}>{tx('workflows:dialog.for-model')}</p>
          <label className={"flex flex-col gap-[6px] text-[12px] [color:var(--dsw-alias-label-secondary)] [&_input]:px-[10px] [&_input]:py-[8px] [&_input]:[border:0.5px_solid_var(--dsw-alias-border-l3)] [&_input]:rounded-[var(--dsw-radius-sm)] [&_input]:bg-[var(--dsw-alias-bg-layer-1)] [&_input]:[color:var(--dsw-alias-label-primary)] [&_input]:[font:inherit] [&_input]:text-[13px] [&_input]:[font-family:var(--ds-font-family-code)] [&_textarea]:px-[10px] [&_textarea]:py-[8px] [&_textarea]:[border:0.5px_solid_var(--dsw-alias-border-l3)] [&_textarea]:rounded-[var(--dsw-radius-sm)] [&_textarea]:bg-[var(--dsw-alias-bg-layer-1)] [&_textarea]:[color:var(--dsw-alias-label-primary)] [&_textarea]:[font:inherit] [&_textarea]:text-[13px] [&_textarea]:resize-none [&_textarea]:[field-sizing:content] [&_textarea]:min-h-[calc(3lh+18px)] [&_textarea]:max-h-[calc(8lh+18px)] [&_textarea]:leading-normal [&_::placeholder]:[color:var(--dsw-alias-label-tertiary)] [&_::placeholder]:[font-family:inherit] [&_small]:[color:var(--dsw-alias-label-tertiary)] wf-field"}>
            <span>{tx('workflows:dialog.id')}</span>
            <TextInput value={id} spellCheck={false} placeholder={tx('workflows:dialog.id-placeholder')} aria-invalid={touched && !!problem} aria-describedby="workflow-id-hint" readOnly={!!source}
              onChange={event => { setId(event.target.value.trim()); setTouched(true); setIdEdited(true); }} disabled={blocked} />
            <small id="workflow-id-hint" className={touched && problem ? "[color:var(--dsw-alias-state-error-primary)]!" : undefined}>{touched && problem ? tx(problem) : tx(source ? 'workflows:dialog.id-fixed' : 'workflows:dialog.id-hint')}</small>
          </label>
          <label className={"flex flex-col gap-[6px] text-[12px] [color:var(--dsw-alias-label-secondary)] [&_input]:px-[10px] [&_input]:py-[8px] [&_input]:[border:0.5px_solid_var(--dsw-alias-border-l3)] [&_input]:rounded-[var(--dsw-radius-sm)] [&_input]:bg-[var(--dsw-alias-bg-layer-1)] [&_input]:[color:var(--dsw-alias-label-primary)] [&_input]:[font:inherit] [&_input]:text-[13px] [&_input]:[font-family:var(--ds-font-family-code)] [&_textarea]:px-[10px] [&_textarea]:py-[8px] [&_textarea]:[border:0.5px_solid_var(--dsw-alias-border-l3)] [&_textarea]:rounded-[var(--dsw-radius-sm)] [&_textarea]:bg-[var(--dsw-alias-bg-layer-1)] [&_textarea]:[color:var(--dsw-alias-label-primary)] [&_textarea]:[font:inherit] [&_textarea]:text-[13px] [&_textarea]:resize-none [&_textarea]:[field-sizing:content] [&_textarea]:min-h-[calc(3lh+18px)] [&_textarea]:max-h-[calc(8lh+18px)] [&_textarea]:leading-normal [&_::placeholder]:[color:var(--dsw-alias-label-tertiary)] [&_::placeholder]:[font-family:inherit] [&_small]:[color:var(--dsw-alias-label-tertiary)] wf-field"}>
            <span>{tx('workflows:dialog.description')}</span>
            <Textarea rows={3} value={description} placeholder={tx('workflows:dialog.description-placeholder')} onChange={event => { setDescription(event.target.value); setTouched(true); }} disabled={blocked} />
            <small>{touched && !description.trim() ? tx('workflows:dialog.description-required') : tx('workflows:dialog.description-hint')}</small>
          </label>
        </section>
      </div>
      <Facts scopeLabel={scopeLabel} target={confirmation.file?.relative_path ?? relative(id || '<id>')} directory={listing.directory} />
      <OperationError confirmation={confirmation} />
    </form>
  </Modal>;
}
