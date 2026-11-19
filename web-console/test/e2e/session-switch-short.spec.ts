import { test as base, expect } from '@playwright/test';
import type { Page } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { connectRemote } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { followRoot } from './source-reading';
import { wireProbe } from './wire-probe';

// Two durable one-turn histories are fixture preparation, not the measured
// path. Each Session owns exactly one user message and one assistant reply.
const MARKERS = {
  a: { prompt: 'Composer identity 0', reply: 'Canonical composer reply 0' },
  b: { prompt: 'Composer identity 1', reply: 'Canonical composer reply 1' },
} as const;

interface Seeded { fixture: Awaited<ReturnType<typeof startDogfood>>; a: string; b: string; pass: () => void }

const test = base.extend<{ seeded: Seeded }>({
  seeded: [async ({}, use) => {
    const fixture = await startDogfood('web_composer_identity', undefined, { imageMcp: true, handshakeSeconds: 1 });
    let passed = false;
    try {
      const remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
      const ids: string[] = [];
      const seeds: [string, string][] = [['Alpha', MARKERS.a.prompt], ['Beta', MARKERS.b.prompt]];
      for (const [label, prompt] of seeds) {
        const created = await remote.client.call('thread/start', { settings: { cwd: fixture.workspaceA } }, 'session_transition');
        const id = created.session.id; ids.push(id);
        await remote.renameSession(id, `Session ${label}`);
        const attached = await remote.client.call('thread/resume', { threadId: (await remote.readSession(id)).active_conversation_id }, 'attached');
        const following = await followRoot(remote, attached.target);
        const terminal = following.nextSettlement();
        await remote.client.call('inbound/queue', { inputId: crypto.randomUUID(), target: attached.target, content: [{ type: 'text', text: prompt }] }, 'inbound_accepted');
        await terminal;
        following.stop();
        await remote.client.call('session/detach', { target: attached.target }, 'detached');
      }
      await remote.shutdown();
      await fixture.restartApp(); // Cold native runtime as well as a cold browser attachment.
      await use({ fixture, a: ids[0], b: ids[1], pass: () => { passed = true; } });
    } finally { await fixture.stop(passed); }
  }, { timeout: 60_000 }],
});

interface LegSpec { name: string; sessionId: string; reply: string; prompt: string; otherReply: string; otherPrompt: string }
const LEG_TIMEOUT_MS = 6_000;

/** Per-rendered-frame sampler. Installed before app scripts on every load so
 * a mid-test reload keeps measuring. Records only observable DOM facts: scroll
 * geometry, visible transcript rows, skeleton/blank state, composer geometry
 * and editor incarnation/revision/selection, and sidebar selection timing. */
