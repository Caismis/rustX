import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Derived from pinned Harness; see PROVENANCE.md. */
import { useState, type ReactNode } from 'react';
import { DisclosureRow } from '../primitives/DisclosureRow';
import { StateDot } from '../primitives/StateDot';
import { IconApiOutline14, IconBrowseOutline16, IconEditOutline16, IconSearchOutline16, IconSparkle16 } from '../primitives/icons';
import css from './Tool.module.css';
import tree from './ToolTree.module.css';
import { TerminalBlock } from '../primitives/TerminalBlock';
import { DiffBlock, type DiffHunk } from '../primitives/DiffBlock';
import { ReadBlock } from '../primitives/ReadBlock';
import { SearchTextBlock } from '../primitives/SearchBlock';


export interface ToolCardView {
 id: string; nativeName?: string; identity?: 'call' | 'execution'; title: string; summary: string; state: 'assembled' | 'running' | 'success' | 'failure' | 'cancelled' | 'uncertain' | 'starting' | 'cancelling' | 'publishing_terminal';
 variant: 'generic' | 'bash' | 'read' | 'write' | 'edit' | 'search'; input?: string; output?: string;
 artifacts?: ReactNode; path?: string; exitCode?: number | null; truncated?: boolean; diffs?: DiffHunk[];
}
const icons = { generic: IconSparkle16, bash: IconApiOutline14, read: IconBrowseOutline16, write: IconEditOutline16, edit: IconEditOutline16, search: IconSearchOutline16 };
/** One dispatch path, one native call. Expansion never changes lifecycle. */
export function ToolCard({ tool, children }: { tool: ToolCardView; children?: ReactNode }) {
  const tx = useTranslation();
  const labels = { copy: tx('tools:tool-card.copy'), copied: tx('tools:tool-card.copied'), collapseAria: tx('tools:tool-card.collapse-output'), collapse: tx('tools:tool-card.collapse'), expandAria: (n: number) => tx('tools:tool-card.show-value-more-lines', { p0: n }), expand: (n: number) => tx('tools:tool-card.show-value-more-lines', { p0: n }) };
 const [open, setOpen] = useState(false);
 const Icon = icons[tool.variant];
 const state = tool.state === 'failure' ? 'error' : tool.state === 'cancelled' || tool.state === 'uncertain' ? 'stopped' : tool.state === 'success' ? 'ok' : tool.state;
 return <div className={tree.callRow} data-tool-call-id={tool.identity !== 'execution' ? tool.id : undefined} data-execution-id={tool.identity === 'execution' ? tool.id : undefined} data-tool-renderer={tool.variant} data-tool-name={tool.nativeName}>
 <div className={css.root} data-state={state} data-variant={tool.variant}>
 <DisclosureRow rowClassName={css.row} leadingClassName={css.leading} titleClassName={css.title} chevronClassName={css.chevron}
 icon={state === 'error' || state === 'stopped' ? <StateDot state={state === 'error' ? 'error' : 'warning'}/> : <Icon size={14}/>}
 title={tool.title} open={open} expandable expandOnRowClick keepContentWhenOpen onToggle={() => setOpen(v => !v)}
 collapsedContent={<><span className={css.sep}/><span className={css.summary}>{tool.summary}</span><small aria-label={tx('tools:tool-card.tool-status')}>{tx(`common:state.${tool.state}`)}</small></>}>
 <div className={css.bodyWrap}>
 {tool.variant === 'bash' ? <TerminalBlock command={tool.input ?? ''} output={tool.output} exitCode={tool.exitCode}
   lifecycle={{ state: tool.state === 'success' ? 'done' : tool.state === 'failure' ? 'error' : tool.state === 'running' ? 'ongoing' : tool.state === 'assembled' ? 'idle' : 'warning', label: tx(`common:state.${tool.state}`) }}
   running={tool.state === 'running' || tool.state === 'assembled'} maxLines={16}
   labels={{ ...labels, signal: s => tx('tools:copy.signal-value', { p0: s }), exitCode: n => tx('tools:copy.exit-value', { p0: n }), noExitCode: tx('tools:tool-card.exit-status-unavailable'), running: tx('tools:tool-card.running'), failed: tx('tools:tool-card.failed'), done: tx('tools:tool-card.done'), noOutput: tx('tools:tool-card.no-output') }}/>
 : (tool.variant === 'write' || tool.variant === 'edit') && tool.diffs?.length ? <><small>{tx('tools:tool-card.requested-changes')}</small><DiffBlock diffs={tool.diffs} maxLines={8} labels={{ ...labels, files: n => tx('tools:copy.value-file-s', { p0: n }) }}/>{tool.output && <pre className={css.ioText}>{tool.output}</pre>}</>
 : tool.variant === 'read' && tool.output !== undefined ? <ReadBlock label={tool.path} lines={tool.output.split('\n').map(text => ({ text }))} maxLines={8} labels={{ ...labels, window: (n, total) => tx('tools:copy.value-of-value-lines', { p0: n, p1: total }) }}/>
 : tool.variant === 'search' && tool.output !== undefined ? <SearchTextBlock text={tool.output} label={tool.summary} truncated={!!tool.truncated}/>
 : <div className={css.ioCard}>
   {tool.input && <div className={css.ioSection}><span className={css.ioLabel}>{tx('tools:tool-card.in')}</span><pre className={css.ioText}>{tool.input}</pre></div>}
   {tool.output && <div className={css.ioSection}><span className={css.ioLabel}>{tx('tools:tool-card.out')}</span><pre className={css.ioText} data-error={state === 'error' || undefined}>{tool.output}</pre></div>}
 </div>}
 {tool.artifacts}
 </div></DisclosureRow></div>{children && <div className={tree.subCalls}>{children}</div>}</div>;
}
