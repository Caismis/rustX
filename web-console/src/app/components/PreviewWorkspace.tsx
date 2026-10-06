import { flushSync } from 'react-dom';
import { useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import { ArtifactPreview } from './ArtifactPreview';
import { PreviewWorkspaceOwner, clampSplit, splitBounds, splitFits, type PreviewPane, type PreviewWorkspaceSnapshot } from '../preview-workspace';
import { PREVIEW_POLICY } from '../../client/preview-policy';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import css from '../../presentation/right-panel/PreviewWorkspace.module.css';

/** Reveal keyboard focus inside the bounded strip without scrolling ancestors. */
function focusTab(tab: HTMLElement | null | undefined) {
  if (!tab) return;
  tab.focus({ preventScroll: true });
  const strip = tab.closest<HTMLElement>('[role="tablist"]'), item = tab.parentElement;
  if (!strip || !item) return;
  const viewport = strip.getBoundingClientRect(), bounds = item.getBoundingClientRect();
  const delta = bounds.left < viewport.left ? bounds.left - viewport.left : Math.max(0, bounds.right - viewport.right);
  // Let the browser clamp its fractional scroll range; integer scrollWidth /
  // clientWidth can truncate the final visible pixel of a tab or close control.
  if (delta) strip.scrollLeft = delta > 0 ? Math.ceil(strip.scrollLeft + delta) : Math.floor(strip.scrollLeft + delta);
}

/** One keyed body tree; only pane selection/visibility changes its lease. */
export function PreviewWorkspace({ owner, snapshot, focusRequest, returnFocus }: {
  owner: PreviewWorkspaceOwner; snapshot: PreviewWorkspaceSnapshot; focusRequest: number; returnFocus: () => void;
}) {
  const tx = useTranslation(), root = useRef<HTMLDivElement>(null), pendingFocus = useRef<number | undefined>(undefined);
  const width = snapshot.width;
  const workspace = snapshot.workspace, visible = snapshot.mode === 'preview' && !!workspace;
  const split = !!workspace && workspace.panes.length === 2 && splitFits(width);
  useLayoutEffect(() => {
    if (!visible) return;
    const element = root.current!, epoch = snapshot.geometryEpoch;
    const measure = () => { if (!element.hidden) owner.measure(element.getBoundingClientRect().width, epoch); };
    measure(); const observer = new ResizeObserver(measure); observer.observe(element);
    return () => observer.disconnect();
  }, [owner, visible, snapshot.geometryEpoch]);
  useLayoutEffect(() => {
    if (!visible) return;
    const id = workspace.panes.find(pane => pane.id === workspace.activePane)?.selected;
    if (id !== undefined) focusTab(root.current?.querySelector<HTMLElement>(`[data-preview-tab="${id}"]`));
  }, [focusRequest]);
  useLayoutEffect(() => {
    const id = pendingFocus.current; if (id === undefined) return; pendingFocus.current = undefined;
    if (!workspace?.tabs.length) { returnFocus(); return; }
    const selected = workspace.tabs.some(tab => tab.id === id) ? id : workspace.panes.find(pane => pane.id === workspace.activePane)?.selected;
    focusTab(root.current?.querySelector<HTMLElement>(`[data-preview-tab="${selected}"]`));
  }, [workspace, returnFocus, visible, split]);
  const close = (id: number) => { pendingFocus.current = id; owner.closeTab(id); };
  const focusPane = (pane: PreviewPane) => { pendingFocus.current = pane.selected; owner.activatePane(pane.id); };
  const reason = !splitFits(width) ? tx('artifacts:workspace.split-room') : workspace?.panes.length === 2 ? tx('artifacts:workspace.two-panes') : (workspace?.tabs.length ?? 0) < 2 ? tx('artifacts:workspace.split-documents') : undefined;
  const separatorId = 'preview-split-reason';
  const escape = (event: KeyboardEvent) => {
    if (event.key !== 'Escape' || event.defaultPrevented || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229 || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    event.preventDefault(); event.stopPropagation();
    if (event.repeat) return;
    if (workspace?.fullscreen) owner.toggleFullscreen(flushSync);
    else { owner.collapse(); returnFocus(); }
  };
  return <div ref={root} className={css.workspace} data-preview-workspace hidden={!visible} onKeyDown={escape}>
    {workspace && <>
      <div className={css.toolbar}>
        <Button size="sm" aria-label={tx('artifacts:workspace.split')} aria-disabled={!!reason} aria-describedby={reason ? separatorId : undefined} title={reason ?? tx('artifacts:workspace.split')} onClick={() => { if (!reason) { owner.split(); pendingFocus.current = owner.getSnapshot().workspace?.panes.find(p => p.id === owner.getSnapshot().workspace?.activePane)?.selected; } }}>{tx('artifacts:workspace.split')}</Button>
        <Button size="sm" aria-label={tx(workspace.fullscreen ? 'artifacts:workspace.restore' : 'artifacts:workspace.fullscreen')} onClick={() => owner.toggleFullscreen(flushSync)}>{tx(workspace.fullscreen ? 'artifacts:workspace.restore' : 'artifacts:workspace.fullscreen')}</Button>
        {workspace.panes.length === 2 && !split && <Button size="sm" onClick={() => focusPane(workspace.panes.find(pane => pane.id !== workspace.activePane)!)}>{tx('artifacts:workspace.switch-pane')}</Button>}
        {reason && <span id={separatorId} className={css.reason}>{reason}</span>}
      </div>
      <div className={css.panes} data-preview-split={split || undefined}>
        {workspace.panes.map((pane, index) => {
          const shown = split || pane.id === workspace.activePane, selected = workspace.tabs.find(tab => tab.id === pane.selected)!;
          return <div key={pane.id} className={css.paneGroup} style={split ? { flex: `${index === 0 ? clampSplit(workspace.ratio, width) : 1 - clampSplit(workspace.ratio, width)} 1 0` } : undefined} hidden={!shown}>
            {index === 1 && split && visible && <Divider width={width} ratio={workspace.ratio} commit={ratio => owner.setRatio(ratio)} retireFocus={() => {
              const current = owner.getSnapshot();
              if (current.mode === 'preview') pendingFocus.current = current.workspace?.panes.find(pane => pane.id === current.workspace?.activePane)?.selected;
            }} />}
            <section className={css.pane} data-preview-pane={pane.id} data-preview-active={pane.id === workspace.activePane || undefined} aria-label={tx('artifacts:workspace.pane', { p0: index + 1 })} onPointerDown={event => { if (!(event.target as HTMLElement).closest('[data-preview-download]')) owner.activatePane(pane.id); }} onFocus={event => { if (!(event.target as HTMLElement).closest('[data-preview-download]')) owner.activatePane(pane.id); }}>
              <div className={css.strip}>
                <div className={css.tabs} role="tablist" aria-label={tx('artifacts:workspace.tabs', { p0: index + 1 })} onKeyDown={event => {
                  if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229 || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
                  const tabs = [...event.currentTarget.querySelectorAll<HTMLElement>('[role="tab"]')];
                  const active = tabs.indexOf(document.activeElement as HTMLElement); let next: number;
                  if (event.key === 'ArrowRight') next = (active + 1) % tabs.length;
                  else if (event.key === 'ArrowLeft') next = (active + tabs.length - 1) % tabs.length;
                  else if (event.key === 'Home') next = 0;
                  else if (event.key === 'End') next = tabs.length - 1;
                  else if (event.key === 'Delete' && active >= 0) { event.preventDefault(); if (event.repeat) return; close(Number(tabs[active].dataset.previewTab)); return; }
                  else return;
                  event.preventDefault(); focusTab(tabs[next]);
                }}>
                  {workspace.tabs.filter(tab => tab.pane === pane.id).map(tab => <div className={css.tabItem} key={tab.id} data-preview-occurrence={tab.id}>
                    <button type="button" className={css.tab} id={`preview-tab-${tab.id}`} data-preview-tab={tab.id} role="tab" aria-selected={tab.id === pane.selected} aria-controls={`preview-body-${pane.id}`} tabIndex={tab.id === pane.selected ? 0 : -1} title={tab.artifact.name} onClick={() => owner.selectTab(tab.id)}>{tab.artifact.name}</button>
                    <button type="button" className={css.close} aria-label={tx('artifacts:workspace.close-tab', { p0: tab.artifact.name })} title={tx('artifacts:workspace.close-tab', { p0: tab.artifact.name })} onClick={() => close(tab.id)}>×</button>
                  </div>)}
                </div>
                {workspace.panes.length === 2 && <Button size="sm" className={css.move} aria-label={tx('artifacts:workspace.move')} title={tx('artifacts:workspace.move')} onClick={() => { pendingFocus.current = pane.selected; owner.move(pane.selected); }}>⇄</Button>}
              </div>
              <div className={css.body} id={`preview-body-${pane.id}`} role="tabpanel" aria-labelledby={`preview-tab-${pane.selected}`}>
                {shown && snapshot.leases.get(selected.id) && <ArtifactPreview key={selected.id} artifact={selected.artifact} resources={snapshot.leases.get(selected.id)!} viewState={selected.view} onViewStateChange={patch => owner.updateView(selected.id, patch)} onDownload={() => void owner.download(selected.artifact)} />}
              </div>
            </section>
          </div>;
        })}
      </div>
    </>}
  </div>;
}

function Divider({ width, ratio, commit, retireFocus }: { width: number; ratio: number; commit: (ratio: number) => void; retireFocus: () => void }) {
  const tx = useTranslation(), element = useRef<HTMLDivElement>(null);
  const gesture = useRef<{ pointer: number; x: number; base: number; latest: number; target: HTMLDivElement; group: HTMLElement; previous: HTMLElement; frame?: number } | undefined>(undefined);
  const bounds = splitBounds(width), effective = clampSplit(ratio, width);
  const [draft, setDraft] = useState<number>();
  const resting = useRef(effective); resting.current = effective;
  const focusReturn = useRef(retireFocus); focusReturn.current = retireFocus;
  useLayoutEffect(() => {
    const focusedControl = element.current;
    return () => { if (document.activeElement === focusedControl) focusReturn.current(); };
  }, []);
  const paint = (next?: number) => {
    setDraft(next);
    const group = element.current?.parentElement, previous = group?.previousElementSibling as HTMLElement | null;
    if (group && previous) { previous.style.flexGrow = String(next ?? effective); group.style.flexGrow = String(1 - (next ?? effective)); }
  };
  const retire = () => {
    const active = gesture.current; if (!active) return;
    gesture.current = undefined;
    if (active.frame !== undefined) cancelAnimationFrame(active.frame);
    active.previous.style.flexGrow = String(resting.current); active.group.style.flexGrow = String(1 - resting.current);
    if (active.target.hasPointerCapture(active.pointer)) active.target.releasePointerCapture(active.pointer);
    return active;
  };
  const finish = (accept: boolean) => { const active = retire(); if (!active) return; setDraft(undefined); if (accept) commit(active.latest); };
  // A changed coordinate system cancels the gesture rather than reinterpreting
  // its old pointer origin; teardown explicitly releases capture and RAF too.
  useLayoutEffect(() => { setDraft(undefined); return () => { retire(); }; }, [width]);
  return <div ref={element} className={css.divider} role="separator" tabIndex={0} aria-label={tx('artifacts:workspace.resize')} aria-orientation="vertical" aria-valuemin={Math.round(bounds.min * 100)} aria-valuemax={Math.round(bounds.max * 100)} aria-valuenow={Math.round((draft ?? effective) * 100)}
    onPointerDown={event => { if (event.button !== 0 || gesture.current) return; event.preventDefault(); event.currentTarget.focus({ preventScroll: true }); event.currentTarget.setPointerCapture(event.pointerId); gesture.current = { pointer: event.pointerId, x: event.clientX, base: effective, latest: effective, target: event.currentTarget, group: event.currentTarget.parentElement!, previous: event.currentTarget.parentElement!.previousElementSibling as HTMLElement }; }}
    onPointerMove={event => { const active = gesture.current; if (!active || active.pointer !== event.pointerId) return; active.latest = clampSplit(active.base + (event.clientX - active.x) / (width - PREVIEW_POLICY.dividerWidth), width); active.frame ??= requestAnimationFrame(() => { active.frame = undefined; if (gesture.current === active) paint(active.latest); }); }}
    onPointerUp={event => { const active = gesture.current; if (!active || active.pointer !== event.pointerId) return; active.latest = clampSplit(active.base + (event.clientX - active.x) / (width - PREVIEW_POLICY.dividerWidth), width); finish(true); }}
    onPointerCancel={event => { if (gesture.current?.pointer === event.pointerId) finish(false); }} onLostPointerCapture={event => { if (gesture.current?.pointer === event.pointerId) finish(false); }}
    onKeyDown={event => { if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229 || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
      if (event.key === 'Escape' && gesture.current) { event.preventDefault(); event.stopPropagation(); if (!event.repeat) finish(false); return; }
      if (event.key === 'ArrowLeft' || event.key === 'ArrowRight' || event.key === 'Home' || event.key === 'End') { event.preventDefault(); if (gesture.current) return; commit(event.key === 'Home' ? bounds.min : event.key === 'End' ? bounds.max : clampSplit(effective + (event.key === 'ArrowLeft' ? -.05 : .05), width)); }
    }} />;
}
