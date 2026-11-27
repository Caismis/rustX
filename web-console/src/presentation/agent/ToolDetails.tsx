/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned ToolDetails.tsx; see PROVENANCE.md. */
import clsx from 'clsx';
import { useTranslation } from '../../locale/react';
import { CodeBlock } from '../markdown/CodeBlock';
import { IconCheckOutline16, IconPlayOutline16 } from '../primitives/icons';

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
 summarySuffix?: string;
 expandedSummary?: string;
 empty?: string;
 caption?: string;
}

/** Recorded entities and receipts. Navigation is supplied by the owning Conversation. */
export function ToolDetails({ model, onOpenAgent }: { model: ToolDetailsModel; onOpenAgent?: (id: string) => void }) {
 const tx = useTranslation();
 return <div className="mt-[4px] mr-0 mb-[4px] ml-[4px] [border:0.5px_solid_var(--dsw-alias-border-l1)] rounded-[var(--dsw-radius-lg)] bg-[var(--dsw-alias-markdown-code-block)] [color:var(--dsw-alias-label-secondary)] [font:var(--dsw-font-xs-13)] overflow-hidden" data-tool-details>
 {model.caption && <div className="px-[14px] pt-[10px] pb-[6px] [color:var(--dsw-alias-label-caption)] text-[12px]">{model.caption}</div>}
 {model.items.length === 0 ? <p className="m-0 px-[14px] py-[10px]">{model.empty}</p> : <ul className="list-none m-0 p-0 max-h-[320px] overflow-y-auto">
 {model.items.map((item, index) => <li key={index} className="m-0 px-[14px] py-[10px] [&+li]:[border-top:0.5px_solid_var(--dsw-alias-border-l2)]" data-deleted={item.status?.value === 'deleted' || undefined}>
 <div className="flex items-baseline gap-[8px] flex-wrap">
 {item.status && <span className="flex-none self-start inline-flex items-center justify-center w-[14px] h-[18px] text-[16px] leading-[18px]" role="img" aria-label={item.status.label}>{item.status.value === 'completed' ? <IconCheckOutline16 size={14}/> : item.status.value === 'in_progress' ? <IconPlayOutline16 size={14}/> : item.status.value === 'deleted' ? '−' : <span className="box-border w-[10px] h-[10px] [border:1px_solid_var(--dsw-alias-label-tertiary)] rounded-[2px]"/>}</span>}
 {item.agentId && onOpenAgent ? <button type="button" className={clsx('min-w-0 whitespace-pre-wrap wrap-anywhere p-0 border-0 bg-transparent [color:inherit] text-start cursor-pointer hover:underline focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-alias-state-business-primary)] focus-visible:outline-offset-[3px]', item.status ? '[flex:1_1_12em]' : 'flex-1', item.status?.value === 'deleted' && '[color:var(--dsw-alias-label-tertiary)] line-through')} onClick={() => onOpenAgent(item.agentId!)}>{item.title}</button> : <span className={clsx('min-w-0 whitespace-pre-wrap wrap-anywhere', item.status ? '[flex:1_1_12em]' : 'flex-1', item.status?.value === 'deleted' && '[color:var(--dsw-alias-label-tertiary)] line-through')}>{item.title}</span>}
 {item.badge && <span className="flex-none inline-flex items-center gap-[5px] [color:var(--dsw-alias-label-caption)] text-[12px] before:content-[''] before:w-[5px] before:h-[5px] before:rounded-full before:bg-current data-[tone=info]:[color:var(--dsw-alias-state-business-primary)] data-[tone=success]:[color:var(--dsw-alias-state-success-primary)] data-[tone=warning]:[color:var(--dsw-alias-state-warn-primary)] data-[tone=error]:[color:var(--dsw-alias-state-error-primary)]" data-tone={item.badge.tone}>{item.badge.label}</span>}
 {item.status && <span className="ml-auto [color:var(--dsw-alias-label-caption)] text-[12px] whitespace-nowrap">{item.status.label}</span>}
 </div>
 {item.subtitle && <div className="mt-[3px] [color:var(--dsw-alias-label-tertiary)] text-[12px] wrap-anywhere">{item.subtitle}</div>}
 {item.description && <p className="my-[6px] mx-0 [color:var(--dsw-alias-label-caption)] whitespace-pre-wrap wrap-anywhere">{item.description}</p>}
 {!!item.fields?.length && <dl className="m-0 mt-[6px]">{item.fields.map(field => <div className="grid grid-cols-[5.5em_minmax(0,1fr)] gap-[12px] [&+div]:mt-[4px] [&_dt]:[color:var(--dsw-alias-label-caption)] [&_dd]:m-0 [&_dd]:whitespace-pre-wrap [&_dd]:wrap-anywhere" key={field.label}><dt>{field.label}</dt><dd>{field.value}</dd></div>)}</dl>}
 {item.code !== undefined && <CodeBlock className="mt-[8px] mr-0 mb-0 ml-0 max-w-full max-h-[240px] overflow-auto text-[13px]" code={item.code} copyLabel={tx('tools:tool-card.copy')} copiedLabel={tx('tools:tool-card.copied')}/>}
 </li>)}
 </ul>}
 </div>;
}
