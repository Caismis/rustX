import { describe, expect, it, vi } from 'vitest';
import type { AttachmentTarget } from '../../protocol/app-server/v82';
import { RecentInactiveViews, RECENT_INACTIVE_VIEW_LIMIT, recentViewSnapshot, type HeldView, type RecentViewSnapshot } from '../src/app/navigation/recent-views';
import type { ClientView } from '../src/client/contracts';

const target = (id: string): AttachmentTarget => ({ session_id: id, conversation_id: `conversation-${id}`, runtime_incarnation: '1', attachment_id: `attachment-${id}` });

function bench(ids: readonly string[], limit = RECENT_INACTIVE_VIEW_LIMIT) {
  const known = new Set(ids);
  const attention = new Set<string>();
  const pinned = new Set<string>();
  const deleting = new Set<string>();
  const held = new Map<string, HeldView>();
  let generation = 1;
  let protectedId: string | undefined;
  let fail: string | undefined;
  const releases: string[] = [];
  const dropped: string[] = [];
  const released = new Set<string>();
  const recent = new RecentInactiveViews(() => ({
    generation, known, deleting, pinned, attention, protectedId,
    held: new Map([...held].filter(([id]) => !released.has(id))),
  } satisfies RecentViewSnapshot), async (id, claim) => {
    if (fail === id) throw new Error('release failed');
    const current = held.get(id);
    if (claim.generation !== generation || !current || (claim.target && current.target !== claim.target)) return false;
    released.add(id);
    releases.push(id);
    recent.sync();
    return true;
  }, limit);
  recent.setListener(ids => dropped.push(...ids));
  const hold = (id: string) => { held.set(id, { generation, target: target(id) }); };
  /** Show each Session in turn. Pass `last` to leave that one selected; otherwise leave none. */
  const visit = (left: readonly string[], last?: string) => {
    for (const id of left) { hold(id); recent.show(id); }
    if (last !== undefined) recent.show(last);
    else if (left.length) recent.show(undefined);
  };
  return {
    recent, attention, pinned, known, deleting, releases, dropped, released, hold,
    visit, setFail: (id?: string) => { fail = id; }, setGeneration: (value: number) => { generation = value; },
    setProtected: (id?: string) => { protectedId = id; },
  };
}

describe('recent inactive views', () => {
  it('keeps a left Session attached and does not detach it when the main view returns', () => {
    const b = bench(['a', 'b']);
    b.visit(['a'], 'b');
    expect(b.releases).toEqual([]);
    expect(b.released.has('a')).toBe(false);
    b.recent.show('a');
    expect(b.releases).toEqual([]);
  });

  it('keeps ten idle Sessions beyond a running one and detaches the least recently left idle Session', () => {
    const ids = Array.from({ length: 13 }, (_, index) => `s${index}`);
    const b = bench(ids);
    b.attention.add('s0');
    b.visit(ids.slice(0, 12), 's11');
    expect(b.releases).toEqual([]);
    b.visit(['s12'], 's12');
    expect(b.releases).toEqual(['s1']);
    expect(b.released.has('s0')).toBe(false);
    b.attention.delete('s0');
    b.recent.sync();
    expect(b.releases).toEqual(['s1']);
  });

  it('does not bring an evicted Session back when room frees', () => {
    const ids = Array.from({ length: 12 }, (_, index) => `e${index}`);
    const b = bench(ids);
    b.visit(ids.slice(0, 11));
    expect(b.releases).toEqual(['e0']);
    b.deleting.add('e5');
    b.recent.sync();
    expect(b.releases).toEqual(['e0', 'e5']);
  });

  it('protects an unread completion until the main view shows it, then counts it', () => {
    const b = bench(['a', 'b'], 0);
    b.attention.add('a');
    b.visit(['a'], 'b');
    expect(b.releases).toEqual([]);
    b.attention.delete('a');
    b.recent.sync();
    expect(b.releases).toEqual([]);
    b.recent.show('a');
    b.recent.show('b');
    expect(b.releases).toEqual(['a']);
  });

  it('detaches unpinned Sessions before pinned ones', () => {
    const ids = Array.from({ length: 12 }, (_, index) => `p${index}`);
    const b = bench(ids);
    b.pinned.add('p0');
    b.pinned.add('p1');
    b.visit(ids);
    expect([...b.releases].sort()).toEqual(['p2', 'p3']);
    expect(b.released.has('p0')).toBe(false);
    expect(b.released.has('p1')).toBe(false);
  });

  it('does not retry a failed or stale release', async () => {
    const b = bench(['a', 'b', 'c'], 0);
    b.hold('a');
    b.setFail('a');
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    b.recent.show('a');
    b.recent.show('b');
    await Promise.resolve();
    expect(warn).toHaveBeenCalledOnce();
    expect(b.releases).toEqual([]);
    b.setFail(undefined);
    b.recent.sync();
    await Promise.resolve();
    expect(b.releases).toEqual([]);
    b.hold('c');
    b.setGeneration(2);
    b.recent.show('c');
    b.recent.show(undefined);
    await Promise.resolve();
    expect(b.releases).toEqual([]);
    warn.mockRestore();
  });

  it('drops an unlisted saved view without a release and never releases the Session being opened', () => {
    const b = bench(['a']);
    b.recent.restore(['missing', 'a']);
    expect(b.dropped).toEqual(['missing']);
    expect(b.releases).toEqual([]);
    b.setProtected('a');
    b.hold('a');
    b.recent.show('b');
    b.recent.restore(['a']);
    expect(b.released.has('a')).toBe(false);
  });

  it('excludes unattached cold-detached views from retention while preserving errors and passive history', () => {
    const baseState: ClientView = {
      connection: 'connected',
      generation: 1,
      sessions: [],
      views: {
        aborted: { id: 'aborted', attachmentIntent: 'wanted', attachment: 'detached' },
        attaching: { id: 'attaching', attachmentIntent: 'wanted', attachment: 'attaching' },
        errored: { id: 'errored', attachmentIntent: 'wanted', attachment: 'error', error: 'Attach failed' },
        passive: { id: 'passive', attachmentIntent: 'wanted', attachment: 'detached', passiveHistory: true },
      },
      uncertain: [],
      interactionOperations: {},
    };
    const snap = recentViewSnapshot(baseState, new Set(), 'attaching');
    expect(snap.known.has('aborted')).toBe(false);
    expect(recentViewSnapshot({ ...baseState, connection: 'reconnecting' }, new Set()).known.has('aborted')).toBe(true);
    expect(snap.known.has('attaching')).toBe(true);
    expect(snap.known.has('errored')).toBe(true);
    expect(snap.known.has('passive')).toBe(true);
  });
});
