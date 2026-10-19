import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Derived from pinned Harness; see PROVENANCE.md. */
import { useState, type ReactNode } from 'react';
import { DisclosureRow } from '../primitives/DisclosureRow';
import { IconInspectOutline12, IconApiOutline14, IconBrowseOutline16, IconEditOutline16, IconSearchOutline16, IconSparkle16 } from '../primitives/icons';
import css from './Tool.module.css';
import tree from './ToolTree.module.css';
import { TerminalBlock } from '../primitives/TerminalBlock';
import { DiffBlock, type DiffHunk } from '../primitives/DiffBlock';
import { ReadBlock } from '../primitives/ReadBlock';
import { SearchTextBlock } from '../primitives/SearchBlock';
import { ToolDetails, type ToolDetailsModel } from './ToolDetails';


export interface ToolCardView {
 id?: string; nativeName?: string; identity?: 'call' | 'job'; title: string; summary: string; state: 'assembled' | 'running' | 'success' | 'failure' | 'cancelled' | 'uncertain' | 'starting' | 'cancelling' | 'publishing_terminal';
 variant: 'generic' | 'bash' | 'image' | 'read' | 'write' | 'edit' | 'search'; input?: string; output?: string;
 artifacts?: ReactNode; path?: string; exitCode?: number | null; truncated?: boolean; diffs?: DiffHunk[];
 background?: boolean; jobId?: string;
 details?: ToolDetailsModel; icon?: ReactNode;
}
const icons = { image: IconBrowseOutline16, generic: IconSparkle16, bash: IconApiOutline14, read: IconBrowseOutline16, write: IconEditOutline16, edit: IconEditOutline16, search: IconSearchOutline16 };
/** One dispatch path, one native call. Expansion never changes lifecycle. */
export function ToolCard({ tool, children, inspect, onOpenAgent }: { tool: ToolCardView; children?: ReactNode; inspect?: () => Promise<void>; onOpenAgent?: (id: string) => void }) {
  const tx = useTranslation();
  const labels = { copy: tx('tools:tool-card.copy'), copied: tx('tools:tool-card.copied'), collapseAria: tx('tools:tool-card.collapse-output'), collapse: tx('tools:tool-card.collapse'), expandAria: (n: number) => tx('tools:tool-card.show-value-more-lines', { p0: n }), expand: (n: number) => tx('tools:tool-card.show-value-more-lines', { p0: n }) };
 const [open, setOpen] = useState(false);
 const Icon = icons[tool.variant];
 const state = tool.state === 'failure' ? 'error' : tool.state === 'cancelled' || tool.state === 'uncertain' ? 'stopped' : tool.state === 'success' ? 'ok' : tool.state;
 return <div className={tree.callRow} data-tool-call-id={tool.identity !== 'job' ? tool.id : undefined} data-job-id={tool.identity === 'job' ? tool.id : undefined} data-tool-renderer={tool.variant} data-tool-name={tool.nativeName}>
 <div className={css.root} data-state={state} data-variant={tool.variant}>
 <DisclosureRow rowClassName={css.row} leadingClassName={css.leading} titleClassName={css.title} chevronClassName={css.chevron}
 icon={tool.icon ?? <Icon size={14}/>}
 title={tool.title} open={open} expandable expandOnRowClick keepContentWhenOpen onToggle={() => setOpen(v => !v)}
 collapsedContent={<>{tool.summary && <><span className={css.sep} aria-hidden/><span className={css.summary} data-error={state === 'error' || undefined} data-stopped={state === 'stopped' || undefined}>{open ? tool.details?.expandedSummary ?? tool.summary : tool.summary}</span></>}{tool.background && <span className={css.summary}>{tx(tool.jobId ? 'common:activity.background-started' : 'common:activity.background-starting')}</span>}<span className={css.visuallyHidden} aria-label={tx('tools:tool-card.tool-status')}>{tool.background ? tx(tool.jobId ? 'common:activity.background-started' : 'common:activity.background-starting') : tx(`common:state.${tool.state}`)}</span></>}>
 <div className={css.bodyWrap}>
 {tool.details ? <ToolDetails model={tool.details} onOpenAgent={onOpenAgent}/>
 : tool.variant === 'bash' ? <TerminalBlock command={tool.input ?? ''} output={tool.output} exitCode={tool.exitCode} background={tool.background}
   lifecycle={{ state: tool.state === 'success' ? 'done' : tool.state === 'failure' ? 'error' : tool.state === 'running' ? 'ongoing' : tool.state === 'assembled' ? 'idle' : 'warning', label: tx(`common:state.${tool.state}`) }}
   running={tool.state === 'running' || tool.state === 'assembled'} maxLines={Infinity} className={css.terminalBody}
   labels={{ ...labels, signal: s => tx('tools:copy.signal-value', { p0: s }), exitCode: n => tx('tools:copy.exit-value', { p0: n }), noExitCode: tx('tools:tool-card.exit-status-unavailable'), running: tx('tools:tool-card.running'), failed: tx('tools:tool-card.failed'), done: tx('tools:tool-card.done'), noOutput: tx('tools:tool-card.no-output') }}/>
 : (tool.variant === 'write' || tool.variant === 'edit') && !['failure', 'cancelled', 'uncertain'].includes(tool.state) && tool.diffs?.length ? <><small className={css.diffCaption}>{tx('tools:tool-card.requested-changes')}</small><DiffBlock className={css.diffBody} diffs={tool.diffs} maxLines={8} labels={{ ...labels, files: n => tx('tools:copy.value-file-s', { p0: n }) }}/>{tool.output && !tool.background && <pre className={css.searchRecovery}>{tool.output}</pre>}</>
 : tool.variant === 'read' && tool.state === 'success' && !tool.background && tool.output !== undefined ? <ReadBlock className={css.readBody} label={tool.path} lines={tool.output.split('\n').map(text => ({ text }))} maxLines={8} labels={{ ...labels, window: (n, total) => tx('tools:copy.value-of-value-lines', { p0: n, p1: total }) }}/>
 : tool.variant === 'search' && tool.state === 'success' && !tool.background && tool.output !== undefined ? <SearchTextBlock className={css.searchBody} text={tool.output} label={tool.summary} truncated={!!tool.truncated}/>
 : tool.variant === 'image' && tool.state === 'success' ? <div className={css.imageBody}><div className={css.imageLabel}>{tool.path}</div>{tool.artifacts}{tool.output && <div className={css.imageMeta}>{tool.output}</div>}</div>
 : <div className={css.ioCard}>
   {tool.input && <div className={css.ioSection}><span className={css.ioLabel}>{tx('tools:tool-card.in')}</span><span className={css.ioText}>{tool.input}</span></div>}
   {tool.input && tool.output && !tool.background && <span className={css.ioDivider} aria-hidden/>}
   {tool.output && !tool.background && <div className={css.ioSection}><span className={css.ioLabel}>{tx('tools:tool-card.out')}</span><span className={css.ioText} data-error={state === 'error' || undefined}>{tool.output}</span></div>}
 </div>}
 {(tool.variant !== 'image' || tool.state !== 'success') && tool.artifacts}
 {tool.jobId && <small>{tx('common:activity.job-title', { id: tool.jobId })}</small>}
 {inspect && <ToolInspectionButton inspect={inspect}/>}
 </div></DisclosureRow></div>{children && <div className={tree.subCalls}>{children}</div>}</div>;
}

/** Pure presentation: the Conversation owner supplies native location and fences. */
export function ToolInspectionButton({ inspect }: { inspect: () => Promise<void> }) {
  const tx = useTranslation(), [pending, setPending] = useState(false), [error, setError] = useState<string>();
  const run = async () => {
    if (pending) return;
    setPending(true); setError(undefined);
    try { await inspect(); } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setPending(false); }
  };
  return <><button className={css.inspectButton} type="button" disabled={pending} aria-busy={pending} onClick={() => void run()}><IconInspectOutline12/>{tx('tools:tool-card.inspect')}</button>{error && <span role="alert">{error}</span>}</>;
}
