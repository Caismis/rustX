import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-chat ChatGroupSeat ProcessGroupHeader and
// step-process processTitle: a settled process group's activity title with its
// hover/expanded chevron, over a capped body whose scrollable ends fade.
// Native composition supplies live titles and whether a presentation mode
// exposes the body directly; manual disclosure state remains with its owner.
import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { IconAgentPresetOutline16, IconApiOutline14, IconBrowseOutline16, IconChevronDownOutline14, IconChevronUpOutline14, IconCodeOutline16, IconEditOutline16, IconGlobeOutline14, IconPlanOutline14, IconQuestionOutline14, IconSearchOutline16, IconSparkle16, IconThinkOutline14 } from '../primitives/icons';
import { TextShimmer } from '../primitives/TextShimmer';
import { useSearchableHidden } from './searchable-hidden';
import { useProcessScroll } from './use-process-scroll';
import css from './StepGroup.module.css';

const PROCESS_TITLE_MINIMUM_MS = 150;

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

interface LiveTitleState {
  readonly activity: StepActivity | 'thinking';
  readonly title: string;
  readonly detail: string;
}

function useStableLiveTitle(desired: LiveTitleState, active: boolean): LiveTitleState {
  const [displayed, setDisplayed] = useState(desired);
  const displayedRef = useRef(displayed);
  const desiredRef = useRef(desired);
  const displayedAtRef = useRef(Date.now());
  useEffect(() => {
    desiredRef.current = desired;
    if (!active || (displayedRef.current.activity === desired.activity
      && displayedRef.current.title === desired.title
      && displayedRef.current.detail === desired.detail)) return;
    const remaining = PROCESS_TITLE_MINIMUM_MS - (Date.now() - displayedAtRef.current);
    const commit = () => {
      const next = desiredRef.current;
      displayedRef.current = next;
      displayedAtRef.current = Date.now();
      setDisplayed(next);
    };
    if (remaining <= 0) {
      commit();
      return;
    }
    const timer = setTimeout(commit, remaining);
    return () => { clearTimeout(timer); };
  }, [active, desired.activity, desired.title, desired.detail]);
  return active ? displayed : desired;
}

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

export function StepGroup({ id, counts, open, onToggle, onReveal, hidden = false, direct = false, running = false, liveActivity, activityTitle, detail, children }: {
  id: string; counts: StepCounts; open: boolean; onToggle: () => void; onReveal?: () => void; hidden?: boolean; direct?: boolean; running?: boolean; liveActivity?: StepActivity | 'thinking'; activityTitle?: string; detail?: string; children: ReactNode;
}) {
  const defaultTitle = useProcessTitle(counts);
  const stableLive = useStableLiveTitle({
    activity: liveActivity ?? counts[0]?.kind ?? 'thinking',
    title: activityTitle ?? defaultTitle,
    detail: detail ?? '',
  }, running);
  const activity = running ? stableLive.activity : counts[0]?.kind ?? 'thinking';
  const title = running ? stableLive.title : activityTitle ?? defaultTitle;
  const shownDetail = running ? stableLive.detail : detail ?? '';
  const fullTitle = shownDetail ? `${title} · ${shownDetail}` : title;
  const bodyId = useId();
  const reveal = useCallback(() => {
    if (onReveal) onReveal();
    else if (!open) onToggle();
  }, [onReveal, onToggle, open]);
  const body = useSearchableHidden(!open && !direct, reveal);
  const content = useRef<HTMLDivElement>(null);
  const { edges, events, initialize } = useProcessScroll(body, content, open && !direct, !direct);
  const handleToggle = useCallback(() => {
    if (!open) initialize(running ? 'bottom' : 'top');
    onToggle();
  }, [initialize, onToggle, open, running]);

  return <div className={css.root} hidden={hidden} data-step-process={id} data-group-expanded-mode={direct || undefined}>
    <button hidden={direct} type="button" className={css.title} aria-expanded={open} aria-controls={bodyId}
      data-process-activity={activity} onClick={event => { event.currentTarget.focus(); handleToggle(); }}>
      <span className={css.leading} aria-hidden="true">
        <span className={css.activityIcon} data-step-process-icon>{ICONS[activity]}</span>
        <span className={css.chevron} data-step-process-chevron>{open ? <IconChevronUpOutline14/> : <IconChevronDownOutline14/>}</span>
      </span>
      <TextShimmer active={running}>
        <TextShimmer className={css.label}>{fullTitle}</TextShimmer>
      </TextShimmer>
    </button>
    <div ref={body} id={bodyId} data-step-process-body
      data-scroll-up={edges.canScrollUp || undefined} data-scroll-down={edges.canScrollDown || undefined}
      className={clsx(css.body, direct && css.expandedBody, !direct && open && edges.canScrollUp && css.fadeTop, !direct && open && edges.canScrollDown && css.fadeBottom)}
      {...events}>
      <div ref={content} className={css.content} data-step-process-content>{children}</div>
    </div>
  </div>;
}
