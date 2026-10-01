import { test as base, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { connectRemote, choose, openSettingsPage } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';

// Building 300 durable Attempts is preparation, with its own fixture budget.
// Browser/race assertions retain the repository's 120-second deadline and gates.
const test = base.extend<{ reading: { fixture: Awaited<ReturnType<typeof startDogfood>>; id: string; pass: () => void } }>({
  reading: [async ({}, use) => {
    const fixture = await startDogfood('web_reading_surface');
    let passed = false;
    try {
    const remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    const created = await remote.client.call('session/create', { settings: { cwd: fixture.workspaceA } }, 'session_transition');
    const id = created.session.id;
    const attached = await remote.client.call('session/attach', { session_id: id }, 'attached');
    await remote.client.call('session/subscribe', { target: attached.target, after_cursor: attached.cursor }, 'subscribed');
    // Every terminal barrier is a real native event. Seeding does not repeatedly
    // rebuild full snapshots or synthesize any history entry.
    for (let i = 0; i < 300; i++) {
      let stop: () => void = () => {};
      const terminal = new Promise<void>((resolve, reject) => {
        stop = remote.client.onNotification(note => {
          if (note.method !== 'session/event' || note.params.target.conversation_id !== attached.target.conversation_id || note.params.event.type !== 'attempt_settled') return;
          stop();
          if (note.params.event.outcome.type === 'completed') resolve();
          else reject(new Error(`Reading Attempt ${i} failed: ${JSON.stringify(note.params.event.outcome)}`));
        });
      });
      await remote.client.call('turn/start', { target: attached.target, content: [{ type: 'text', text: `Reading ${i}` }] }, 'inbound_accepted');
      await terminal;
    }
    const first = (await remote.client.call('session/turns', { target: attached.target, offset: 0, limit: 64 }, 'conversation_turns')).page.turns[0];
    const latest = (await remote.client.call('session/transcript', { target: attached.target, at: { type: 'latest' }, limit: 64 }, 'transcript_window')).window;
    expect(BigInt(latest.page.entries![0].cursor) - BigInt(first.cursor!)).toBeGreaterThan(512n);
    await remote.client.call('session/detach', { target: attached.target }, 'detached'); await remote.shutdown();
      await use({ fixture, id, pass: () => { passed = true; } });
    } finally { await fixture.stop(passed); }
  }, { timeout: 240_000 }],
});

// Gates delay transport replies, not native history or semantic owners.
// They prove visible pending/error states without sleep-based race assertions.
test('native distant reading rail, detached/latest follow and measured width in both locales', async ({ page, reading }) => {
  const { fixture, id } = reading;
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.addInitScript(() => {
      const NativeSocket = window.WebSocket;
      const state = { gate: false, fail: false, held: [] as (() => void)[], reads: 0 };
      Object.assign(window, { readingTransport: state });
      class GatedSocket extends NativeSocket {
        private methods = new Map<string, string>();
        constructor(...args: ConstructorParameters<typeof WebSocket>) {
          super(...args);
          this.addEventListener('message', event => {
            const reply = JSON.parse(String(event.data));
            if (reply.id && this.methods.get(String(reply.id)) === 'turn') {
              event.stopImmediatePropagation();
              const deliver = () => this.onmessage?.(new MessageEvent('message', { data: JSON.stringify(state.fail ? {jsonrpc:'2.0',id:reply.id,error:{code:-32000,message:'Reading fixture rejection'}} : reply) }));
              if (state.gate) state.held.push(deliver); else deliver();
            }
          });
        }
        send(data: Parameters<WebSocket['send']>[0]) {
          const request = JSON.parse(String(data));
          if (request.method === 'session/transcript' && request.params.at.type === 'turn') { this.methods.set(String(request.id), 'turn'); state.reads++; }
          super.send(data);
        }
      }
      window.WebSocket = GatedSocket;
    });
    await routeWorkspaceHost(page, fixture); await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await page.locator(`button[data-session-id="${id}"]`).click();
    const rail = page.locator('[data-turn-navigator]'), viewport = page.locator('.conversation-scroll');
    await expect(rail.locator('[data-turn-id]')).toHaveCount(44); // latest native page 257..300
    await rail.getByRole('spinbutton', { name: 'Turn number' }).fill('1');
    await rail.getByRole('button', { name: 'Show this turn page' }).click();
    await expect(rail.locator('[data-turn-id]')).toHaveCount(64);
    const mark = rail.locator('[data-turn-ordinal="1"]');
    await mark.focus(); await expect(rail.getByRole('tooltip')).toContainText('Native reading answer 0');
    await page.keyboard.press('ArrowDown'); await expect(rail.locator('[data-turn-ordinal="2"]')).toBeFocused();
    await page.keyboard.press('Home'); await expect(mark).toBeFocused();
    await mark.hover(); await expect(rail.getByRole('tooltip')).toBeVisible();
    await page.evaluate(() => { (window as any).readingTransport.gate = true; });
    await page.keyboard.press('Enter'); await expect(mark).toHaveAttribute('aria-busy', 'true');
    await expect.poll(() => page.evaluate(() => (window as any).readingTransport.held.length)).toBe(1);
    await page.evaluate(() => { const s=(window as any).readingTransport; s.gate=false; s.held.shift()(); });
    await expect(mark).toHaveAttribute('aria-current', 'true');
    const target = page.locator('[data-chat-anchor-key]').filter({ hasText: 'Native reading answer 0' }).first();
    await expect(page.getByText(/Native reading answer 0\n/).first()).toBeVisible();
    await expect.poll(async () => Math.abs(await target.evaluate(el => el.getBoundingClientRect().top) - await viewport.evaluate(el=>el.getBoundingClientRect().top))).toBeLessThan(3);
    expect(await page.evaluate(()=>(window as any).readingTransport.reads)).toBe(1);
    expect(await page.locator('[data-chat-anchor-key]').count()).toBeLessThan(200);
    // A rejected unloaded turn keeps the currently selected historical page.
    await rail.getByRole('spinbutton', { name: 'Turn number' }).fill('200'); await rail.getByRole('button', { name: 'Show this turn page' }).click();
    const errorMark=rail.locator('[data-turn-ordinal="200"]');
    await page.evaluate(() => { (window as any).readingTransport.fail=true; }); await errorMark.click();
    await expect(rail.getByRole('alert')).toContainText('Reading fixture rejection');
    await page.evaluate(() => { (window as any).readingTransport.fail=false; });
    await rail.getByRole('button', { name: 'Reload turns' }).click();
    await expect(page.getByText(/Native reading answer 0\n/).first()).toHaveCount(1);
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
    await expect(page.getByRole('complementary',{name:'轮次导航'})).toBeVisible();
    await page.locator('[data-turn-id]').first().focus();await expect(page.getByRole('tooltip')).toBeVisible();
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
    await page.setViewportSize({width:1440,height:1000});await expect(page.locator('[data-width-handle="right"]')).toHaveAttribute('aria-valuenow',String(Math.round(Number(saved))));
    await expect(page.locator('[data-conversation-width-owner]').locator('..')).toHaveCSS('--dsh-chat-content-width', `${saved}px`);
    expect(await page.evaluate(()=>localStorage.getItem('rustx-conversation-width-v1'))).toBe(saved);
    expect(errors).toEqual([]);reading.pass();
});
