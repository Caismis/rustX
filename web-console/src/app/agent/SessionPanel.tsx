/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-session-panel SessionPanel.tsx; see PROVENANCE.md. */
import { useCallback, useEffect, useId, useMemo, useRef, useSyncExternalStore, type FocusEvent } from 'react';
import type { AppServerClient } from '../../client/app-server';
import { useTranslation } from '../../locale/react';
import { observeComposition } from '../../presentation/primitives/keyboard-composition';
import { BackgroundJobs } from './BackgroundJobs';
import { panelElementId, presentationOf, toggleElementId, useSessionPanelState } from './session-panel-state';

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
    <div className="absolute inset-x-0 top-0 bottom-[var(--dsh-composer-height,0px)] z-[9] pointer-events-none [--dsh-chat-aside-gutter:max(0px,calc((100%-var(--dsh-chat-content-width,100%))/2))]">
      <span ref={probeRef} className="[--panel-clear:40px] [--panel-edge:8px] [--panel-min-width:300px] absolute top-0 right-0 w-[calc(var(--dsh-chat-aside-gutter,0px)-var(--panel-clear)-var(--panel-edge)-var(--panel-min-width)+1px)] max-w-[1px] h-0 invisible pointer-events-none" data-session-panel-probe="" />
      {shown && (
        <aside
          id={panelElementId(sessionId)}
          className="sp-panel [--panel-clear:40px] [--panel-edge:8px] [--panel-min-width:300px] absolute top-[12px] right-[var(--panel-edge)] box-border w-[clamp(var(--panel-min-width),calc(var(--dsh-chat-aside-gutter,0px)-var(--panel-clear)-var(--panel-edge)),360px)] max-h-[max(0px,calc(100%-24px))] overflow-y-auto overscroll-contain [scrollbar-gutter:stable] [--dsh-scrollbar-thumb:var(--dsw-alias-scrollbar-bg-l2)] [--dsh-scrollbar-thumb-hover:var(--dsw-alias-scrollbar-hover-l2)] [--dsh-scrollbar-track-margin:12px] py-[6px] pr-[calc(8px-2px-var(--dsh-scrollbar-width,5px))] pl-[8px] border-0 [border-right:2px_solid_transparent] rounded-[var(--dsw-radius-lg)] [color:var(--dsw-alias-label-primary)] bg-[var(--dsw-alias-bg-layer-1)] shadow-[var(--dsw-elevation-panel)] pointer-events-auto animate-[dsh-session-panel-enter_120ms_ease-out] motion-reduce:animate-none data-[session-panel=overlay]:w-[min(280px,calc(100%-2*var(--panel-edge)))] data-[session-panel=overlay]:shadow-[var(--dsw-elevation-prominent)] not-has-[>.sp-section>.sp-body>[data-slot]:not(:empty)]:hidden [&_[data-session-panel-row]]:relative [&_[data-session-panel-row]]:flex [&_[data-session-panel-row]]:items-center [&_[data-session-panel-row]]:gap-[8px] [&_[data-session-panel-row]]:box-border [&_[data-session-panel-row]]:min-w-0 [&_[data-session-panel-row]]:min-h-[28px] [&_[data-session-panel-row]]:px-[6px] [&_[data-session-panel-row]]:rounded-[var(--dsw-radius-sm)] [&_[data-session-panel-row]]:[color:var(--dsw-alias-label-primary)] [&_[data-session-panel-row]]:[font:var(--dsw-font-xs-13)] [&_[data-session-panel-row]>[data-row-part=body]]:flex [&_[data-session-panel-row]>[data-row-part=body]]:flex-auto [&_[data-session-panel-row]>[data-row-part=body]]:items-center [&_[data-session-panel-row]>[data-row-part=body]]:gap-[8px] [&_[data-session-panel-row]>[data-row-part=body]]:min-w-0 [&_[data-session-panel-row]>[data-row-part=body]]:h-[28px] [&_[data-session-panel-row]>button[data-row-part=body]]:m-0 [&_[data-session-panel-row]>button[data-row-part=body]]:p-0 [&_[data-session-panel-row]>button[data-row-part=body]]:border-0 [&_[data-session-panel-row]>button[data-row-part=body]]:[color:inherit] [&_[data-session-panel-row]>button[data-row-part=body]]:bg-transparent [&_[data-session-panel-row]>button[data-row-part=body]]:[font:inherit] [&_[data-session-panel-row]>button[data-row-part=body]]:text-start [&_[data-session-panel-row]>button[data-row-part=body]]:cursor-pointer [&_[data-session-panel-row]>button[data-row-part=body]]:after:absolute [&_[data-session-panel-row]>button[data-row-part=body]]:after:inset-0 [&_[data-session-panel-row]>button[data-row-part=body]]:after:rounded-[inherit] [&_[data-session-panel-row]>button[data-row-part=body]]:after:content-[''] [&_[data-session-panel-row]>button[data-row-part=body]:focus-visible]:outline-none [&_[data-session-panel-row]:has(>button[data-row-part=body]:focus-visible)]:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] [&_[data-session-panel-row]:has(>button[data-row-part=body]:focus-visible)]:-outline-offset-2 [&_[data-session-panel-row]_[data-row-part=icon]]:inline-flex [&_[data-session-panel-row]_[data-row-part=icon]]:flex-none [&_[data-session-panel-row]_[data-row-part=icon]]:items-center [&_[data-session-panel-row]_[data-row-part=icon]]:justify-center [&_[data-session-panel-row]_[data-row-part=icon]]:w-[16px] [&_[data-session-panel-row]_[data-row-part=icon]]:h-[16px] [&_[data-session-panel-row]_[data-row-part=icon]_svg]:w-[16px] [&_[data-session-panel-row]_[data-row-part=icon]_svg]:h-[16px] [&_[data-session-panel-row]_[data-row-part=content]]:flex-auto [&_[data-session-panel-row]_[data-row-part=content]]:min-w-0 [&_[data-session-panel-row]_[data-row-part=content]]:overflow-hidden [&_[data-session-panel-row]_[data-row-part=content]]:text-ellipsis [&_[data-session-panel-row]_[data-row-part=content]]:whitespace-nowrap [&_[data-session-panel-row]:has(>button[data-row-part=body]:active)]:bg-[var(--dsw-alias-interactive-bg-active)] [&_[data-session-panel-row]:has(>button[data-row-part=body]):hover]:bg-[var(--dsw-alias-interactive-bg-hover)] [&_[data-session-panel-row=fold]]:[color:var(--dsw-alias-label-tertiary)] [&_[data-session-panel-row=fold]]:[font:var(--dsw-font-xxs-12)] [&_[data-session-panel-row=fold]]:tabular-nums [&_[data-session-panel-row=fold]_[data-row-part=icon]_svg]:w-[14px] [&_[data-session-panel-row=fold]_[data-row-part=icon]_svg]:h-[14px] [&_[data-session-panel-row=fold]:has(>button:focus-visible)]:[color:var(--dsw-alias-label-secondary)] [&_[data-session-panel-row=fold]:hover]:[color:var(--dsw-alias-label-secondary)]"
          aria-label={tx('common:session-panel.name')}
          ref={panel}
          onFocus={focus.onFocus}
          onBlur={focus.onBlur}
          data-session-panel={presentation}
        >
          <section className="sp-section pt-[4px] pb-[6px] has-[>.sp-body>[data-slot]:empty]:hidden has-[>.sp-body>[data-slot]>[data-slot]:only-child:empty]:hidden [[.sp-section:not(:has(>.sp-body>[data-slot]:empty)):not(:has(>.sp-body>[data-slot]>[data-slot]:only-child:empty))~&]:not-has-[>.sp-body>[data-slot]:empty]:not-has-[>.sp-body>[data-slot]>[data-slot]:only-child:empty]:mt-[2px] [[.sp-section:not(:has(>.sp-body>[data-slot]:empty)):not(:has(>.sp-body>[data-slot]>[data-slot]:only-child:empty))~&]:not-has-[>.sp-body>[data-slot]:empty]:not-has-[>.sp-body>[data-slot]>[data-slot]:only-child:empty]:pt-[8px] [[.sp-section:not(:has(>.sp-body>[data-slot]:empty)):not(:has(>.sp-body>[data-slot]>[data-slot]:only-child:empty))~&]:not-has-[>.sp-body>[data-slot]:empty]:not-has-[>.sp-body>[data-slot]>[data-slot]:only-child:empty]:[border-top:0.5px_solid_var(--dsw-alias-border-l2)]" aria-labelledby={sectionTitleId} data-session-panel-section="background-jobs">
            <div className="flex items-center gap-[8px] min-h-[24px] px-[6px]">
              <h2 id={sectionTitleId} className="flex-[0_1_auto] min-w-0 m-0 overflow-hidden [color:var(--dsw-alias-label-secondary)] [font:var(--dsw-font-xxs-12)] font-medium text-ellipsis whitespace-nowrap">{tx('common:activity.background-jobs')}</h2>
            </div>
            <div className="sp-body">
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
