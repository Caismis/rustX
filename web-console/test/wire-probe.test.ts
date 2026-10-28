// @vitest-environment node
import { describe, expect, it } from 'vitest';
import type { Page } from '@playwright/test';
import type { AttachmentTarget } from '../../protocol/app-server/v62';
import { checkpoint, snapshot } from './fixture';
import { wireProbe } from './e2e/wire-probe';

type Frame = { id?: string | number; method?: string; params?: object; result?: unknown; error?: unknown };
type Message = string | Buffer;
type Close = (code: number, reason: string) => void;
const target: AttachmentTarget = { session_id: 'A', conversation_id: 'conversation-A', runtime_incarnation: '1', attachment_id: 'attachment-A' };
const nativeCheckpoint = { type: 'checkpoint', checkpoint: checkpoint(snapshot()) };

/** Only the physical callback boundary is fake. It has no queue or admission
 * algorithm: every forward and every Native response is driven by the test. */
class FakeRoute {
  readonly nativeFrames: Frame[] = [];
  readonly browserFrames: Frame[] = [];
  readonly nativeCloses: object[] = [];
  readonly browserCloses: object[] = [];
  private browserMessage!: (message: Message) => void;
  private nativeMessage!: (message: Message) => void;
  private browserClose!: Close;
  private nativeClose!: Close;
  connectToServer() {
    return {
      send: (message: Message) => { this.nativeFrames.push(JSON.parse(String(message))); },
      onMessage: (receive: (message: Message) => void) => { this.nativeMessage = receive; },
      onClose: (receive: Close) => { this.nativeClose = receive; },
      close: (options = {}) => { this.nativeCloses.push(options); },
    };
  }
  send(message: Message) { this.browserFrames.push(JSON.parse(String(message))); }
  onMessage(receive: (message: Message) => void) { this.browserMessage = receive; }
  onClose(receive: Close) { this.browserClose = receive; }
  close(options = {}) { this.browserCloses.push(options); }
  browser(id: string, method = 'session/checkpoint', params: object = { target }) {
    this.browserMessage(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
  }
  reply(id: string | number, result: unknown = nativeCheckpoint) {
    this.nativeMessage(JSON.stringify({ jsonrpc: '2.0', id, result }));
  }
  fail(id: string | number) {
    this.nativeMessage(JSON.stringify({ jsonrpc: '2.0', id, error: { code: -32000, message: 'Operation rejected', data: { kind: 'request_capacity' } } }));
  }
  retire(side: 'browser' | 'native') {
    (side === 'browser' ? this.browserClose : this.nativeClose)(1011, 'controlled transport retirement');
  }
  ids() { return this.nativeFrames.map(frame => frame.id); }
}

async function harness() {
  let accept!: (route: FakeRoute) => void;
  const page = { routeWebSocket: async (_url: RegExp, receive: typeof accept) => { accept = receive; } };
  const wire = await wireProbe(page as unknown as Page);
  const connect = () => { const route = new FakeRoute(); accept(route); return route; };
  return { wire, connect, route: connect() };
}
const browserBatch = (route: FakeRoute, count = 6) => {
  for (let n = 1; n <= count; n++) route.browser(`browser-${n}`);
};

describe('acceptance probe shares only its added physical occupancy', () => {
  it('waits behind six browser requests, then dispatches exactly once after a real response', async () => {
    const { wire, route } = await harness();
    browserBatch(route);
    const read = wire.read('session/checkpoint', target);
    expect(route.ids()).toEqual(Array.from({ length: 6 }, (_, n) => `browser-${n + 1}`));
    route.reply('browser-1');
    expect(route.ids().at(-1)).toBe('acceptance-read-1');
    route.reply('acceptance-read-1');
    expect(await read).toEqual(nativeCheckpoint);
    expect(route.ids().filter(id => id === 'acceptance-read-1')).toHaveLength(1);
    expect(route.browserFrames.map(frame => frame.id)).toEqual(['browser-1']);
    expect(wire.requests).toHaveLength(6);
    expect(wire.responses).toHaveLength(1);
  });

  it('holds only the browser sixth position borrowed by a probe and repays it before a queued probe', async () => {
    const { wire, route } = await harness();
    const first = wire.read('session/checkpoint', target);
    const second = wire.read('session/checkpoint', target);
    browserBatch(route);
    expect(route.ids()).toEqual(['acceptance-read-1', 'browser-1', 'browser-2', 'browser-3', 'browser-4', 'browser-5']);
    route.reply('acceptance-read-1');
    expect(await first).toEqual(nativeCheckpoint);
    expect(route.ids().at(-1)).toBe('browser-6');
    expect(route.ids()).not.toContain('acceptance-read-2');
    route.reply('browser-1');
    expect(route.ids().at(-1)).toBe('acceptance-read-2');
    route.reply('acceptance-read-2');
    expect(await second).toEqual(nativeCheckpoint);
    expect(route.ids().filter(id => id === 'browser-6')).toHaveLength(1);
  });

  it('forwards a browser seventh ordinary request unchanged when no probe exists', async () => {
    const { wire, route } = await harness();
    browserBatch(route, 7);
    expect(route.ids()).toEqual(Array.from({ length: 7 }, (_, n) => `browser-${n + 1}`));
    route.fail('browser-7');
    expect(route.browserFrames.at(-1)).toMatchObject({ id: 'browser-7', error: { data: { kind: 'request_capacity' } } });
    expect(wire.requests).toHaveLength(7);
    expect(route.ids().filter(id => id === 'browser-7')).toHaveLength(1);
  });

  it('still exposes a browser-owned seventh request and Native refusal while a probe occupies a slot', async () => {
    const { wire, route } = await harness();
    const read = wire.read('session/checkpoint', target);
    browserBatch(route, 7);
    expect(route.ids()).toEqual(['acceptance-read-1', 'browser-1', 'browser-2', 'browser-3', 'browser-4', 'browser-5', 'browser-7']);
    route.fail('browser-7');
    expect(route.browserFrames.at(-1)).toMatchObject({ id: 'browser-7', error: { data: { kind: 'request_capacity' } } });
    expect(route.ids()).not.toContain('browser-6');
    route.reply('acceptance-read-1');
    await read;
    expect(route.ids().at(-1)).toBe('browser-6');
    expect(route.ids().filter(id => id === 'browser-7')).toHaveLength(1);
  });

  it.each([
    ['control', 'session/submissionIntent', { session_id: 'A', node_id: 'node-A' }],
    ['control cancellation', 'turn/cancel', { target }],
    ['attachment', 'session/attach', { session_id: 'A', node_id: 'node-A' }],
    ['metadata', 'session/settings', { session_id: 'A' }],
    ['observation', 'agent/observe', { target, agent_id: 'child-A', after: null }],
  ] as const)('forwards the independent %s lane while an artificial read and five ordinary calls are active', async (_lane, method, params) => {
    const { wire, route } = await harness();
    const read = wire.read('session/checkpoint', target);
    browserBatch(route);
    route.browser('independent', method, params);
    expect(route.nativeFrames.at(-1)).toMatchObject({ id: 'independent', method, params });
    expect(route.ids()).not.toContain('browser-6');
    route.reply('acceptance-read-1');
    await read;
    expect(route.ids().at(-1)).toBe('browser-6');
  });

  it.each(['future/ordinary', 'toString', '__proto__'])('leaves unknown browser method %s to Native instead of applying fixture admission to it', async method => {
    const { wire, route } = await harness();
    const read = wire.read('session/checkpoint', target);
    browserBatch(route);
    route.browser('unknown', method, { opaque: true });
    expect(route.nativeFrames.at(-1)).toMatchObject({ id: 'unknown', method, params: { opaque: true } });
    route.reply('acceptance-read-1'); await read;
  });

  it('releases physical occupancy on the raw Native response even while its browser acknowledgement is held', async () => {
    const { wire, route } = await harness();
    wire.holdNext('session/checkpoint');
    browserBatch(route);
    const read = wire.read('session/checkpoint', target);
    route.reply('browser-1');
    expect(wire.held()).toBe(1);
    expect(route.browserFrames).toHaveLength(0);
    expect(route.ids().at(-1)).toBe('acceptance-read-1');
    route.reply('acceptance-read-1'); await read;
    expect(wire.held()).toBe(1);
    wire.releaseHeld('session/checkpoint');
    expect(route.browserFrames.map(frame => frame.id)).toEqual(['browser-1']);
    expect(wire.held()).toBe(0);
  });

  it('bounds artificial work to six total reads and one active Native request', async () => {
    const { wire, route } = await harness();
    const reads = Array.from({ length: 6 }, () => wire.read('session/checkpoint', target));
    const complete = Promise.all(reads);
    await expect(wire.read('session/checkpoint', target)).rejects.toThrow(/fixture capacity/);
    expect(route.ids()).toEqual(['acceptance-read-1']);
    for (let n = 1; n <= 6; n++) {
      expect(route.ids()).toHaveLength(n);
      expect(route.ids().at(-1)).toBe(`acceptance-read-${n}`);
      route.reply(`acceptance-read-${n}`);
    }
    expect(await complete).toEqual(Array.from({ length: 6 }, () => nativeCheckpoint));
    expect(route.browserFrames).toHaveLength(0);
    const next = wire.read('session/checkpoint', target);
    expect(route.ids().at(-1)).toBe('acceptance-read-7');
    route.reply('acceptance-read-7'); await next;
  });

  it('propagates an actual probe refusal without retrying it and settles the next distinct owner', async () => {
    const { wire, route } = await harness();
    const first = wire.read('session/checkpoint', target);
    const rejection = expect(first).rejects.toThrow(/request_capacity/);
    const next = wire.read('session/checkpoint', target);
    route.fail('acceptance-read-1');
    await rejection;
    expect(route.ids()).toEqual(['acceptance-read-1', 'acceptance-read-2']);
    route.reply('acceptance-read-2');
    expect(await next).toEqual(nativeCheckpoint);
    expect(route.browserFrames).toHaveLength(0);
  });

  it.each(['browser', 'native'] as const)('retires active/queued artificial work and never forwards delayed work after %s closes', async side => {
    const { wire, route, connect } = await harness();
    const first = wire.read('session/checkpoint', target);
    const firstRejected = expect(first).rejects.toThrow(/transport retired/);
    const queued = wire.read('session/checkpoint', target);
    const queuedRejected = expect(queued).rejects.toThrow(/transport retired/);
    const attached = wire.attachedTarget('waiting');
    const attachedRejected = expect(attached).rejects.toThrow(/transport retired/);
    browserBatch(route);
    expect(route.ids()).not.toContain('browser-6');
    route.retire(side);
    await Promise.all([firstRejected, queuedRejected, attachedRejected]);
    const original = [...route.ids()];
    route.reply('acceptance-read-1');
    route.reply('browser-1');
    route.browser('late-browser');
    expect(route.ids()).toEqual(original);
    expect(route.browserFrames).toHaveLength(0);
    await expect(wire.read('session/checkpoint', target)).rejects.toThrow(/current browser transport/);
    const replacement = connect();
    const fresh = wire.read('session/checkpoint', target);
    expect(replacement.ids()).toEqual(['acceptance-read-3']);
    route.reply('acceptance-read-2');
    expect(replacement.ids()).toEqual(['acceptance-read-3']);
    replacement.reply('acceptance-read-3');
    expect(await fresh).toEqual(nativeCheckpoint);
  });

  it('cannot satisfy a replacement attachment with an old physical socket held acknowledgement', async () => {
    const { wire, route, connect } = await harness();
    const result = (value: AttachmentTarget) => ({ type: 'attached', target: value, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    wire.holdNext('session/attach');
    route.browser('old-attach', 'session/attach', { session_id: 'A', node_id: 'node-A' });
    const old = wire.attachedTarget('A');
    const rejected = expect(old).rejects.toThrow(/transport retired/);
    route.reply('old-attach', result(target));
    expect(wire.held()).toBe(1);
    route.retire('browser');
    await rejected;
    const replacement = connect();
    const attached = wire.attachedTarget('A');
    wire.releaseHeld('session/attach');
    expect(route.browserFrames).toHaveLength(0);
    expect(replacement.browserFrames).toHaveLength(0);
    const fresh = { ...target, attachment_id: 'replacement-attachment' };
    replacement.browser('new-attach', 'session/attach', { session_id: 'A', node_id: 'node-A' });
    route.reply('old-attach', result(target));
    replacement.reply('new-attach', result(fresh));
    expect(await attached).toEqual(fresh);
    expect(await wire.attachedTarget('A')).toEqual(fresh);
  });

  it('loses a committed acknowledgement by retiring before any queued probe is activated', async () => {
    const { wire, route } = await harness();
    browserBatch(route);
    const read = wire.read('session/checkpoint', target);
    const rejection = expect(read).rejects.toThrow(/transport retired/);
    wire.loseNext('session/checkpoint');
    route.reply('browser-1');
    await rejection;
    expect(wire.lost()).toBe(1);
    expect(route.ids()).toHaveLength(6);
    expect(route.browserFrames).toHaveLength(0);
    expect(route.browserCloses).toHaveLength(1);
    expect(route.nativeCloses).toHaveLength(1);
  });
});
