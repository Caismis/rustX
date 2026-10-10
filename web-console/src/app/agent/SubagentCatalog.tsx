/* Copyright (c) 2026 DeepSeek. MIT. Catalog tree and hover/focus behavior adapted from ui-subagent/SubagentHeaderLineage; see PROVENANCE.md. */
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { autoUpdate, flip, offset, shift, size, useFloating } from '@floating-ui/react-dom';
import type { RuntimeClientAgent } from '../../../../protocol/app-server/v44';
import { useTranslation } from '../../locale/react';
import { StateDot } from '../../presentation/primitives/StateDot';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { IconChevronDownOutline14, IconChevronRightOutline14 } from '../../presentation/primitives/icons';
import { useSubagents } from './subagent-context';
import { agentDot, agentRunning, agentStatus, agentDuration } from './subagent-state';
import { formatDuration, formatTokens } from './token-format';
import css from './SubagentCatalog.module.css';

function SwitcherIcon() {
  return <svg width="16" height="16" viewBox="0 0 20 20" fill="none" aria-hidden="true">
    <path d="M5.99951 12.7L8.95546 14.9478C9.40011 15.2859 9.62244 15.455 9.87526 15.488C9.95774 15.4988 10.0413 15.4988 10.1238 15.488C10.3766 15.455 10.5989 15.2859 11.0436 14.9478L13.9995 12.7" stroke="currentColor" strokeWidth="1.5"/>
    <path d="M13.9995 7.7417L11.0436 5.49387C10.5989 5.15574 10.3766 4.98668 10.1238 4.95362C10.0413 4.94283 9.95775 4.94283 9.87527 4.95362C9.62245 4.98668 9.40012 5.15574 8.95547 5.49387L5.99952 7.7417" stroke="currentColor" strokeWidth="1.5"/>
  </svg>;
}
/** One coherent native inventory supplies both roots and descendants. */
export function SubagentMenu({ agents, selected, label, openChild, openTitle }: { agents: RuntimeClientAgent[]; selected?: string; label: string; openChild: (id: string) => void; openTitle?: () => void }) {
  const tx = useTranslation(), scope = useSubagents()!;
  const [open, setOpen] = useState(false), [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set()), [now, setNow] = useState(Date.now);
  const trigger = useRef<HTMLButtonElement>(null), menu = useRef<HTMLDivElement>(null), pinned = useRef(false);
  const hoverOpen = useRef<ReturnType<typeof setTimeout> | undefined>(undefined), hoverClose = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const { refs, floatingStyles, isPositioned } = useFloating({ open, placement: 'bottom-start', strategy: 'fixed', whileElementsMounted: autoUpdate,
    middleware: [offset(5), flip({ padding: 16 }), shift({ padding: 16 }), size({ padding: 16, apply: ({ availableHeight, elements }) => { elements.floating.style.maxHeight = `${Math.min(560, availableHeight)}px`; } })] });
  const cancelOpen = () => { clearTimeout(hoverOpen.current); hoverOpen.current = undefined; };
  const cancelClose = () => { clearTimeout(hoverClose.current); hoverClose.current = undefined; };
  const close = (restore = false) => { cancelOpen(); cancelClose(); pinned.current = false; setOpen(false); setExpanded(new Set()); if (restore) trigger.current?.focus({ preventScroll: true }); };
  const scheduleOpen = () => { cancelOpen(); cancelClose(); if (!open) hoverOpen.current = setTimeout(() => setOpen(true), 150); };
  const scheduleClose = () => { cancelOpen(); cancelClose(); if (!pinned.current) hoverClose.current = setTimeout(() => close(), 120); };
  const items = () => [...(menu.current?.querySelectorAll<HTMLElement>('[role=treeitem]') ?? [])];
  const focusAt = (index: number) => { const rows = items(); if (rows.length) rows[(index + rows.length) % rows.length].focus({ preventScroll: true }); };
  useEffect(() => () => { cancelOpen(); cancelClose(); }, []);
  useEffect(() => {
    if (!open) return;
    const dismiss = (event: PointerEvent) => { if (event.target instanceof Node && !trigger.current?.contains(event.target) && !menu.current?.contains(event.target)) close(); };
    document.addEventListener('pointerdown', dismiss); return () => document.removeEventListener('pointerdown', dismiss);
  }, [open]);
  useLayoutEffect(() => { if (open && isPositioned && pinned.current) focusAt(0); }, [open, isPositioned]);
  useEffect(() => {
    if (!open || !scope.agents.some(agentRunning)) return;
    setNow(Date.now()); const clock = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(clock);
  }, [open, scope.agents]);
  const navigate = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.nativeEvent.isComposing || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    const index = items().indexOf(document.activeElement as HTMLElement);
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(true); }
    else if (event.key === 'Home') { event.preventDefault(); focusAt(0); }
    else if (event.key === 'End') { event.preventDefault(); focusAt(items().length - 1); }
    else if (event.key === 'ArrowDown') { event.preventDefault(); focusAt(index + 1); }
    else if (event.key === 'ArrowUp') { event.preventDefault(); focusAt(index < 0 ? items().length - 1 : index - 1); }
  };
  const toggle = (id: string) => setExpanded(previous => {
    const next = new Set(previous);
    if (!next.has(id)) next.add(id);
    else {
      const collapse = (parent: string) => { next.delete(parent); scope.agents.filter(agent => agent.parent_agent_id === parent && next.has(agent.agent_id)).forEach(agent => collapse(agent.agent_id)); };
      collapse(id);
    }
    return next;
  });
  const rows = (entries: RuntimeClientAgent[], level = 1, lineage: ReadonlySet<string> = new Set()): ReactNode => entries.map(agent => {
    if (lineage.has(agent.agent_id)) return null;
    const children = scope.agents.filter(child => child.parent_agent_id === agent.agent_id && !lineage.has(child.agent_id));
    const branch = children.length > 0, isExpanded = expanded.has(agent.agent_id), current = agent.agent_id === selected;
    const metrics = scope.metrics[agent.agent_id], usage = metrics?.statistics.reported_usage;
    const duration = metrics && formatDuration(Math.floor(agentDuration(metrics, now) / 1000) * 1000, tx);
    const select = () => { openChild(agent.agent_id); close(); };
    return <div key={agent.agent_id} className={css.node}>
      <div role="treeitem" tabIndex={0} aria-level={level} aria-current={current || undefined} aria-expanded={branch ? isExpanded : undefined} aria-label={`${agent.title} ${agentStatus(agent, tx)}`} className={css.row} data-agent-id={agent.agent_id} data-agent-state={agent.state} onClick={select}
        onKeyDown={event => { if (event.nativeEvent.isComposing) return; if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); event.stopPropagation(); select(); } else if (branch && (event.key === 'ArrowRight' && !isExpanded || event.key === 'ArrowLeft' && isExpanded)) { event.preventDefault(); event.stopPropagation(); toggle(agent.agent_id); } }}>
        {branch ? <button type="button" tabIndex={-1} className={`${css.disclosure} ${isExpanded ? css.disclosureOpen : ''}`} aria-label={tx(isExpanded ? 'common:subagents.collapse-branch' : 'common:subagents.expand-branch', { name: agent.title })} onClick={event => { event.stopPropagation(); toggle(agent.agent_id); }}><span aria-hidden="true"><IconChevronRightOutline14/></span></button> : entries.some(entry => scope.agents.some(child => child.parent_agent_id === entry.agent_id)) && <span className={css.disclosureSpace}/>}
        <div className={css.clickarea}>
          <span className={css.rowActivitySlot}><StateDot state={agentDot(agent)} size={7}/></span>
          <span className={css.content}><span className={`${css.label} ${current ? css.currentLabel : ''}`}>{agent.title}</span><span className={css.summary}>{agent.agent} · {agentStatus(agent, tx)}</span></span>
          {metrics && <span className={css.metrics}>{usage && <span className={css.metricToken}>{tx('agent:usage.count', { count: formatTokens(usage.total_tokens, tx) })}</span>}<span className={css.metricDuration} title={duration}>{duration}</span></span>}
          {!current && scope.openAside && <Tooltip label={tx('common:subagents.open-aside', { name: agent.title })} side="bottom" delayMs={500}><button type="button" className={css.sidebarButton} aria-label={tx('common:subagents.open-aside', { name: agent.title })} onKeyDown={event => event.stopPropagation()} onClick={event => { event.stopPropagation(); scope.openAside?.(agent.agent_id); close(); }}><span aria-hidden="true"><IconChevronRightOutline14/></span></button></Tooltip>}
        </div>
      </div>
      {branch && isExpanded && <div role="group" className={css.children}>{rows(children, level + 1, new Set([...lineage, agent.agent_id]))}</div>}
    </div>;
  });
  return <div className={`${css.root} ${selected ? css.switcherRoot : ''}`} onMouseEnter={scheduleOpen} onMouseLeave={scheduleClose}>
    <button ref={element => { trigger.current = element; refs.setReference(element); }} id={selected && !openTitle ? 'session-title' : undefined} type="button" className={selected ? `${css.switcherTrigger} ${openTitle ? css.ancestorSwitcherTrigger : ''}` : css.trigger} aria-label={openTitle ? label : tx('common:subagents.list')} aria-haspopup="tree" aria-expanded={open}
      onClick={() => { cancelOpen(); cancelClose(); if (openTitle) { close(); openTitle(); } else { pinned.current = true; setOpen(true); } }}
      onKeyDown={event => { if (event.key === 'ArrowDown') { event.preventDefault(); event.stopPropagation(); cancelOpen(); cancelClose(); pinned.current = true; setOpen(true); if (open && isPositioned) focusAt(0); } }}>
      {!selected && agents.some(agentRunning) && <span className={css.activitySlot}><StateDot state="ongoing"/></span>}<span className={selected ? css.switcherTitle : css.count}>{label}</span>{selected ? <SwitcherIcon/> : <span aria-hidden="true"><IconChevronDownOutline14 className={open ? css.triggerOpen : undefined}/></span>}
    </button>
    {open && createPortal(<div ref={element => { menu.current = element; refs.setFloating(element); }} className={css.menu} style={floatingStyles} onMouseEnter={cancelClose} onMouseLeave={scheduleClose} onKeyDown={navigate}><div className={css.menuBody} role="tree" aria-label={tx('common:subagents.list')}>{rows(agents)}</div></div>, document.body)}
  </div>;
}