async function installSampler(page: Page) {
  await page.addInitScript(() => {
    const w = window as any;
    const diag: any = w.__diag = { leg: '', cfg: {} as any, legCfgs: {} as any, frames: [] as any[], clickT: 0 };
    document.addEventListener('click', (event) => {
      const button = (event.target as Element | null)?.closest?.('button[data-session-id]');
      if (button) diag.clickT = event.timeStamp || performance.now();
    }, true);
    const scrollKey = (f: any) => (f.scroll ? f.scroll.join(',') : '');
    const allKey = (f: any) => (f.scroll ? f.replyY + '|' + scrollKey(f) + '|' + (f.composer ? f.composer.slice(0, 4).join(',') : 'none') : '');
    const bodyOk = (f: any) => !!(f.reply && f.prompt && !f.other);
    const enabled = (f: any) => !!(f.composer && f.composer[9] === false && f.composer[10] === false);
    const loop = () => {
      const f: any = { t: performance.now(), leg: diag.leg, scroll: null, rows: -1, reply: false, prompt: false, other: false, skeleton: false, blank: false, composer: null, selected: null, title: '' };
      const port = document.querySelector<HTMLElement>('[data-conversation-scroll]');
      if (port) {
        const r = port.getBoundingClientRect();
        f.scroll = [+port.scrollTop.toFixed(2), port.scrollHeight, port.clientHeight, +r.x.toFixed(2), +r.y.toFixed(2), +r.width.toFixed(2), +r.height.toFixed(2)];
        const rows = Array.from(port.querySelectorAll<HTMLElement>('[data-chat-anchor-key]')).filter(el => !el.closest('[hidden]'));
        f.rows = rows.length;
        const rowText = rows.map(el => el.textContent ?? '').join('\n');
        const cfg = diag.cfg || {};
        f.reply = !!cfg.reply && rowText.includes(cfg.reply);
        f.prompt = !!cfg.prompt && rowText.includes(cfg.prompt);
        const replyRow = rows.find(el => cfg.reply && el.textContent?.includes(cfg.reply));
        f.replyY = replyRow?.getBoundingClientRect().top;
        f.other = (!!cfg.otherReply && rowText.includes(cfg.otherReply)) || (!!cfg.otherPrompt && rowText.includes(cfg.otherPrompt));
        f.skeleton = Array.from(port.querySelectorAll<HTMLElement>('[class*="skeleton"]')).some(el => { const b = el.getBoundingClientRect(); return b.width > 0 && b.height > 0; });
        f.blank = rows.length === 0 && !f.skeleton;
      }
      const card = (port ?? document).querySelector<HTMLElement>('[data-composer-card]');
      if (card) {
        const r = card.getBoundingClientRect();
        const ta = card.querySelector('textarea');
        f.composer = [+r.x.toFixed(2), +r.y.toFixed(2), +r.width.toFixed(2), +r.height.toFixed(2), card.getAttribute('data-editor-incarnation'), card.getAttribute('data-editor-revision'), ta ? ta.selectionStart : -1, ta ? ta.selectionEnd : -1, ta ? +ta.scrollTop.toFixed(2) : -1, ta ? !!ta.disabled : null, ta ? !!ta.readOnly : null];
      }
      f.selected = document.querySelector('button[data-session-id][aria-current="page"]')?.getAttribute('data-session-id') ?? null;
      f.title = document.title;
      f.live = document.querySelector('#session-view')?.getAttribute('data-shell-route') === 'session';
      diag.frames.push(f);
      requestAnimationFrame(loop);
    };
    requestAnimationFrame(loop);
    w.__diagSetLeg = (name: string, sessionId: string, cfg: any) => { diag.leg = name; diag.cfg = cfg; diag.legCfgs[name] = { sessionId, ...cfg }; diag.clickT = 0; };
    w.__diagProgress = (name: string) => {
      let streak = 0, prevS = '', prevA = '', n = 0, lastT = 0;
      for (const f of diag.frames) {
        if (f.leg !== name || !diag.clickT || f.t < diag.clickT) continue;
        n++; lastT = f.t;
        const ok = bodyOk(f);
        const s = ok ? scrollKey(f) : '', a = ok ? allKey(f) : '';
        streak = (s && s === prevS && a === prevA) ? streak + 1 : (ok ? 1 : 0);
        prevS = s; prevA = a;
        if (streak >= 5 && enabled(f) && f.live) return { n, done: true, settledAt: f.t };
      }
      return { n, done: false, lastT };
    };
    w.__diagCollect = (name: string) => {
      const cfg = diag.legCfgs[name] || {};
      const frames = diag.frames.filter((f: any) => f.leg === name && diag.clickT && f.t >= diag.clickT);
      const replyY = frames.filter(bodyOk).map((f: any) => f.replyY as number);
      let firstBodyT = -1, firstActionT = -1, stableScroll: any = null, stableAll: any = null, selectedT = -1;
      let blank = 0, stale = 0, skeleton = 0, maxGap = 0, prevT = 0;
      let s1 = 0, k1 = '', s2 = 0, k2 = '';
      const incarnations = new Set<string>(), revisions = new Set<string>(), selections = new Set<string>();
      frames.forEach((f: any, i: number) => {
        if (prevT) maxGap = Math.max(maxGap, f.t - prevT); prevT = f.t;
        const good = bodyOk(f);
        if (f.blank) blank++; if (f.other) stale++; if (f.skeleton) skeleton++;
        if (f.composer) {
          if (f.composer[4] != null) incarnations.add(String(f.composer[4]));
          if (f.composer[5] != null) revisions.add(String(f.composer[5]));
          if (f.composer[6] >= 0) selections.add(`${f.composer[6]}:${f.composer[7]}`);
        }
        if (firstBodyT < 0 && good) firstBodyT = f.t;
        if (firstActionT < 0 && good && enabled(f)) firstActionT = f.t;
        if (selectedT < 0 && f.selected === cfg.sessionId) selectedT = f.t;
        const a = good ? scrollKey(f) : '', b = good ? allKey(f) : '';
        s1 = (a && a === k1) ? s1 + 1 : (a ? 1 : 0); k1 = a;
        s2 = (b && b === k2) ? s2 + 1 : (b ? 1 : 0); k2 = b;
        if (!stableScroll && s1 >= 5) stableScroll = { start: frames[i - 4].t, end: f.t };
        if (!stableAll && s2 >= 5) stableAll = { start: frames[i - 4].t, end: f.t };
      });
      return {
        clickT: diag.clickT || null,
        firstBodyT: firstBodyT > 0 ? firstBodyT : null,
        firstActionT: firstActionT > 0 ? firstActionT : null,
        stableScroll, stableAll, selectedT: selectedT > 0 ? selectedT : null,
        counts: { bodyShiftPx: replyY.length ? Math.max(...replyY) - Math.min(...replyY) : null, frames: frames.length, blank, stale, skeleton, maxGapMs: +maxGap.toFixed(1) },
        editor: { incarnations: [...incarnations], revisions: [...revisions], selections: [...selections].slice(0, 10) },
        span: frames.length ? { first: frames[0].t, last: frames[frames.length - 1].t } : null,
        frames,
      };
    };
  });
}

