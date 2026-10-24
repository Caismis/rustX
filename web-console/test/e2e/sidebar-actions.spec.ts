import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { choose, connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';

for (const [width, zh] of [[1440, false], [390, true]] as const) {
  test(`DSH Session list actions and native delete confirmation ${width}`, async ({ page, context }) => {
    const fixture = await startDogfood('web_sidebar_actions'), wire = await wireProbe(page);
    let passed = false;
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    const showSidebar = async () => { const expand = page.getByRole('button', { name: zh ? '打开侧边栏' : 'Expand Sidebar', exact: true }); if (await expand.isVisible()) await expand.click(); };
    const dismissSidebar = async () => { const close = page.getByRole('button', { name: zh ? '关闭导航' : 'Close navigation', exact: true }); if (await close.isVisible()) await close.click({ position: { x: width - 20, y: 400 } }); };
    const actions = async (id: string) => { await showSidebar(); await page.locator(`button[data-session-id="${id}"]`).hover(); await page.locator(`button[data-session-actions="${id}"]`).click(); };
    const message = page.getByRole('textbox', { name: zh ? '消息' : 'Message', exact: true });
    const send = async (text: string) => { await dismissSidebar(); await message.fill(text); await page.getByRole('button', { name: zh ? '发送' : 'Send', exact: true }).click(); };
    const selected = () => page.locator('button[data-session-id][aria-current=page]').getAttribute('data-session-id');
    const target = (id: string) => [...wire.responses].reverse().find(row => row.method === 'session/attach' && row.result?.target?.session_id === id)!.result.target;
    try {
      await context.grantPermissions(['clipboard-read', 'clipboard-write']);
      await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 }); await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript(zh => localStorage.setItem('rustx-appearance-v1', zh ? 'dark' : 'light'), zh);
      await routeWorkspaceHost(page, fixture); await page.goto('/'); await expect(page).toHaveTitle(/rustX/);
      await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
      const source = (await selected())!;
      if (zh) {
        await page.getByRole('button', { name: 'Settings', exact: true }).click();
        await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文'); await page.getByRole('button', { name: '关闭设置' }).click();
      }
      await send('Sidebar source'); await expect(page.getByText('Completed sidebar answer.', { exact: true })).toBeVisible();
      await expect.poll(async () => (await wire.read('session/checkpoint', target(source))).checkpoint.snapshot.attempt?.phase.type).toBe('settled');
      await actions(source);
      expect(await page.getByRole('menuitem').allTextContents()).toEqual(zh ? ['置顶会话', '重命名', '分叉会话', '删除会话', '复制 Session ID'] : ['Pin session', 'Rename', 'Fork session', 'Delete Session', 'Copy Session ID']);
      await page.screenshot({ path: `/tmp/rustx-session-menu-${width}.png` });
      await page.getByRole('menuitem', { name: zh ? '置顶会话' : 'Pin session', exact: true }).click();
      await actions(source); await page.getByRole('menuitem', { name: zh ? '复制 Session ID' : 'Copy Session ID', exact: true }).click();
      await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(source);
      await expect(page.locator('[data-toast]')).toContainText(zh ? 'Session ID 已复制' : 'Session ID copied');
      await actions(source); await page.getByRole('menuitem', { name: zh ? '重命名' : 'Rename', exact: true }).click();
      const rename = page.getByRole('dialog', { name: zh ? '重命名会话' : 'Rename session', exact: true });
      const input = rename.getByRole('textbox', { name: zh ? '会话名称' : 'Session name', exact: true });
      await expect(input).toHaveValue('New session');
      await expect(input).toBeFocused(); expect(await input.evaluate((el: HTMLInputElement) => [el.selectionStart, el.selectionEnd])).toEqual([0, 'New session'.length]);
      await page.screenshot({ path: `/tmp/rustx-session-rename-${width}.png` });
      await rename.getByRole('button', { name: zh ? '取消' : 'Cancel', exact: true }).click();
      await expect(page.locator(`button[data-session-id="${source}"]`)).toBeFocused();
      if (width === 1440) await page.locator(`button[data-session-id="${source}"]`).dblclick();
      else { await actions(source); await page.getByRole('menuitem', { name: '重命名', exact: true }).click(); }
      await expect(input).toBeFocused();
      await input.fill('Renamed source'); await input.press('Enter'); await expect(rename).toHaveCount(0);
      await expect(page.locator(`button[data-session-id="${source}"]`)).toContainText('Renamed source');
      await page.locator(`button[data-session-id="${source}"]`).hover();
      await page.getByRole('treeitem').filter({ has: page.locator(`button[data-session-id="${source}"]`) }).getByRole('button', { name: zh ? '删除会话' : 'Delete Session', exact: true }).click();
      const deletion = page.getByRole('dialog', { name: zh ? '删除此会话？' : 'Delete this session?', exact: true });
      await expect(deletion).toContainText('Renamed source'); await expect(deletion).toContainText(zh ? '无法恢复' : 'cannot be undone');
      await expect(deletion.getByRole('button', { name: zh ? '删除' : 'Delete', exact: true })).toBeEnabled();
      await page.screenshot({ path: `/tmp/rustx-session-delete-${width}.png` });
      await deletion.getByRole('button', { name: zh ? '取消' : 'Cancel', exact: true }).click();
      await expect(page.locator(`button[data-session-id="${source}"]`)).toBeFocused();
      expect(wire.requests.filter(row => row.method === 'session/delete')).toHaveLength(0);
      await send('Keep source running'); await fixture.gate('sidebar-source-running');
      const before = wire.requests.length;
      await actions(source); await page.getByRole('menuitem', { name: zh ? '分叉会话' : 'Fork session', exact: true }).click();
      await expect(page.getByRole('dialog')).toHaveCount(0);
      await expect.poll(() => wire.responses.filter(row => row.method === 'session/fork').length).toBe(1);
      const forked = wire.responses.find(row => row.method === 'session/fork')!.result.session;
      expect(forked.name).toBe('Renamed source');
      expect(forked.generated_title).toBeNull();
      const child = forked.id;
      await expect(page.locator(`button[data-session-id="${child}"]`).getByText('Renamed source', { exact: true })).toBeVisible();
      expect(await selected()).toBe(source);
      await expect(page.getByRole('dialog')).toHaveCount(0);
      expect((await wire.read('session/checkpoint', target(source))).checkpoint.snapshot.attempt?.phase.type).toBe('running');
      expect(wire.requests.slice(before).filter(row => ['turn/cancel', 'session/switchNode', 'session/unload'].includes(row.method))).toEqual([]);
      expect(wire.requests.slice(before).filter(row => row.method === 'session/attach')).toEqual([]);
      expect(wire.requests.slice(before).filter(row => row.method === 'session/name')).toEqual([]);
      await page.locator(`button[data-session-id="${child}"]`).click();
      await expect(page.getByText('Completed sidebar answer.', { exact: true })).toBeVisible();
      await expect(page.getByText('Unfinished source output.', { exact: true })).toHaveCount(0);
      if (width === 390) await expect(page.locator('[data-harness-frame]')).not.toHaveAttribute('data-sidebar-overlay');
      await send('Fork follow-up'); await expect(page.getByText('Fork kept the completed answer.', { exact: true })).toBeVisible();
      await expect.poll(async () => (await wire.read('session/checkpoint', target(child))).checkpoint.snapshot.attempt?.phase.type).toBe('settled');
      await actions(child); await page.getByRole('menuitem', { name: zh ? '删除会话' : 'Delete Session', exact: true }).click();
      await expect(deletion).toBeVisible(); await deletion.getByRole('button', { name: zh ? '删除' : 'Delete', exact: true }).click(); await expect(deletion).toHaveCount(0);
      await showSidebar(); await expect(page.locator(`button[data-session-id="${child}"]`)).toHaveCount(0);
      await page.locator(`button[data-session-id="${source}"]`).click(); await dismissSidebar();
      await expect(page.getByRole('button', { name: zh ? '停止' : 'Stop', exact: true })).toBeEnabled();
      await page.getByRole('button', { name: zh ? '停止' : 'Stop', exact: true }).click();
      await expect.poll(async () => (await wire.read('session/checkpoint', target(source))).checkpoint.snapshot.attempt?.phase)
        .toMatchObject({ type: 'settled', outcome: { type: 'cancelled' } });
      await fixture.release('sidebar-source-running');
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]); passed = true;
    } finally { await page.close(); await fixture.stop(passed); }
  });
}
