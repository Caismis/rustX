import { useTranslation } from '../../locale/react';
import { MarkdownText } from '../markdown/MarkdownText';

/** Workflow-authored fields remain readable prose, without exposing protocol envelopes. */
export function ReviewValue({ value }: { value: unknown }) {
  const tx = useTranslation();
  if (typeof value === 'string') return <MarkdownText text={value}/>;
  if (value == null) return <span className="[color:var(--dsw-alias-label-tertiary)]">—</span>;
  if (typeof value === 'boolean') return <span>{tx(value ? 'interactions:questionnaire.true' : 'interactions:questionnaire.false')}</span>;
  if (typeof value === 'number') return <span>{value}</span>;
  if (Array.isArray(value)) return <ul className="pl-[20px]">{value.map((item, index) => <li key={index}><ReviewValue value={item}/></li>)}</ul>;
  return <dl className="my-[8px] mx-0">{Object.entries(value).map(([key, item]) => <div className="my-[8px] mx-0 [&>dt]:[color:var(--dsw-alias-label-tertiary)] [&>dt]:text-[var(--dsh-content-font-size-secondary,13px)] [&>dd]:mt-[4px] [&>dd]:mb-0 [&>dd]:ml-0 [&>dd]:pl-[12px] [&>dd]:border-l [&>dd]:border-l-[var(--dsw-alias-border-l1)]" key={key}><dt>{key}</dt><dd><ReviewValue value={item}/></dd></div>)}</dl>;
}
