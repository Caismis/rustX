/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-session-panel visibility and element-ids; see PROVENANCE.md. */
import { createElement, createContext, useContext, useEffect, useState, type ReactNode } from 'react';

export const PINNED_STORAGE_KEY = 'dsh.sessionPanel.pinned';

export type SessionPanelPresentation = 'docked' | 'overlay' | 'hidden';

export interface SessionPanelVisibility {
  readonly pinned: boolean;
  readonly overlay: string | null;
}

export type SessionPanelDockable = ReadonlyMap<string, boolean>;
export type SessionPanelProbes = ReadonlyMap<string, boolean>;

export function panelElementId(sessionId: string): string {
  return `dsh-session-panel-${sessionId}`;
}

export function toggleElementId(sessionId: string): string {
  return `dsh-session-panel-toggle-${sessionId}`;
}

export function presentationOf(
  fits: boolean | undefined,
  visibility: SessionPanelVisibility,
  sessionId: string,
): SessionPanelPresentation {
  if (fits === undefined) return 'hidden';
  if (fits) return visibility.pinned ? 'docked' : 'hidden';
  return visibility.overlay === sessionId ? 'overlay' : 'hidden';
}

interface ObservableValue<T> {
  getSnapshot(): T;
  subscribe(listener: () => void): () => void;
}

function createObservable<T>(initial: T, equals: (a: T, b: T) => boolean = Object.is): ObservableValue<T> & { set(next: T): void } {
  let value = initial;
  const listeners = new Set<() => void>();
  return {
    getSnapshot: () => value,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    set: (next) => {
      if (equals(value, next)) return;
      value = next;
      for (const listener of [...listeners]) listener();
    },
  };
}

function readPinnedPreference(): boolean {
  try {
    return typeof localStorage !== 'undefined' && localStorage.getItem(PINNED_STORAGE_KEY) !== 'false';
  } catch {
    return true;
  }
}

function writePinnedPreference(pinned: boolean): void {
  try {
    if (typeof localStorage !== 'undefined') localStorage.setItem(PINNED_STORAGE_KEY, String(pinned));
  } catch {
    // Ignore storage quota / privacy mode failures.
  }
}

function sameBoolMap(a: ReadonlyMap<string, boolean>, b: ReadonlyMap<string, boolean>): boolean {
  if (a.size !== b.size) return false;
  for (const [key, value] of a) {
    if (b.get(key) !== value) return false;
  }
  return true;
}

export class SessionPanelState {
  private readonly pinned: ReturnType<typeof createObservable<boolean>>;
  private readonly overlay = createObservable<string | null>(null);
  private readonly probeValues = new Map<string, boolean>();
  private readonly probeOwners = new Map<string, string>();
  private readonly detachers = new Set<() => void>();
  private unfollow: () => void = () => {};

  readonly fits = createObservable<SessionPanelProbes>(new Map(), sameBoolMap);
  readonly room = createObservable<SessionPanelDockable>(new Map(), sameBoolMap);

  readonly visibility: ObservableValue<SessionPanelVisibility> = {
    getSnapshot: () => this.currentVisibility(),
    subscribe: (listener) => {
      const offPinned = this.pinned.subscribe(listener);
      const offOverlay = this.overlay.subscribe(listener);
      return () => {
        offPinned();
        offOverlay();
      };
    },
  };

  readonly dockable: ObservableValue<SessionPanelDockable> = this.room;
  readonly probes: ObservableValue<SessionPanelProbes> = this.fits;

  private cachedVisibility: SessionPanelVisibility;

  constructor(initialPinned: boolean = readPinnedPreference()) {
    this.pinned = createObservable(initialPinned);
    this.cachedVisibility = { pinned: initialPinned, overlay: null };
    this.pinned.subscribe((/* value ignored; rebuild cached snapshot */) => {
      writePinnedPreference(this.pinned.getSnapshot());
      this.cachedVisibility = { pinned: this.pinned.getSnapshot(), overlay: this.overlay.getSnapshot() };
    });
    this.overlay.subscribe(() => {
      this.cachedVisibility = { pinned: this.pinned.getSnapshot(), overlay: this.overlay.getSnapshot() };
    });
  }

  private currentVisibility(): SessionPanelVisibility {
    return this.cachedVisibility;
  }

  createProbeRef(sessionId: string, probe: string): (element: HTMLElement | null) => void {
    let detach: (() => void) | undefined;
    return (element) => {
      detach?.();
      detach = element === null ? undefined : this.attach(sessionId, probe, element);
    };
  }

  toggle(sessionId: string): void {
    const dockable = this.room.getSnapshot().get(sessionId);
    if (dockable === undefined) return;
    if (dockable) {
      this.pinned.set(!this.pinned.getSnapshot());
      this.setOverlay(null);
      return;
    }
    this.setOverlay(this.overlay.getSnapshot() === sessionId ? null : sessionId);
  }

  closeOverlay(sessionId: string): void {
    if (this.overlay.getSnapshot() === sessionId) this.setOverlay(null);
  }

  dispose(): void {
    this.stopFollowing();
    for (const detach of [...this.detachers]) detach();
  }

  private stopFollowing(): void {
    this.unfollow();
    this.unfollow = () => {};
  }

  private setOverlay(sessionId: string | null): void {
    this.stopFollowing();
    this.overlay.set(sessionId);
    if (sessionId === null) return;
    const check = (): void => {
      if (this.room.getSnapshot().get(sessionId) !== false) this.closeOverlay(sessionId);
    };
    this.unfollow = this.room.subscribe(check);
  }

  private attach(sessionId: string, probe: string, element: HTMLElement): () => void {
    this.report(sessionId, probe, false);
    const measure: ResizeObserverCallback = (entries) => {
      for (const entry of entries) this.report(sessionId, probe, entry.contentRect.width >= 1);
    };
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(measure);
    observer?.observe(element);
    const detach = (): void => {
      if (!this.detachers.delete(detach)) return;
      observer?.disconnect();
      this.withdraw(probe);
    };
    this.detachers.add(detach);
    return detach;
  }

  private report(sessionId: string, probe: string, value: boolean): void {
    this.probeOwners.set(probe, sessionId);
    this.probeValues.set(probe, value);
    this.syncSnapshots();
  }

  private withdraw(probe: string): void {
    this.probeOwners.delete(probe);
    this.probeValues.delete(probe);
    this.syncSnapshots();
  }

  private syncSnapshots(): void {
    this.fits.set(new Map(this.probeValues));
    const sessions = new Map<string, boolean>();
    for (const [probe, value] of this.probeValues) {
      const owner = this.probeOwners.get(probe);
      if (owner !== undefined) sessions.set(owner, (sessions.get(owner) ?? false) || value);
    }
    this.room.set(sessions);
  }
}

const fallbackState = new SessionPanelState();
const SessionPanelContext = createContext<SessionPanelState>(fallbackState);

export function SessionPanelProvider({ children }: { children: ReactNode }) {
  const [state] = useState(() => new SessionPanelState());
  useEffect(() => () => state.dispose(), [state]);
  return createElement(SessionPanelContext.Provider, { value: state }, children);
}

export function useSessionPanelState(): SessionPanelState {
  return useContext(SessionPanelContext);
}
