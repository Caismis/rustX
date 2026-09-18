/* Copyright (c) 2026 DeepSeek. MIT. Derived from pinned Harness; see PROVENANCE.md. */
import { useState, type ReactNode } from 'react';
import { DisclosureRow } from '../primitives/DisclosureRow';
import { StateDot } from '../primitives/StateDot';
import { IconApiOutline14, IconBrowseOutline16, IconEditOutline16, IconSearchOutline16, IconSparkle16 } from '../primitives/icons';
import css from './Tool.module.css';
import tree from './ToolTree.module.css';
export interface ToolCardView {
 id: string; nativeName?: string; identity?: 'call' | 'execution'; title: string; summary: string; state: 'assembled' | 'running' | 'success' | 'failure' | 'cancelled' | 'uncertain' | 'starting' | 'cancelling' | 'publishing_terminal';
 variant: 'generic' | 'bash' | 'read' | 'write' | 'edit' | 'search'; input?: string; output?: string;
 removed?: string; added?: string; artifacts?: ReactNode;
}
const icons = { generic: IconSparkle16, bash: IconApiOutline14, read: IconBrowseOutline16, write: IconEditOutline16, edit: IconEditOutline16, search: IconSearchOutline16 };
/** One dispatch path, one native call. Expansion never changes lifecycle. */
export function ToolCard({ tool, children }: { tool: ToolCardView; children?: ReactNode }) {
 const [open, setOpen] = useState(false);
 const Icon = icons[tool.variant];
 const state = tool.state === 'failure' ? 'error' : tool.state === 'cancelled' || tool.state === 'uncertain' ? 'stopped' : tool.state === 'success' ? 'ok' : tool.state;
 return <div className={tree.callRow} data-tool-call-id={tool.identity !== 'execution' ? tool.id : undefined} data-execution-id={tool.identity === 'execution' ? tool.id : undefined} data-tool-renderer={tool.variant} data-tool-name={tool.nativeName}>
 <div className={css.root} data-state={state} data-variant={tool.variant}>
 <DisclosureRow rowClassName={css.row} leadingClassName={css.leading} titleClassName={css.title} chevronClassName={css.chevron}
 icon={state === 'error' || state === 'stopped' ? <StateDot state={state === 'error' ? 'error' : 'warning'}/> : <Icon size={14}/>}
 title={tool.title} open={open} expandable expandOnRowClick keepContentWhenOpen onToggle={() => setOpen(v => !v)}
 collapsedContent={<><span className={css.sep}/><span className={css.summary}>{tool.summary}</span><small aria-label="Tool status">{tool.state}</small></>}>
 <div className={css.bodyWrap}><div className={css.ioCard}>
 {tool.input && <div className={css.ioSection}><span className={css.ioLabel}>{tool.variant === 'bash' ? '$' : 'IN'}</span><pre className={css.ioText}>{tool.input}</pre></div>}
 {(tool.removed !== undefined || tool.added !== undefined) && <div className="agent-diff" aria-label="Requested changes"><small>Requested changes</small>{tool.removed !== undefined && <pre data-diff="removed">{tool.removed.split('\n').map(line => '- '+line).join('\n')}</pre>}{tool.added !== undefined && <pre data-diff="added">{tool.added.split('\n').map(line => '+ '+line).join('\n')}</pre>}</div>}
 {tool.output && <div className={css.ioSection}><span className={css.ioLabel}>OUT</span><pre className={css.ioText} data-error={state === 'error' || undefined}>{tool.output}</pre></div>}
 {tool.artifacts}
 </div></div></DisclosureRow></div>{children && <div className={tree.subCalls}>{children}</div>}</div>;
}