async function diagSetLeg(page: Page, name: string, sessionId: string, cfg: Record<string, string>) {
  await page.evaluate(arg => (window as any).__diagSetLeg(arg.name, arg.sessionId, arg.cfg), { name, sessionId, cfg });
}
async function diagProgress(page: Page, name: string) {
  return page.evaluate(n => (window as any).__diagProgress(n), name) as Promise<{ n: number; done: boolean; lastT?: number; settledAt?: number }>;
}
async function diagCollect(page: Page, name: string) {
  return page.evaluate(n => (window as any).__diagCollect(n), name) as Promise<any>;
}

/** One measured switch: real click on the sidebar row, then per-frame DOM
 * observation until the target body is correct and geometry holds for five
 * consecutive rendered frames, or the leg deadline passes. */
async function runLeg(page: Page, wire: Awaited<ReturnType<typeof wireProbe>>, spec: LegSpec) {
  const reqN = wire.requests.length, resN = wire.responses.length, noteN = wire.notifications.length;
  const nodeT0 = performance.now();
  await diagSetLeg(page, spec.name, spec.sessionId, { reply: spec.reply, prompt: spec.prompt, otherReply: spec.otherReply, otherPrompt: spec.otherPrompt });
  let clickError: string | undefined;
  try { await page.locator(`button[data-session-id="${spec.sessionId}"]`).click({ timeout: 15_000 }); }
  catch (cause) { clickError = String(cause).slice(0, 300); }
  const deadline = Date.now() + LEG_TIMEOUT_MS;
  let done = false;
  while (!clickError && Date.now() < deadline) {
    const p = await diagProgress(page, spec.name);
    if (p.done) { done = true; break; }
    await page.waitForTimeout(120);
  }
  const collected = await diagCollect(page, spec.name);
  const waitedMs = performance.now() - nodeT0;
  const clickT = collected.clickT as number | null;
  const rel = (t: number | null | undefined) => (t != null && clickT != null ? +(t - clickT).toFixed(1) : null);
  return {
    ...spec, clickError, settled: done, waitedMs: +waitedMs.toFixed(1),
    clickToFirstBodyMs: rel(collected.firstBodyT),
    clickToActionableMs: rel(collected.firstActionT),
    clickToLiveMs: rel(collected.frames.find((f: any) => f.live && f.selected === spec.sessionId && f.reply)?.t),
    clickToStableScrollMs: collected.stableScroll ? rel(collected.stableScroll.end) : null,
    clickToStableGeometryMs: collected.stableAll ? rel(collected.stableAll.end) : null,
    clickToSidebarSelectedMs: rel(collected.selectedT),
    stableScrollStartMs: collected.stableScroll ? rel(collected.stableScroll.start) : null,
    stableGeometryStartMs: collected.stableAll ? rel(collected.stableAll.start) : null,
    counts: collected.counts, editor: collected.editor,
    rpcs: {
      requests: wire.requests.slice(reqN).map(r => ({ method: r.method, atMs: +(((wire.requestTimes.get(r.id) ?? nodeT0) - nodeT0).toFixed(1)) })),
      responses: wire.responses.slice(resN).map(r => ({ method: r.method, type: (r.result as any)?.type ?? null })),
      notifications: wire.notifications.slice(noteN).map(n => ({ method: n.method, session: (n.params as any)?.target?.session_id ?? null })),
    },
    frames: collected.frames,
  };
}

