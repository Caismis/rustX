/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-session-panel SessionPanel.tsx; see PROVENANCE.md. */
import { useCallback, useEffect, useId, useMemo, useRef, useSyncExternalStore, type FocusEvent } from 'react';
import type { AppServerClient } from '../../client/app-server';
import { useTranslation } from '../../locale/react';
import { observeComposition } from '../../presentation/primitives/keyboard-composition';
import { BackgroundJobs } from './BackgroundJobs';
import { panelElementId, presentationOf, toggleElementId, useSessionPanelState } from './session-panel-state';
import css from './SessionPanel.module.css';

function isBehindModal(panel: HTMLElement | null): boolean {
  const modal = document.querySelector('[role="dialog"][aria-modal="true"]');
  if (modal === null) return false;
  return panel === null || !modal.contains(panel);
}

function useOverlayDismissal(
  panel: { readonly current: HTMLElement | null },
  open: boolean,
  toggleId: string,
  close: () => void,
): void {
  useEffect(() => {
    if (!open) return;
    const composition = observeComposition(document);
    const inside = (target: EventTarget | null): boolean =>
      target instanceof Element
      && (panel.current?.contains(target) === true
        || document.getElementById(toggleId)?.contains(target) === true
        || target.closest('[role="menu"]') !== null
        || target.closest('[data-session-panel-details]') !== null);
    const onPointerDown = (event: PointerEvent): void => {
      if (!inside(event.target)) close();
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape' || composition.guards(event) || event.defaultPrevented || isBehindModal(panel.current)) return;
      event.preventDefault();
      const focused = panel.current?.contains(document.activeElement) === true;
      close();
      if (focused) document.getElementById(toggleId)?.focus();
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
      composition.dispose();
    };
  }, [panel, open, toggleId, close]);
}

function useFocusReturn(hidden: boolean, toggleId: string): {
  readonly onFocus: () => void;
  readonly onBlur: (event: FocusEvent<HTMLElement>) => void;
} {
  const inside = useRef(false);
  useEffect(() => {
    if (!hidden || !inside.current) return;
    inside.current = false;
    document.getElementById(toggleId)?.focus({ preventScroll: true });
  }, [hidden, toggleId]);
  return useMemo(() => ({
    onFocus: () => { inside.current = true; },
    onBlur: (event) => {
      if (event.relatedTarget instanceof Element && !event.currentTarget.contains(event.relatedTarget)) inside.current = false;
    },
  }), []);
}

export function SessionPanel({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const tx = useTranslation();
  const state = useSessionPanelState();
  const probeId = useId();
  const probeRef = useMemo(() => state.createProbeRef(sessionId, probeId), [state, sessionId, probeId]);
  const probes = useSyncExternalStore(state.probes.subscribe, state.probes.getSnapshot);
  const visibility = useSyncExternalStore(state.visibility.subscribe, state.visibility.getSnapshot);
  const fits = probes.get(probeId) === true;
  const presentation = presentationOf(fits, visibility, sessionId);

  const panel = useRef<HTMLElement>(null);
  const dismissOverlay = useCallback(() => { state.closeOverlay(sessionId); }, [state, sessionId]);
  const shown = presentation !== 'hidden';
  const toggleId = toggleElementId(sessionId);
  useOverlayDismissal(panel, presentation === 'overlay', toggleId, dismissOverlay);
  const focus = useFocusReturn(!shown, toggleId);

  const sectionTitleId = `${panelElementId(sessionId)}-background-jobs`;

  return (
    <div className={css.asideSlot}>
      <span ref={probeRef} className={css.dockProbe} data-session-panel-probe="" />
      {shown && (
        <aside
          id={panelElementId(sessionId)}
          className={css.panel}
          aria-label={tx('common:session-panel.name')}
          ref={panel}
          onFocus={focus.onFocus}
          onBlur={focus.onBlur}
          data-session-panel={presentation}
        >
          <section className={css.section} aria-labelledby={sectionTitleId} data-session-panel-section="background-jobs">
            <div className={css.head}>
              <h2 id={sectionTitleId} className={css.title}>{tx('common:activity.background-jobs')}</h2>
            </div>
            <div className={css.body}>
              <div data-slot="">
                <BackgroundJobs client={client} sessionId={sessionId} />
              </div>
            </div>
          </section>
        </aside>
      )}
    </div>
  );
}
