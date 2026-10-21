/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned ToolDetails.tsx; see PROVENANCE.md. */
import { useTranslation } from '../../locale/react';
import { CodeBlock } from '../markdown/CodeBlock';
import { IconCheckOutline16, IconPlayOutline16 } from '../primitives/icons';
import css from './ToolDetails.module.css';

export interface ToolDetailItem {
  title: string;
  subtitle?: string;
  description?: string;
  badge?: { label: string; tone: 'neutral' | 'info' | 'success' | 'warning' | 'error' };
  status?: { value: 'pending' | 'in_progress' | 'completed' | 'deleted'; label: string };
  fields?: readonly { label: string; value: string }[];
  code?: string;
  agentId?: string;
}
export interface ToolDetailsModel {
  items: readonly ToolDetailItem[];
  summary?: string;
  expandedSummary?: string;
  empty?: string;
  caption?: string;
}

/** Recorded entities and receipts. Navigation is supplied by the owning Conversation. */
export function ToolDetails({ model, onOpenAgent }: { model: ToolDetailsModel; onOpenAgent?: (id: string) => void }) {
  const tx = useTranslation();
  return <div className={css.root} data-tool-details>
    {model.caption && <div className={css.caption}>{model.caption}</div>}
    {model.items.length === 0 ? <p className={css.empty}>{model.empty}</p> : <ul className={css.list}>
      {model.items.map((item, index) => <li key={index} className={css.item} data-deleted={item.status?.value === 'deleted' || undefined}>
        <div className={css.heading}>
          {item.status && <span className={css.status} role="img" aria-label={item.status.label}>{item.status.value === 'completed' ? <IconCheckOutline16 size={14}/> : item.status.value === 'in_progress' ? <IconPlayOutline16 size={14}/> : item.status.value === 'deleted' ? '−' : <span className={css.pending}/>}</span>}
          {item.agentId && onOpenAgent ? <button type="button" className={css.link} onClick={() => onOpenAgent(item.agentId!)}>{item.title}</button> : <span className={css.text}>{item.title}</span>}
          {item.badge && <span className={css.badge} data-tone={item.badge.tone}>{item.badge.label}</span>}
          {item.status && <span className={css.statusText}>{item.status.label}</span>}
        </div>
        {item.subtitle && <div className={css.subtitle}>{item.subtitle}</div>}
        {item.description && <p className={css.description}>{item.description}</p>}
        {!!item.fields?.length && <dl className={css.fields}>{item.fields.map(field => <div className={css.field} key={field.label}><dt>{field.label}</dt><dd>{field.value}</dd></div>)}</dl>}
        {item.code !== undefined && <CodeBlock className={css.code} code={item.code} copyLabel={tx('tools:tool-card.copy')} copiedLabel={tx('tools:tool-card.copied')}/>}
      </li>)}
    </ul>}
  </div>;
}
