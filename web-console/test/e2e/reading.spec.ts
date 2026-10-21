import { test as base, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { connectRemote, choose, openSettingsPage } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { followRoot } from './source-reading';

const READING_PREPARATION_MS = 240_000;

// Building 300 durable Attempts is preparation, with its own fixture budget.
// Browser/race assertions retain the repository's 120-second deadline and gates.
const test = base.extend<{ readingScenario: string; reading: { fixture: Awaited<ReturnType<typeof startDogfood>>; id: string; forkId: string; sourceNode: string; sourceMessage: string; sourceCursor: string; pass: () => void } }>({
  readingScenario: ['web_reading_surface', { option: true }],
  reading: [async ({ readingScenario }, use) => {
    const begun = performance.now();
    let phase = 'native startup';
    let settledAttempts = 0;
    let inFlight: number | undefined;
    const recent: { attempt: number; admissionMs: number; settlementMs: number }[] = [];
    let following: Awaited<ReturnType<typeof followRoot>> | undefined;
    // Observe the existing fixture deadline only. This neither cancels nor
    // retries native work; it preserves bounded evidence if setup never returns.
    let fixture: Awaited<ReturnType<typeof startDogfood>> | undefined;
    let prepared = false;
    let diagnosed = false;
    const diagnose = (reason: string) => {
      if (diagnosed) return;
      diagnosed = true;
      console.error('READING_PREPARATION_INCOMPLETE', JSON.stringify({
        reason, scenario: readingScenario, phase, completed: settledAttempts, inFlight,
        elapsedMs: performance.now() - begun, recent, observation: following?.diagnostics(), native: fixture?.diagnostics().native,
      }));
    };
    const diagnostic = setTimeout(() => diagnose('fixture deadline'), READING_PREPARATION_MS);
    diagnostic.unref();
    let passed = false;
    try {
    fixture = await startDogfood(readingScenario);
    phase = 'connect';
    const remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    phase = 'session/create';
    const created = await remote.client.call('session/create', { settings: { cwd: fixture.workspaceA } }, 'session_transition');
    const id = created.session.id;
    phase = 'session/attach';
    const attached = await remote.client.call('session/attach', { session_id: id }, 'attached');
    phase = 'checkpoint/subscribe';
    following = await followRoot(remote, attached.target);
    // Every terminal barrier is a real native event. Seeding does not repeatedly
    // rebuild full snapshots or synthesize any history entry.
    for (let i = 0; i < 300; i++) {
      inFlight = i;
      phase = 'turn/start';
      const started = performance.now();
      const terminal = following.nextSettlement();
      await remote.client.call('turn/start', { target: attached.target, content: [{ type: 'text', text: `Reading ${i}` }] }, 'inbound_accepted');
      const admitted = performance.now();
      phase = 'terminal event';
      await terminal;
      settledAttempts++;
      recent.push({ attempt: i, admissionMs: admitted - started, settlementMs: performance.now() - admitted });
      if (recent.length > 8) recent.shift();
    }
    following.stop();
    inFlight = undefined;
    phase = 'first turn page';
    const firstPage = (await remote.client.call('session/turns', { target: attached.target, offset: 0, limit: 64 }, 'conversation_turns')).page;
    const first=firstPage.turns[0];
    phase = 'latest transcript';
    const latest = (await remote.client.call('session/transcript', { target: attached.target, at: { type: 'latest' }, limit: 32 }, 'transcript_window')).window;
    expect(BigInt(latest.page.entries![0].cursor) - BigInt(first.cursor!)).toBeGreaterThan(512n);
    phase = 'first turn transcript';
    const firstWindow = (await remote.client.call('session/transcript', { target: attached.target, at: { type: 'turn_ordinal', ordinal: first.ordinal, cut: firstPage.cut }, limit: 32 }, 'transcript_window')).window;
    const sourceEntry = firstWindow.page.entries!.find(entry => entry.item.type === 'message' && entry.item.message.role === 'assistant')!;
    if (sourceEntry.item.type !== 'message') throw new Error('Missing canonical source message');
    phase = 'exact original message';
    const exact = (await remote.client.call('session/transcript', { target: attached.target, at: { type: 'message', id: sourceEntry.item.message.id, cut: firstWindow.cut }, limit: 1 }, 'transcript_window')).window;
    expect(exact.target_cursor).toBe(sourceEntry.cursor);
    expect(exact.target ?? null).toBeNull();
    expect(exact.page.entries).toHaveLength(1);
    expect(exact.page.entries![0].item).toMatchObject({ type: 'message', message: { id: sourceEntry.item.message.id } });
    const completed = sourceEntry.completed_response;
    if (!completed) throw new Error('The selected source Message has no native completed-response cut.');
    phase = 'session/fork';
    const fork = await remote.client.call('session/fork', { session_id: id, node_id: created.session.active_node, surface_revision: completed.surface_revision, boundary: completed.closing_locator, side: 'after' }, 'session_transition');
    phase = 'session/detach';
    await remote.client.call('session/detach', { target: attached.target }, 'detached'); await remote.shutdown();
      clearTimeout(diagnostic);
      prepared = true;
      await use({ fixture, id, forkId: fork.session.id, sourceNode: created.session.active_node, sourceMessage: sourceEntry.item.message.id, sourceCursor: sourceEntry.cursor, pass: () => { passed = true; } });
    } catch (error) {
      if (!prepared) diagnose(error instanceof Error ? error.message.slice(0, 1024) : 'setup failed');
      throw error;
    } finally { clearTimeout(diagnostic); await fixture?.stop(passed); }
  }, { timeout: READING_PREPARATION_MS }],
});

// Gates delay transport replies, not native history or semantic owners.
// They prove visible pending/error states without sleep-based race assertions.
test('native distant reading rail, detached/latest follow and measured width in both locales', async ({ page, reading }) => {
  const { fixture, id } = reading;
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.addInitScript(() => {
      const NativeSocket = window.WebSocket;
      const state = { gate: false, fail: false, held: [] as (() => void)[], reads: 0, other: 0, requests: [] as any[], windows: [] as number[] };
      Object.assign(window, { readingTransport: state });
      class GatedSocket extends NativeSocket {
        private methods = new Map<string, string>();
        constructor(...args: ConstructorParameters<typeof WebSocket>) {
          super(...args);
          this.addEventListener('message', event => {
            const reply = JSON.parse(String(event.data));
            if (reply.id && this.methods.get(String(reply.id)) === 'turn') {
              event.stopImmediatePropagation();
              if(reply.result?.window)state.windows.push(reply.result.window.page.entries?.length ?? 0);
              const deliver = () => this.onmessage?.(new MessageEvent('message', { data: JSON.stringify(state.fail ? {jsonrpc:'2.0',id:reply.id,error:{code:-32000,message:'Reading fixture rejection'}} : reply) }));
              if (state.gate) state.held.push(deliver); else deliver();
            }
          });
        }
        send(data: Parameters<WebSocket['send']>[0]) {
          const request = JSON.parse(String(data));
          if (request.method === 'session/transcript' && request.params.at.type === 'turn_ordinal') { this.methods.set(String(request.id), 'turn'); state.reads++; }
          if (request.method === 'session/transcript') state.requests.push(request);
          if (request.method === 'session/transcript' && request.params.at.type !== 'turn_ordinal') state.other++;
          super.send(data);
        }
      }
      window.WebSocket = GatedSocket;
    });
    await routeWorkspaceHost(page, fixture); await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await page.locator(`button[data-session-id="${id}"]`).click();
    const rail = page.getByRole('navigation', { name: 'Turn navigation' }), viewport = page.locator('[data-conversation-scroll]').first();
    const railScroll = (top: number) => rail.locator(':scope > div').first().evaluate((el, top) => { el.scrollTop = top; }, top);
    // Initial history has its own bounded fill. Finish it before measuring
    // requests attributable to distant navigation.
    await expect.poll(() => page.evaluate(() => (window as any).readingTransport.other)).toBe(2);
    await expect(page.getByRole('button', { name: 'Load earlier', exact: true })).toBeEnabled();
    const initialReads = await page.evaluate(() => (window as any).readingTransport.requests);
    expect(initialReads).toHaveLength(2);
    expect(initialReads.every((request: any) => request.params.at.type === 'older' && request.params.limit === 32)).toBe(true);
    // Every native turn has a mark; following the latest output places the newest in view.
    const newest = rail.locator('[data-turn-ordinal="300"]');
    await expect(newest).toBeInViewport();
    await expect(rail.locator('[data-turn-ordinal="1"]')).toHaveCount(0);
    await expect(rail.locator('[data-turn-id]').first()).toBeVisible(); // latest native page 257..300
    await newest.hover();
    await expect(rail.getByRole('tooltip')).toContainText('Reading 299');
    await expect(rail.getByRole('tooltip')).toContainText('Native reading answer 299');
    // A rejected history read fails visibly and stays recoverable.
    await railScroll(1900);
    const errorMark=rail.locator('[data-turn-ordinal="200"]');
    await page.evaluate(() => { (window as any).readingTransport.fail=true; }); await errorMark.click();
    await expect(page.locator('[data-turn-navigator]').getByRole('alert')).toContainText('Reading fixture rejection');
    await page.evaluate(() => { (window as any).readingTransport.fail=false; });
    await page.locator('[data-turn-navigator]').getByRole('button', { name: 'Reload turns' }).click();
    await expect(page.locator('[data-turn-navigator]').getByRole('alert')).toHaveCount(0);
    // A distant unloaded mark reads its outline page and exactly one bounded
    // native Turn window; no intervening history is materialized.
    await railScroll(0);
    const mark = rail.locator('[data-turn-ordinal="1"]');
    await expect(mark).toHaveAccessibleName('Load and jump to turn 1');
    await mark.focus(); await expect(rail.getByRole('tooltip')).toContainText('Turn 1');
    await page.evaluate(() => { (window as any).readingTransport.gate = true; });
    await page.keyboard.press('Enter'); await expect(mark).toHaveAttribute('aria-busy', 'true');
    await expect.poll(() => page.evaluate(() => (window as any).readingTransport.held.length)).toBe(1);
    await expect(mark).toHaveAccessibleName('Jump to turn 1');
    await expect(rail.getByRole('tooltip')).toContainText('Reading 0');
    await page.evaluate(() => { const s=(window as any).readingTransport; s.gate=false; s.held.shift()(); });
    await expect(mark).toHaveAttribute('aria-current', 'true');
    const target = page.locator('[data-chat-anchor-key]').filter({ hasText: 'Native reading answer 0' }).first();
    await expect(page.getByText(/Native reading answer 0\n/).first()).toBeVisible();
    await expect.poll(async () => Math.abs(await target.evaluate(el => el.getBoundingClientRect().top) - await viewport.evaluate(el=>el.getBoundingClientRect().top))).toBeLessThan(3);
    // Manual reading across user prompts and message spacing must keep one
    // highlighted native turn, in both directions, without a navigation click.
    await page.mouse.move(10, 10);
    const stops = await viewport.evaluate(el => {
      const owners = [...el.querySelectorAll<HTMLElement>('[data-chat-turn-owner]')].filter(row => !row.closest('[hidden]'));
      const first = owners[0], next = owners.find(row => row.dataset.chatTurnOwner !== first.dataset.chatTurnOwner)!;
      const offset = el.scrollTop - el.getBoundingClientRect().top - Math.min(96, el.clientHeight * 0.2);
      const start = first.getBoundingClientRect().top + offset;
      const end = next.getBoundingClientRect().top + offset;
      return { first: first.dataset.chatTurnOwner!.slice(5), next: next.dataset.chatTurnOwner!.slice(5),
        positions: [start + 1, (start + end) / 2, end - 1, end + 1, end - 1, start + 1], end };
    });
    for (const top of stops.positions) {
      await viewport.evaluate((el, top) => { el.scrollTop = top; el.dispatchEvent(new Event('scroll')); }, top);
      await expect(rail.locator('[aria-current="true"]')).toHaveCount(1);
      await expect(rail.locator('[aria-current="true"]')).toHaveAttribute('data-turn-id', top >= stops.end ? stops.next : stops.first);
    }
    // Two direct windows (one rejected, one accepted), independent live tail,
    // and a finite presentation. No prefix traversal or duplicate output.
    await test.info().attach('reading-requests', { body: JSON.stringify(await page.evaluate(() => (window as any).readingTransport.requests), null, 2), contentType: 'application/json' });
    expect(await page.evaluate(()=>(window as any).readingTransport.requests.filter((request: any) => request.params.at.type !== 'turn_ordinal'))).toEqual(initialReads);
    expect(await page.evaluate(()=>(window as any).readingTransport.reads)).toBe(2);
    await expect(page.getByText(/Native reading answer 299\n/)).toHaveCount(1);
    expect(await page.evaluate(()=>(window as any).readingTransport.windows)).toEqual([32,32]);
    expect(await viewport.locator('[data-chat-anchor-key]').count()).toBeLessThanOrEqual(256);
    await expect(page.getByRole('button', { name: 'Load later', exact: true })).toHaveCount(0);
    const continuation = page.locator('[data-chat-history-boundary]');
    await expect(continuation).toHaveCount(1);
    const after = await continuation.getAttribute('data-chat-history-boundary');
    await continuation.scrollIntoViewIfNeeded();
    await viewport.hover();
    await page.mouse.wheel(0, 80);
    await expect.poll(() => page.evaluate(() => (window as any).readingTransport.other)).toBe(initialReads.length + 1);
    await expect(continuation).not.toHaveAttribute('data-chat-history-boundary', after!);
    await page.screenshot({ path: '/tmp/rustx-history-main-continuation.png' });
    await page.getByRole('button', { name: 'Return to latest', exact: true }).click();
    await expect.poll(()=>viewport.evaluate(el=>el.scrollHeight-el.clientHeight-el.scrollTop)).toBeLessThan(3);
    // Ordinary detached state has no historical jump/cache-limit prerequisite.
    await viewport.evaluate(el=>{el.scrollTop=100;el.dispatchEvent(new Event('scroll'));});
    await expect(page.getByRole('button', { name: 'Return to latest', exact: true })).toBeVisible();
    await page.getByLabel('Message', { exact: true }).fill('Stream reading'); await page.getByRole('button', { name: 'Send', exact: true }).click();
    await fixture.gate('reading-detached');
    const before=await viewport.evaluate(el=>el.scrollTop); await fixture.release('reading-detached'); await fixture.gate('reading-latest');
    await expect(page.getByText(/Detached output/).last()).toHaveCount(1);
    expect(await viewport.evaluate(el=>el.scrollTop)).toBe(before);
    await page.getByRole('button', { name: 'Return to latest', exact: true }).click();
    await fixture.release('reading-latest'); await expect(page.getByText(/Following output/).last()).toHaveCount(1);
    await expect.poll(()=>viewport.evaluate(el=>el.scrollHeight-el.clientHeight-el.scrollTop)).toBeLessThan(3);
    // Both physical handles use the measured column; persisted intent restores
    // after the mobile column temporarily clamps it.
    const right=page.getByRole('slider',{name:'Resize conversation from the right'});
    await expect(right).toBeVisible(); const initial=Number(await right.getAttribute('aria-valuenow')), box=(await right.boundingBox())!;
    await page.mouse.move(box.x+box.width/2,box.y+100);await page.mouse.down();await page.mouse.move(box.x+box.width/2+24,box.y+100);await page.mouse.up();
    const preferred=await page.evaluate(()=>localStorage.getItem('rustx-conversation-width-v1'));expect(Number(preferred)).toBeGreaterThan(initial);
    const left=page.getByRole('slider',{name:'Resize conversation from the left'});await left.focus();await page.keyboard.press('ArrowLeft');
    const saved=await page.evaluate(()=>localStorage.getItem('rustx-conversation-width-v1'));
    await page.setViewportSize({width:390,height:844});await expect(page.locator('[data-width-handle]')).toHaveCount(0);
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await openSettingsPage(page, 'General');
    await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
    await page.getByRole('button', { name: '关闭设置', exact: true }).click();
    // As in Harness, a narrow reading column has no rail.
    await expect(page.getByRole('navigation',{name:'轮次导航'})).toBeHidden();
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
    await page.setViewportSize({width:1440,height:1000});
    await expect(page.getByRole('navigation',{name:'轮次导航'})).toBeVisible();
    await page.getByRole('navigation',{name:'轮次导航'}).getByRole('button').first().focus();await expect(page.getByRole('tooltip')).toBeVisible();await expect(page.locator('[data-width-handle="right"]')).toHaveAttribute('aria-valuenow',String(Math.round(Number(saved))));
    await expect(page.locator('[data-conversation-width-owner]').locator('..')).toHaveCSS('--dsh-chat-content-width', `${saved}px`);
    expect(await page.evaluate(()=>localStorage.getItem('rustx-conversation-width-v1'))).toBe(saved);
    expect(errors).toEqual([]);reading.pass();
});
