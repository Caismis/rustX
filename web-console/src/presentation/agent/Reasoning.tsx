import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Derived from pinned Harness; see PROVENANCE.md. */
import { memo, useState } from 'react';
import { DisclosureRow } from '../primitives/DisclosureRow';
import { IconThinkOutline14 } from '../primitives/icons';
import { MarkdownText } from '../markdown/MarkdownText';
import { TextShimmer } from '../primitives/TextShimmer';

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
export const Reasoning = memo(function Reasoning({ text, running = false, hidden = false, preview = true }: { text: string; running?: boolean; hidden?: boolean; preview?: boolean }) {
  const tx = useTranslation();
 const [expanded, setExpanded] = useState(false);
 const summary = previewLine(text, running).replaceAll('**', '');
 const showPreview = preview && !expanded && summary !== '';
 return <div hidden={hidden} className="flex flex-col [&:not([data-expanded])]:[contain:size_layout] [&:not([data-expanded])]:h-[calc(24px+var(--dsh-content-font-delta,0px))] [&[data-expanded]_[data-open]_[data-disclosure-header]]:sticky [&[data-expanded]_[data-open]_[data-disclosure-header]]:top-0 [&[data-expanded]_[data-open]_[data-disclosure-header]]:z-[1] [&[data-expanded]_[data-open]_[data-disclosure-header]]:bg-[var(--dsw-alias-bg-base)]" data-variant="think" data-state={running ? 'running' : 'ok'} data-expanded={expanded || undefined} data-preview={showPreview || undefined}>
 <DisclosureRow rowClassName="overflow-hidden" leadingClassName="shrink-0" titleClassName="font-normal" running={running}
 icon={<IconThinkOutline14 size={14}/>} title={tx('agent:turn-tail.reasoning')} open={expanded} expandable expandOnRowClick onToggle={() => setExpanded(v => !v)}
 collapsedContent={showPreview ? <><span className="flex-none w-[2px] h-[2px] mx-[8px] my-0 rounded-[1px] bg-[var(--dsw-alias-label-caption)]" data-shimmer-decoration aria-hidden/><span className="group/sum min-w-0 overflow-hidden flex-auto text-[var(--dsh-content-font-size-secondary,13px)] leading-[calc(20px+var(--dsh-content-font-delta-secondary,0px))] whitespace-nowrap data-[streaming]:[mask-image:linear-gradient(to_right,black_calc(100%-48px),transparent)]" data-streaming={running || undefined}><span className="block overflow-hidden text-ellipsis group-data-[streaming]/sum:overflow-visible group-data-[streaming]/sum:text-clip"><TextShimmer>{summary}</TextShimmer></span></span></> : undefined}>
 <div className="pt-[4px] px-0 pb-[4px] pl-[calc(22px+var(--dsh-content-font-delta,0px))] min-w-0"><MarkdownText variant="compact" text={text} streaming={running}/></div></DisclosureRow></div>;
});
