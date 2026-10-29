import { autoUpdate, flip, offset, shift, useFloating } from '@floating-ui/react-dom';
import { createPortal } from 'react-dom';
import { useContext, useEffect, useId, useMemo, useRef, useState } from 'react';
import { useSelector } from '@xstate/react';
import type { WorkflowAnalysisQuery, WorkflowReferenceChoice } from '../../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../../locale/react';
import { consumerAddress } from '../../../document/analysis';
import { encodeField, setField, type FieldAddress } from '../../../document/fields';
import { nodePresentation } from '../../../document/geometry';
import { blockAt, nodeAt, type WorkflowDocument } from '../../../document/model';
import { DraftOwner } from './drafts';
import { useCandidateAnalysis } from './analysis';
import css from '../Editor.module.css';

/** A bounded native consumer query. Titles decorate the catalog; they never
 * decide availability, compatibility or lexical reference paths. */
export function ReferencePicker({ address, path, edit, initiallyOpen = false, onChoose, placeholder, conditionVariable = false }: {
  address: FieldAddress; path: readonly string[]; edit: (apply: (document: WorkflowDocument) => WorkflowDocument) => void;
  conditionVariable?: boolean; initiallyOpen?: boolean; onChoose?: (choice: WorkflowReferenceChoice) => void; placeholder?: string;
}) {
  const tx = useTranslation(), owner = useContext(DraftOwner)!;
  const document = useSelector(owner.editor, snapshot => snapshot.context.document);
  const [open, setOpen] = useState(initiallyOpen), [search, setSearch] = useState('');
  const popup = useRef<HTMLDivElement>(null);
  const { refs, floatingStyles } = useFloating({ open, placement: 'bottom-start', strategy: 'fixed', whileElementsMounted: autoUpdate, middleware: [offset(6), flip(), shift({ padding: 12 })] });
  const root = useRef<HTMLDivElement>(null), trigger = useRef<HTMLButtonElement>(null), searchInput = useRef<HTMLInputElement>(null), id = useId();
  const field = encodeField(address);
  useEffect(() => { setOpen(initiallyOpen); setSearch(''); }, [field, initiallyOpen]);
  useEffect(() => {
    if (!open) return;
    searchInput.current?.focus();
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node) && !popup.current?.contains(event.target as Node)) setOpen(false); };
    window.document.addEventListener('pointerdown', outside, true);
    return () => window.document.removeEventListener('pointerdown', outside, true);
  }, [open]);
  const titleOf = (choice: WorkflowReferenceChoice) => choice.producer && document ? nodePresentation(document, choice.producer.block, choice.producer.node)?.title || choice.path[0]
    : choice.path[0] === 'args' ? tx('workflows:value.block-inputs') : choice.path[0] === 'result' ? tx('workflows:value.result') : choice.path[0];
  const query = useMemo<WorkflowAnalysisQuery | undefined>(() => {
    const original = document && consumerAddress(document, address);
    const consumer = original && conditionVariable && original.field.kind === 'condition'
      ? { ...original, field: { ...original.field, kind: 'condition_variable' as const } } : original;
    return consumer ? { consumer, reference: path.length ? [...path] : undefined, search, limit: 128 } : undefined;
  }, [document, field, path, search, conditionVariable]);
  const result = useCandidateAnalysis(query, open && !!query), report = result?.report?.analysis;
  const groups = new Map<string, WorkflowReferenceChoice[]>();
  for (const choice of report?.references ?? []) {
    if (conditionVariable && choice.compatibility === 'incompatible') continue;
    const key = choice.path[0], group = groups.get(key) ?? [];
    group.push(choice); groups.set(key, group);
  }
  const close = () => { setOpen(false); trigger.current?.focus(); };
  const resultScope = query?.consumer.field.kind === 'loop_until' || query?.consumer.field.kind === 'loop_carry';
  const producer = path[0], currentTitle = producer === 'args' ? tx('workflows:value.block-inputs') : producer === 'result' && resultScope ? tx('workflows:value.result') : document ? nodePresentation(document, address.block, producer)?.title || producer : producer;
  const present = (content: import('react').ReactNode) => conditionVariable ? createPortal(content, window.document.body) : content;
  return <div ref={root} className={css.referencePicker}>
    <button ref={element => { trigger.current = element; refs.setReference(element); }} data-workflow-field={field} type="button" className={css.referenceTrigger} aria-label={tx('workflows:value.pick')} aria-haspopup="dialog" aria-expanded={open} aria-controls={id} onClick={() => setOpen(value => !value)}>
      <span>{currentTitle || placeholder || tx('workflows:value.pick')}{path.length > 1 ? ` / ${path.slice(1).join(' / ')}` : ''}</span>
    </button>
    {open && present(<div ref={element => { popup.current = element; refs.setFloating(element); }} id={id} className={conditionVariable ? `${css.referencePopup} ${css.branchVariablePopup}` : css.referencePopup} style={conditionVariable ? floatingStyles : undefined} role="dialog" aria-label={tx('workflows:value.pick')} onWheel={event => event.stopPropagation()} onKeyDown={event => {
      if (event.nativeEvent.isComposing) return;
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); }
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        const choices = [...popup.current!.querySelectorAll<HTMLButtonElement>('[data-variable]:not(:disabled)')], active = choices.indexOf(window.document.activeElement as HTMLButtonElement);
        const next = event.key === 'ArrowDown' ? Math.min(active + 1, choices.length - 1) : Math.max(active - 1, 0);
        if (choices[next]) { event.preventDefault(); choices[next].focus(); }
      }
    }}>
      <input ref={searchInput} className={css.input} aria-label={tx('workflows:value.search')} placeholder={tx('workflows:value.search')} value={search} onChange={event => setSearch(event.target.value)} />
      {!result && <p role="status" className={css.note}>{tx('workflows:value.analyzing')}</p>}
      {result?.failed && <p role="alert" className={css.note}>{tx('workflows:value.analysis-failed')}</p>}
      {report?.diagnostic && <p className={css.note}>{report.diagnostic}</p>}
      {!conditionVariable && report?.selected && <p className={css.note}>{tx(`workflows:value.${report.selected.availability}`)} · {tx(`workflows:value.${report.selected.compatibility}`)}</p>}
      {[...groups].map(([producer, choices]) => <fieldset className={css.variableGroup} key={producer}>
        <legend>{titleOf(choices[0])}<code>{producer}</code></legend>
        {choices.map(choice => {
          const owner = choice.producer && document && blockAt(document, choice.producer.block), node = owner && choice.producer && nodeAt(owner, choice.producer.node);
          const label = choice.path.length > 1 ? choice.path.slice(1).join(' / ') : tx(node?.type === 'model' && node.output.type === 'text' ? 'workflows:value.full-text' : 'workflows:value.whole');
          const type = typeof choice.schema?.type === 'string' ? choice.schema.type : tx('workflows:value.unknown-type');
          return <button type="button" data-variable className={css.variableChoice} key={JSON.stringify(choice.path)}
            aria-label={`${choice.path.join(' / ')} · ${tx(`workflows:value.${choice.availability}`)} · ${tx(`workflows:value.${choice.compatibility}`)}`}
            disabled={choice.availability === 'unavailable' || choice.compatibility === 'incompatible'} title={choice.diagnostic ?? undefined}
            onClick={() => { if (onChoose) onChoose(choice); else edit(document => setField(document, address, { type: 'reference', path: choice.path })); close(); }}>
            <span>{label} <code>{type}</code></span>{(!conditionVariable || choice.availability === 'unavailable' || choice.compatibility === 'incompatible') && <small>{tx(`workflows:value.${choice.availability}`)} · {tx(`workflows:value.${choice.compatibility}`)}</small>}
            {choice.diagnostic && <small>{choice.diagnostic}</small>}
          </button>;
        })}
      </fieldset>)}
      {report && !groups.size && <p className={css.note}>{tx('workflows:value.no-matches')}</p>}
      {report?.truncated && <p className={css.note}>{tx('workflows:value.truncated')}</p>}
      <button type="button" className={css.smallButton} onClick={close}>{tx('workflows:value.close-picker')}</button>
    </div>)}
  </div>;
}
