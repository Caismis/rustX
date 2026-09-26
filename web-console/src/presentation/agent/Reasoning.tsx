import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Derived from pinned Harness; see PROVENANCE.md. */
import { useState } from 'react';
import { DisclosureRow } from '../primitives/DisclosureRow';
import { IconThinkOutline14 } from '../primitives/icons';
import { MarkdownText } from '../markdown/MarkdownText';
import css from './Reasoning.module.css';
export function Reasoning({ text, running = false }: { text: string; running?: boolean }) {
  const tx = useTranslation();
 const [expanded, setExpanded] = useState(false);
 const lines = text.trimEnd().split('\n');
 const summary = (running ? lines.at(-1) : lines[0])?.replaceAll('**', '');
 return <div className={css.root} data-variant="think" data-state={running ? 'running' : 'ok'} data-expanded={expanded || undefined}>
 <DisclosureRow rowClassName={css.row} leadingClassName={css.leading} titleClassName={css.title} chevronClassName={css.chevron}
 icon={<IconThinkOutline14 size={14}/>} title={tx('agent:turn-tail.reasoning')} open={expanded} expandable expandOnRowClick onToggle={() => setExpanded(v => !v)}
 collapsedContent={<><span className={css.separator}/><span className={css.summary}><span className={css.summaryText}>{summary}</span></span></>}>
 <div className={css.thinkBody}><MarkdownText variant="compact" text={text} streaming={running}/></div></DisclosureRow></div>;
}
