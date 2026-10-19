import type { JobResultItem } from '../../bindings/jobs';
import { jobStateLabel } from '../../bindings/status-labels';
import { useTranslation } from '../../locale/react';
import { StateDot } from '../../presentation/primitives/StateDot';
import { jobDot } from './job-state';
import css from './JobResultDetails.module.css';

/** Immutable recorded results, never patched from the current Jobs roster. */
export function JobResultDetails({ items }: { items: readonly JobResultItem[] }) {
  const tx = useTranslation();
  return <ul className={css.list}>{items.length ? items.map(item => <li key={item.jobId} className={css.item} data-recorded-job-id={item.jobId}>
    <div className={css.heading}><span>{item.tool}</span><span className={css.status}><StateDot state={jobDot(item.state)} size={7}/>{jobStateLabel(tx, item.state)}</span></div>
    <small className={css.identity}>{tx('common:activity.job-title', { id: item.jobId })}</small>
    {item.output && <pre className={css.output}>{item.output}</pre>}
  </li>) : <li className={css.item}>{tx('common:activity.no-jobs')}</li>}</ul>;
}
