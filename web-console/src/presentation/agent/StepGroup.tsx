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
import css from './StepGroup.module.css';

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
  return <div className={css.root} hidden={hidden} data-step-process={id}>
    <button hidden={direct} type="button" className={css.title} aria-expanded={open} aria-controls={bodyId}
      data-process-activity={activity} onClick={event => { event.currentTarget.focus(); onToggle(); }}>
      <span className={css.leading} aria-hidden="true">
        <span className={css.activityIcon}>{ICONS[activity]}</span>
        <span className={css.chevron}>{open ? <IconChevronUpOutline14/> : <IconChevronDownOutline14/>}</span>
      </span>
      <span className={css.label}>{title}{detail && ` · ${detail}`}</span>
    </button>
    <div ref={body} id={bodyId} hidden={!open && !direct} data-step-process-body
      className={clsx(css.body, direct && css.expandedBody, !direct && open && edges.up && css.fadeTop, !direct && open && edges.down && css.fadeBottom)}>
      <div ref={content} className={css.content}>{children}</div>
    </div>
  </div>;
}