function legSpecs(prefix: string, a: string, b: string): LegSpec[] {
  const A = { reply: MARKERS.a.reply, prompt: MARKERS.a.prompt, otherReply: MARKERS.b.reply, otherPrompt: MARKERS.b.prompt };
  const B = { reply: MARKERS.b.reply, prompt: MARKERS.b.prompt, otherReply: MARKERS.a.reply, otherPrompt: MARKERS.a.prompt };
  return [
    { name: `${prefix}-open-A-first`, sessionId: a, ...A },
    { name: `${prefix}-A-to-B-first`, sessionId: b, ...B },
    { name: `${prefix}-B-to-A-warm`, sessionId: a, ...A },
    { name: `${prefix}-A-to-B-warm`, sessionId: b, ...B },
    { name: `${prefix}-B-to-A-warm2`, sessionId: a, ...A },
  ];
}

test('short A-B-A session switching frame diagnostics', async ({ page, seeded }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const { fixture, a, b, pass } = seeded;
  const wire = await wireProbe(page);
  await routeWorkspaceHost(page, fixture);
  await installSampler(page);
  await page.goto('/');
  await connectRemote(page, fixture.endpoint, fixture.token);
  await expect(page.locator(`button[data-session-id="${a}"]`)).toBeVisible({ timeout: 30_000 });
  await expect(page.locator(`button[data-session-id="${b}"]`)).toBeVisible({ timeout: 30_000 });

  const results: Awaited<ReturnType<typeof runLeg>>[] = [];
  for (const spec of legSpecs('p1', a, b)) results.push(await runLeg(page, wire, spec));

  // Second pass on a cold transport: every attachment is a first-open again.
  await page.evaluate(() => { try { localStorage.clear(); } catch { /* Storage may be disabled. */ } localStorage.setItem('rustx-locale-v1', 'en'); });
  await page.reload();
  await connectRemote(page, fixture.endpoint, fixture.token);
  await expect(page.locator(`button[data-session-id="${a}"]`)).toBeVisible({ timeout: 30_000 });
  await expect(page.locator(`button[data-session-id="${b}"]`)).toBeVisible({ timeout: 30_000 });
  for (const spec of legSpecs('p2', a, b)) results.push(await runLeg(page, wire, spec));

  const report = {
    scenario: 'web_composer_identity',
    markers: MARKERS,
    sessions: { a, b },
    pageErrors: errors,
    legs: results,
  };
  await test.info().attach('session-switch-short-report', { body: JSON.stringify(report, null, 2), contentType: 'application/json' });
  for (const leg of results) {
    console.log(`[leg] ${leg.name} settled=${leg.settled} waited=${leg.waitedMs}ms body=${leg.clickToFirstBodyMs}ms live=${leg.clickToLiveMs}ms action=${leg.clickToActionableMs}ms stableScroll=${leg.clickToStableScrollMs}ms stableGeom=${leg.clickToStableGeometryMs}ms sidebar=${leg.clickToSidebarSelectedMs}ms counts=${JSON.stringify(leg.counts)} rpcs=${leg.rpcs.requests.map(r => r.method).join(',')}${leg.clickError ? ` ERROR=${leg.clickError}` : ''}`);
  }
  if (errors.length) console.log(`[pageerrors] ${errors.join(' | ')}`);
  expect(errors).toEqual([]);
  for (const leg of results) {
    expect(leg.settled, leg.name).toBe(true);
    expect(leg.clickToStableGeometryMs, leg.name).not.toBeNull();
    if (leg.name.startsWith('p1-') && leg.name.endsWith('-first')) {
      expect(leg.clickToLiveMs, leg.name).not.toBeNull();
      expect(leg.clickToFirstBodyMs!, 'history visible during the delayed MCP handshake').toBeLessThan(leg.clickToLiveMs!);
    }
    expect(leg.counts.bodyShiftPx, leg.name).toBeLessThan(3);
    if (leg.name.includes('warm')) {
      expect(leg.counts.blank, leg.name).toBe(0);
      expect(leg.counts.skeleton, leg.name).toBe(0);
    }
  }
  pass();
});
