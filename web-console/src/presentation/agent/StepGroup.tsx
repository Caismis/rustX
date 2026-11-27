import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-chat ChatGroupSeat ProcessGroupHeader and
// step-process processTitle: a settled process group's activity title with its
// hover/expanded chevron, over a capped body whose scrollable ends fade.
// Native composition supplies live titles and whether a presentation mode
// exposes the body directly; manual disclosure state remains with its owner.
import { useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { IconAgentPresetOutline16, IconApiOutline14, IconBrowseOutline16, IconChevronDownOutline14, IconChevronUpOutline14, IconCodeOutline16, IconEditOutline16, IconGlobeOutline14, IconPlanOutline14, IconQuestionOutline14, IconSearchOutline16, IconSparkle16, IconThinkOutline14 } from '../primitives/icons';

/** Harness step-process categories, ranked by distinct calls in a group's title. */
export type StepActivity = 'read' | 'readImage' | 'search' | 'write' | 'edit' | 'commands' | 'code'
 | 'webSearch' | 'webFetch' | 'subagents' | 'plan' | 'questions' | 'tools';
export type StepCounts = readonly { readonly kind: StepActivity; readonly count: number }[];

const ICONS: Record<StepActivity | 'thinking', ReactNode> = {
 thinking: <IconThinkOutline14/>,
 read: <IconBrowseOutline16 size={14}/>,
 readImage: <IconBrowseOutline16 size={14}/>,
 search: <IconSearchOutline16 size={14}/>,
 edit: <IconEditOutline16 size={14}/>,
 write: <IconEditOutline16 size={14}/>,
 commands: <IconApiOutline14/>,
 code: <IconCodeOutline16 size={14}/>,
 webSearch: <IconGlobeOutline14/>,
 webFetch: <IconBrowseOutline16 size={14}/>,
 subagents: <IconAgentPresetOutline16 size={14}/>,
 plan: <IconPlanOutline14/>,
 questions: <IconQuestionOutline14/>,
 tools: <IconSparkle16 size={14}/>,
};

/** A closed group's title from its top three categories, without counts. */
function useProcessTitle(counts: StepCounts) {
 const tx = useTranslation();
 const labels = counts.slice(0, 3).map(({ kind }) => tx(`agent:step-process.done.${kind}`));
 const [first, second] = labels;
 if (first === undefined) return tx('agent:step-process.done.thinking');
 const continuation = (label: string) => label.charAt(0).toLowerCase() + label.slice(1);
 if (second === undefined) return first;
 if (labels.length === 2) {
 const prefix = tx('agent:step-process.shared-prefix');
 const shared = prefix !== '' && first.startsWith(prefix) && second.startsWith(prefix);
 return tx('agent:step-process.join-two', { first, second: continuation(shared ? second.slice(prefix.length) : second) });
 }
 const title = [first, ...labels.slice(1).map(continuation)].join(tx('agent:step-process.comma'));
 return counts.length > 3 ? tx('agent:step-process.more', { title }) : title;
}

export function StepGroup({ id, counts, open, onToggle, hidden = false, direct = false, activityTitle, detail, children }: {
 id: string; counts: StepCounts; open: boolean; onToggle: () => void; hidden?: boolean; direct?: boolean; activityTitle?: string; detail?: string; children: ReactNode;
}) {
 const defaultTitle = useProcessTitle(counts);
 const title = activityTitle ?? defaultTitle;
 const bodyId = useId();
 const body = useRef<HTMLDivElement>(null);
 const content = useRef<HTMLDivElement>(null);
 const [edges, setEdges] = useState({ up: false, down: false });
 useLayoutEffect(() => {
 const element = body.current;
 if (!open || !element) return;
 const sync = () => {
 const floor = element.scrollHeight - element.clientHeight;
 const next = { up: element.scrollTop > 1, down: element.scrollTop < floor - 1 };
 setEdges(previous => previous.up === next.up && previous.down === next.down ? previous : next);
 };
 sync();
 element.addEventListener('scroll', sync);
 if (typeof ResizeObserver === 'undefined') return () => element.removeEventListener('scroll', sync);
 const observer = new ResizeObserver(sync);
 observer.observe(element);
 if (content.current) observer.observe(content.current);
 return () => { observer.disconnect(); element.removeEventListener('scroll', sync); };
 }, [open]);
 const activity = counts[0]?.kind ?? 'thinking';
 return <div className="min-w-0" hidden={hidden} data-step-process={id}>
 <button hidden={direct} type="button" className="group/step flex max-w-full items-center gap-[6px] p-0 border-0 bg-none [color:var(--dsw-alias-label-tertiary)] [font-size:var(--dsh-content-font-size,14px)] leading-[calc(24px+var(--dsh-content-font-delta,0px))] text-left cursor-pointer transition-colors duration-100 motion-reduce:transition-none focus-visible:[color:var(--dsw-alias-label-secondary)] hover:[color:var(--dsw-alias-label-secondary)] aria-[expanded=true]:pb-[8px]" aria-expanded={open} aria-controls={bodyId}
 data-process-activity={activity} onClick={event => { event.currentTarget.focus(); onToggle(); }}>
 <span className="relative flex-none inline-flex items-center justify-center w-[calc(16px+var(--dsh-content-font-delta,0px))] h-[calc(16px+var(--dsh-content-font-delta,0px))] [color:inherit] [&_svg]:w-[calc(14px+var(--dsh-content-font-delta,0px))] [&_svg]:h-[calc(14px+var(--dsh-content-font-delta,0px))] [&_svg[width='16']]:w-[calc(16px+var(--dsh-content-font-delta,0px))] [&_svg[width='16']]:h-[calc(16px+var(--dsh-content-font-delta,0px))]" aria-hidden="true">
 <span className="absolute inset-0 inline-flex items-center justify-center transition-opacity duration-100 motion-reduce:transition-none opacity-100 group-hover/step:opacity-0 group-focus-visible/step:opacity-0 group-aria-[expanded=true]/step:opacity-0">{ICONS[activity]}</span>
 <span className="absolute inset-0 inline-flex items-center justify-center transition-opacity duration-100 motion-reduce:transition-none opacity-0 group-hover/step:opacity-100 group-focus-visible/step:opacity-100 group-aria-[expanded=true]/step:opacity-100">{open ? <IconChevronUpOutline14/> : <IconChevronDownOutline14/>}</span>
 </span>
 <span className="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">{title}{detail && ` · ${detail}`}</span>
 </button>
 <div ref={body} id={bodyId} hidden={!open && !direct} data-step-process-body
 className={clsx('max-h-[min(400px,50vh)] overflow-y-auto overscroll-behavior-y-auto [scrollbar-gutter:stable] [--dsh-chat-flow-gap:4px]', direct && 'max-h-none! overflow-visible! [scrollbar-gutter:auto]!', !direct && open && edges.up && edges.down ? '[mask-image:linear-gradient(to_bottom,transparent_0,#000_24px,#000_calc(100%-24px),transparent_100%)]' : !direct && open && edges.up ? '[mask-image:linear-gradient(to_bottom,transparent_0,#000_24px,#000_100%)]' : !direct && open && edges.down ? '[mask-image:linear-gradient(to_bottom,#000_0,#000_calc(100%-24px),transparent_100%)]' : undefined)}>
 <div ref={content} className="flex flex-col [&>*]:shrink-0 [&>:not([hidden]):not(:empty)~:not([hidden]):not(:empty)]:mt-[var(--dsh-chat-flow-gap,4px)]">{children}</div>
 </div>
 </div>;
}
