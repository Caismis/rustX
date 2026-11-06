import { useTranslation } from '../../locale/react';
import { MarkdownText } from '../markdown/MarkdownText';
import css from './ReviewContent.module.css';

/** Workflow-authored fields remain readable prose, without exposing protocol envelopes. */
export function ReviewValue({ value }: { value: unknown }) {
  const tx = useTranslation();
  if (typeof value === 'string') return <MarkdownText text={value}/>;
  if (value == null) return <span className={css.empty}>—</span>;
  if (typeof value === 'boolean') return <span>{tx(value ? 'interactions:questionnaire.true' : 'interactions:questionnaire.false')}</span>;
  if (typeof value === 'number') return <span>{value}</span>;
  if (Array.isArray(value)) return <ul className={css.list}>{value.map((item, index) => <li key={index}><ReviewValue value={item}/></li>)}</ul>;
  return <dl className={css.fields}>{Object.entries(value).map(([key, item]) => <div className={css.field} key={key}><dt>{key}</dt><dd><ReviewValue value={item}/></dd></div>)}</dl>;
}
