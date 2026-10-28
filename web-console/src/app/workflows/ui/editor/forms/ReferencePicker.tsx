import { useContext, useMemo, useState } from 'react';
import { useSelector } from '@xstate/react';
import type { WorkflowAnalysisQuery } from '../../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../../locale/react';
import { consumerAddress } from '../../../document/analysis';
import { setField, type FieldAddress } from '../../../document/fields';
import type { WorkflowDocument } from '../../../document/model';
import { DraftOwner } from './drafts';
import { useCandidateAnalysis } from './analysis';
import css from '../Editor.module.css';

export function ReferencePicker({ address, path, edit }: { address: FieldAddress; path: readonly string[]; edit: (apply: (document: WorkflowDocument) => WorkflowDocument) => void }) {
  const tx = useTranslation(), owner = useContext(DraftOwner)!;
  const document = useSelector(owner.editor, snapshot => snapshot.context.document);
  const [open, setOpen] = useState(false), [search, setSearch] = useState('');
  const query = useMemo<WorkflowAnalysisQuery | undefined>(() => {
    const consumer = document && consumerAddress(document, address);
    return consumer ? { consumer, reference: path.length ? [...path] : undefined, search, limit: 128 } : undefined;
  }, [document, address, path, search]);
  const result = useCandidateAnalysis(query, open && !!query);
  const report = result?.report?.analysis;
  const groups = new Map<string, NonNullable<typeof report>['references']>();
  for (const choice of report?.references ?? []) {
    const key = choice.path[0], group = groups.get(key) ?? [];
    group.push(choice); groups.set(key, group);
  }
  return <div className={css.nested}>
    <button type="button" className={css.smallButton} aria-expanded={open} onClick={() => setOpen(value => !value)}>{tx('workflows:value.pick')}</button>
    {open && <div className={css.nested}>
      <input className={css.input} aria-label={tx('workflows:value.search')} placeholder={tx('workflows:value.search')} value={search} onChange={event => setSearch(event.target.value)} />
      {!result && <p role="status" className={css.note}>{tx('workflows:value.analyzing')}</p>}
      {result?.failed && <p role="alert" className={css.note}>{tx('workflows:value.analysis-failed')}</p>}
      {report?.diagnostic && <p className={css.note}>{report.diagnostic}</p>}
      {report?.selected && <p className={css.note}>{tx(`workflows:value.${report.selected.availability}`)} · {tx(`workflows:value.${report.selected.compatibility}`)}</p>}
      {[...groups].map(([producer, choices]) => <fieldset className={css.value} key={producer}>
        <legend className={css.fieldLabel}>{producer}</legend>
        {choices.map(choice => <button type="button" className={css.smallButton} key={JSON.stringify(choice.path)}
          disabled={choice.availability === 'unavailable' || choice.compatibility === 'incompatible'} title={choice.diagnostic ?? undefined}
          onClick={() => { edit(document => setField(document, address, { type: 'reference', path: choice.path })); setOpen(false); }}>
          {choice.path.join(' / ')} · {tx(`workflows:value.${choice.availability}`)} · {tx(`workflows:value.${choice.compatibility}`)}
        </button>)}
      </fieldset>)}
      {report?.truncated && <p className={css.note}>{tx('workflows:value.truncated')}</p>}
    </div>}
  </div>;
}
