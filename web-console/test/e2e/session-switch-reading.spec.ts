import { test as base, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { connectRemote } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { followRoot } from './source-reading';

const SEED_TURNS = 72;
const PREPARATION_MS = 240_000;

interface Seeded { fixture: Awaited<ReturnType<typeof startDogfood>>; a: string; b: string }

// Two durable long histories are fixture preparation, not the measured path.
// The provider scenario is intentionally partial: stop() skips its
// all-steps-settled report, which only full-length scenarios can satisfy.
// The browser budget stays at the repository's 120 seconds.
const test = base.extend<{ seeded: Seeded }>({
  seeded: [async ({}, use) => {
    const fixture = await startDogfood('web_reading_surface');
    try {
      const remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
      const ids: string[] = [];
      for (const [index, label] of ['Alpha', 'Beta'].entries()) {
        const created = await remote.client.call('thread/start', { settings: { cwd: fixture.workspaceA } }, 'session_transition');
        const id = created.session.id; ids.push(id);
        await remote.renameSession(id, `Session ${label}`);
        const attached = await remote.client.call('thread/resume', { threadId: (await remote.readSession(id)).active_conversation_id }, 'attached');
        const following = await followRoot(remote, attached.target);
        for (let i = index * SEED_TURNS; i < (index + 1) * SEED_TURNS; i++) {
          const terminal = following.nextSettlement();
          await remote.client.call('inbound/queue', { inputId: crypto.randomUUID(), target: attached.target, content: [{ type: 'text', text: `Reading ${i}` }] }, 'inbound_accepted');
          await terminal;
        }
        following.stop();
        await remote.client.call('session/detach', { target: attached.target }, 'detached');
      }
      await remote.shutdown();
      await use({ fixture, a: ids[0], b: ids[1] });
    } finally { await fixture.stop(false); }
  }, { timeout: PREPARATION_MS }],
});

interface Sample { t: number; top: number; floor: number; height: number; marker: string; session: boolean; latest: boolean }

async function sample(page: import('@playwright/test').Page, sessionText: string): Promise<Sample> {
  return page.evaluate(marker => {
    const el = document.querySelector<HTMLElement>('[data-conversation-scroll]');
    if (!el) return { t: performance.now(), top: -1, floor: -1, height: -1, marker: '', session: false, latest: false };
    const rows = [...el.querySelectorAll<HTMLElement>('[data-chat-anchor-key]')].filter(row => !row.closest('[hidden]'));
    const edge = el.getBoundingClientRect().top;
    const row = rows.find(row => row.getBoundingClientRect().bottom > edge) ?? rows[0];
    return {
      t: performance.now(), top: el.scrollTop, floor: Math.max(0, el.scrollHeight - el.clientHeight),
      height: el.scrollHeight, marker: (row?.textContent ?? '').replace(/\s+/g, ' ').slice(0, 120),
      session: el.textContent?.includes(marker) ?? false,
      latest: !!document.querySelector('[data-chat-latest]'),
    };
  }, sessionText);
}

async function waitBottom(page: import('@playwright/test').Page, sessionText: string, timeoutMs = 30_000) {
  const start = Date.now();
  const series: Sample[] = [];
  let stable = 0;
  while (Date.now() - start < timeoutMs) {
    const s = await sample(page, sessionText);
    series.push(s);
    const ok = s.session && s.top >= 0 && Math.abs(s.floor - s.top) < 3;
    stable = ok ? stable + 1 : 0;
    if (stable >= 3) return { series, settled: s };
    await page.waitForTimeout(50);
  }
  return { series, settled: series.at(-1)! };
}

/** Click a sidebar session and record in-page phase marks until the reading
 * surface is stable: correct session text, stable geometry, settled scroll. */
async function switchAndMeasure(page: import('@playwright/test').Page, sessionId: string, sessionText: string, savedTop: number | undefined) {
  await page.evaluate(id => {
    const button = document.querySelector<HTMLButtonElement>(`button[data-session-id="${id}"]`)!;
    button.addEventListener('click', () => { (window as any).__switchAt = performance.now(); }, { once: true });
    button.click();
  }, sessionId);
  const start = Date.now();
  const series: Sample[] = [];
  let stable = 0, firstContent = -1, lastTop = -2, lastHeight = -2;
  while (Date.now() - start < 30_000) {
    const s = await sample(page, sessionText);
    series.push(s);
    if (firstContent < 0 && s.session) firstContent = s.t;
    const geometrySame = Math.abs(s.top - lastTop) < 0.5 && s.height === lastHeight;
    lastTop = s.top; lastHeight = s.height;
    if (s.session && geometrySame) stable++; else stable = 0;
    if (stable >= 4) break;
    await page.waitForTimeout(50);
  }
  const t0 = await page.evaluate(() => (window as any).__switchAt ?? 0);
  const last = series.at(-1)!;
  const scrollOk = stable >= 4 && last.session && (savedTop === undefined ? Math.abs(last.floor - last.top) < 3 : Math.abs(last.top - savedTop) < 3);
  return {
    clickToFirstContentMs: firstContent > 0 ? firstContent - t0 : null,
    clickToStableMs: last.t - t0,
    finalTop: last.top, finalFloor: last.floor, finalMarker: last.marker,
    expectedTop: savedTop ?? null,
    retained: scrollOk,
    series,
  };
}

test('warm A-B-A session switching preserves detached reading position', async ({ page, seeded }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  const { fixture, a, b } = seeded;
  await routeWorkspaceHost(page, fixture);
  await page.goto('/');
  await connectRemote(page, fixture.endpoint, fixture.token);

  const viewport = page.locator('[data-conversation-scroll]').first();
  const report: Record<string, unknown> = {};

  // Cold open of A: seed the warm view, then detach at a mid-history position.
  await page.locator(`button[data-session-id="${a}"]`).click();
  const aOpen = await waitBottom(page, 'Native reading answer 71');
  report.aFirstOpen = { settledTop: aOpen.settled.top, floor: aOpen.settled.floor, marker: aOpen.settled.marker, samples: aOpen.series.length };
  await viewport.evaluate(el => { el.scrollTop = Math.floor(el.scrollHeight * 0.35); el.dispatchEvent(new Event('scroll')); });
  await expect(page.getByRole('button', { name: 'Return to latest', exact: true })).toBeVisible({ timeout: 10_000 });
  await page.waitForTimeout(100);
  report.aSaved = await sample(page, 'Native reading answer 71');

  // First B open (cold -> warm), detach at a different depth.
  await page.locator(`button[data-session-id="${b}"]`).click();
  const bOpen = await waitBottom(page, 'Native reading answer 143');
  report.bFirstOpen = { settledTop: bOpen.settled.top, floor: bOpen.settled.floor, marker: bOpen.settled.marker, samples: bOpen.series.length };
  await viewport.evaluate(el => { el.scrollTop = Math.floor(el.scrollHeight * 0.6); el.dispatchEvent(new Event('scroll')); });
  await expect(page.getByRole('button', { name: 'Return to latest', exact: true })).toBeVisible({ timeout: 10_000 });
  await page.waitForTimeout(100);
  report.bSaved = await sample(page, 'Native reading answer 143');

  // Warm B -> A -> B -> A: every revisit must restore the detached offset
  // captured before leaving that session.
  const aSavedTop = (report.aSaved as Sample).top;
  const bSavedTop = (report.bSaved as Sample).top;
  report.warmToA = await switchAndMeasure(page, a, 'Native reading answer 71', aSavedTop);
  report.warmToB = await switchAndMeasure(page, b, 'Native reading answer 143', bSavedTop);
  report.warmBackToA = await switchAndMeasure(page, a, 'Native reading answer 71', aSavedTop);

  await test.info().attach('session-switch-report', { body: JSON.stringify(report, null, 2), contentType: 'application/json' });
  expect(errors).toEqual([]);
  expect((report.warmToA as { retained: boolean }).retained, 'first return to A must restore its detached reading position').toBe(true);
  expect((report.warmBackToA as { retained: boolean }).retained, 'returning to A must restore its detached reading position').toBe(true);
  expect((report.warmToB as { retained: boolean }).retained, 'returning to B must restore its detached reading position').toBe(true);
});
