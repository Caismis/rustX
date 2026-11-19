/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-workspace recent-views.ts. */
import type { AttachmentTarget, ThreadStatusEntry } from '../../../../protocol/app-server/v82';
import type { ClientView, SessionView } from '../../client/contracts';

/** Inactive recent views kept besides the selected Session. Matches the Harness default. */
export const RECENT_INACTIVE_VIEW_LIMIT = 10;

export interface HeldView {
  generation: number;
  /** Absent while wanted intent has not acquired a controller. */
  target?: AttachmentTarget;
}

/** Facts the retention pass reads. Attention is running, waiting, or a pending interaction. */
export interface RecentViewSnapshot {
  generation: number;
  known: ReadonlySet<string>;
  deleting: ReadonlySet<string>;
  pinned: ReadonlySet<string>;
  attention: ReadonlySet<string>;
  held: ReadonlyMap<string, HeldView>;
  /** Session the shell is opening or showing. Never release it. */
  protectedId?: string;
}

/**
 * Attention-ordered retention of Sessions the main view has left.
 *
 * The state records only what the user did: the order Sessions left the main view, and the Session it shows now.
 * {@link RecentInactiveViews.sync} keeps every eligible Session that is running, awaiting the user, or holding an
 * unread completion, plus the first `limit` others. Pinned Sessions take that limit first. An evicted Session leaves
 * the order and returns only when the main view leaves it again. Release is attempted once per eviction and never
 * retried; a claim that no longer matches is left alone.
 */
export class RecentInactiveViews {
  /** Sessions the main view left that are still candidates, most recently left first. */
  private order: string[] = [];
  private current: string | undefined;
  private readonly unread = new Set<string>();
  private previousAttention = new Set<string>();
  private didRestore = false;
  private listener?: (evicted: readonly string[]) => void;

  constructor(
    private readonly read: () => RecentViewSnapshot,
    private readonly release: (id: string, claim: HeldView) => Promise<boolean>,
    private readonly limit = RECENT_INACTIVE_VIEW_LIMIT,
  ) {}

  /** Register the open-view list update. Does not subscribe to native state. */
  setListener(listener: ((evicted: readonly string[]) => void) | undefined): void {
    this.listener = listener;
  }

  /**
   * Record the Session the main view shows and sync. Call before its attach starts so a previous Session moves
   * here while its controller is still held.
   */
  show(sessionId: string | undefined): void {
    if (sessionId !== undefined) {
      this.unread.delete(sessionId);
      this.previousAttention.delete(sessionId);
    }
    const previous = this.current;
    if (previous !== undefined && previous !== sessionId) {
      this.order = [previous, ...this.order.filter(id => id !== previous)];
    }
    if (sessionId !== undefined) this.order = this.order.filter(id => id !== sessionId);
    this.current = sessionId;
    this.sync();
  }

  /**
   * Seed leave-order from saved views once. Oldest saved id is the least recently left. Does not attach.
   * @param ids - saved open views, oldest first.
   */
  restore(ids: readonly string[]): void {
    if (this.didRestore) return;
    this.didRestore = true;
    const seeded = [...ids].reverse().filter(id => id !== this.current && !this.order.includes(id));
    this.order = [...this.order, ...seeded];
    this.sync();
  }

  /** An explicit close owns release. Later syncs must not release that Session again. */
  forget(sessionId: string): void {
    this.order = this.order.filter(id => id !== sessionId);
    this.unread.delete(sessionId);
    this.previousAttention.delete(sessionId);
    if (this.current === sessionId) this.current = undefined;
  }

  /** Drop remembered order. In-flight releases are not repeated. */
  reset(): void {
    this.order = [];
    this.current = undefined;
    this.unread.clear();
    this.previousAttention.clear();
    this.didRestore = false;
  }

  /**
   * Hold exactly the Sessions the current state selects. A re-entrant call only records its request.
   * Controllers are released after the keep set is chosen, and only once.
   */
  sync(): void {
    if (!this.order.length) return;
    const snapshot = this.read();
    for (const id of this.previousAttention) {
      if (this.order.includes(id) && !snapshot.attention.has(id)) this.unread.add(id);
    }
    this.previousAttention = new Set(this.order.filter(id => snapshot.attention.has(id)));

    const eligible = this.order.filter(id => snapshot.known.has(id) && !snapshot.deleting.has(id));
    const ordinary = eligible.filter(id =>
      id !== snapshot.protectedId && !snapshot.attention.has(id) && !this.unread.has(id));
    const ranked = [...ordinary.filter(id => snapshot.pinned.has(id)), ...ordinary.filter(id => !snapshot.pinned.has(id))];
    const evicted = new Set(ranked.slice(this.limit));
    const keep = new Set(eligible.filter(id => !evicted.has(id)));
    const dropped = this.order.filter(id => !keep.has(id) && id !== snapshot.protectedId);

    // Commit the order before detaching. A synchronous lifecycle publication can re-enter sync.
    this.order = this.order.filter(id => keep.has(id) || id === snapshot.protectedId);
    if (!dropped.length) return;
    for (const id of dropped) {
      this.unread.delete(id);
      this.previousAttention.delete(id);
      const claim = snapshot.held.get(id);
      if (claim) void this.release(id, claim).catch(error => console.warn('Recent session detach failed', error));
    }
    this.listener?.(dropped);
  }

}

function activeStatus(entry: ThreadStatusEntry | null | undefined): boolean {
  return entry?.status.type === 'active';
}

function viewAttention(view: SessionView | undefined): boolean {
  if (!view) return false;
  if (activeStatus(view.threadStatus) || activeStatus(view.summary?.thread_status)) return true;
  const attempt = view.snapshot?.attempt?.phase.type;
  if (attempt === 'running' || attempt === 'admitted') return true;
  return (view.snapshot?.pending_interactions?.length ?? 0) > 0;
}

/** Whether this view still owns a controller the recent-view list may release. */
export function heldView(view: SessionView | undefined, generation: number): HeldView | undefined {
  if (!view || view.deleting) return;
  if (view.attachmentIntent !== 'wanted' && !view.target) return;
  return { generation, target: view.target };
}

/** Project catalog, thread status, pins, and held controllers into one retention snapshot. */
export function recentViewSnapshot(state: ClientView, pinned: ReadonlySet<string>, protectedId?: string): RecentViewSnapshot {
  const known = new Set(state.sessions.map(row => row.id));
  const attention = new Set<string>();
  const deleting = new Set<string>();
  const held = new Map<string, HeldView>();
  for (const row of state.sessions) if (activeStatus(row.thread_status)) attention.add(row.id);
  for (const view of Object.values(state.views)) {
    known.add(view.id);
    if (view.deleting) deleting.add(view.id);
    if (viewAttention(view)) attention.add(view.id);
    const transcript = view.snapshot?.transcript;
    if (transcript?.source_cut?.transcript_total === '0' && !transcript.entries?.length
      && !view.snapshot?.messages?.length && !view.inboundRequests && !view.sendingSubmissions?.length
      && !view.submissions?.length && !attention.has(view.id)) known.delete(view.id);
    if (state.connection === 'connected' && view.attachmentIntent === 'wanted' && view.attachment === 'detached' && !view.target && !view.passiveHistory && !view.error
      && !view.snapshot && !attention.has(view.id) && !view.inboundRequests
      && !view.sendingSubmissions?.length && !view.submissions?.length) known.delete(view.id);
    const claim = heldView(view, state.generation);
    if (claim) held.set(view.id, claim);
  }
  return { generation: state.generation, known, deleting, pinned, attention, held, protectedId };
}
