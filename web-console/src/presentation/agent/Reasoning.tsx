import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Derived from pinned Harness; see PROVENANCE.md. */
import { useState } from 'react';
import { DisclosureRow } from '../primitives/DisclosureRow';
import { IconThinkOutline14 } from '../primitives/icons';
import { MarkdownText } from '../markdown/MarkdownText';
import { TextShimmer } from '../primitives/TextShimmer';
import css from './Reasoning.module.css';

/** Advance the running preview only when a paragraph's first line is complete. */
function previewLine(text: string, running: boolean): string {
 if (!running) return text.split('\n', 1)[0];
 let summary = '', paragraphStart = 0;
 const separator = /\r?\n(?:[\t ]*\r?\n)+/gu;
 while (true) {
  const nextParagraph = separator.exec(text);
  const paragraphEnd = nextParagraph === null ? text.length : nextParagraph.index + nextParagraph[0].indexOf('\n');
  const newline = text.indexOf('\n', paragraphStart);
  if (newline !== -1 && newline <= paragraphEnd) {
   const candidate = text.slice(paragraphStart, newline).trim();
   if (candidate) summary = candidate;
  }
  if (nextParagraph === null) return summary;
  paragraphStart = nextParagraph.index + nextParagraph[0].length;
 }
}
export function Reasoning({ text, running = false, hidden = false, preview = true }: { text: string; running?: boolean; hidden?: boolean; preview?: boolean }) {
  const tx = useTranslation();
 const [expanded, setExpanded] = useState(false);
 const summary = previewLine(text, running).replaceAll('**', '');
 return <div hidden={hidden} className={css.root} data-variant="think" data-state={running ? 'running' : 'ok'} data-expanded={expanded || undefined} data-preview={preview && !expanded && !!summary || undefined}>
 <DisclosureRow rowClassName={css.row} leadingClassName={css.leading} titleClassName={css.title} running={running}
 icon={<IconThinkOutline14 size={14}/>} title={tx('agent:turn-tail.reasoning')} open={expanded} expandable expandOnRowClick onToggle={() => setExpanded(v => !v)}
 collapsedContent={<><span className={css.separator} data-shimmer-decoration aria-hidden/><span className={css.summary} data-streaming={running || undefined}><span className={css.summaryText}><TextShimmer>{summary}</TextShimmer></span></span></>}>
 <div className={css.thinkBody}><MarkdownText variant="compact" text={text} streaming={running}/></div></DisclosureRow></div>;
}
