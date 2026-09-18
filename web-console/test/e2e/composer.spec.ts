import { connectRemote } from './shell-actions';
import { chooseWorkspace, connectionAction } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { expect, test, type Locator } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { startDogfood } from './dogfood-server';
import { wireProbe } from './wire-probe';

test('native Todo, Goal and Queue docks follow the real App Server through control, loss and reload', async ({ page }) => {
  const fixture = await startDogfood('web_composer_context');
  const wire = await wireProbe(page);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  const connect = async () => {
    await connectRemote(page, `${fixture.endpoint}/`, fixture.token);
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
  };
  const message = page.getByRole('textbox', { name: 'Message', exact: true });
  const todo = page.getByRole('region', { name: 'To-dos' });
  const goal = page.getByRole('region', { name: 'Goal' });
  const queue = page.getByRole('region', { name: 'Queue' });
  const revision = async () => Number(wire.responses.filter(row => row.result?.snapshot?.goal?.current).at(-1)?.result.snapshot.goal.current.reference.revision);
  const aligned = async (docks: Locator[]) => {
    // ResizeObserver and the Harness grid transition settle independently of
    // the viewport call. Observe the geometry contract, never sleep for it.
    for (const dock of docks) {
      await expect.poll(async () => {
        const card = (await page.locator('[data-composer-card]').boundingBox())!;
        const box = (await dock.boundingBox())!;
        return Math.max(Math.abs(box.width - (card.width - 32)), Math.abs(box.x + box.width / 2 - (card.x + card.width / 2)));
      }).toBeLessThanOrEqual(1);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  };
  let passed = false;
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await connect();
    await chooseWorkspace(page, 'Workspace A');
    await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(page.getByLabel('Session location', { exact: true })).toHaveText(`${fixture.workspaceA}`);
    // Composed Todo with no current list is its own bounded fact; no Goal and no queue take space.
    await expect(todo).toHaveAttribute('data-todo-state', 'empty');
    await expect(goal).toHaveCount(0); await expect(queue).toHaveCount(0);

    await message.fill('Plan the composer docks'); await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByText('Plan recorded.', { exact: true })).toBeVisible();
    await expect(todo).toHaveAttribute('data-todo-state', 'current');
    await expect(todo.getByRole('button', { expanded: false })).toContainText(/1 in progress\s·\s1 pending/);
    await todo.getByRole('button', { expanded: false }).click();
    await expect(todo.locator('li')).toHaveCount(2);
    await expect(todo.locator('li').nth(0)).toHaveAttribute('data-status', 'in_progress');
    await expect(todo.locator('li').nth(0)).toContainText('Binding native Todo');
    await expect(todo.locator('li').nth(1)).toContainText('after #1');

    await message.fill('Keep working until the docks are verified'); await page.getByRole('button', { name: 'Send', exact: true }).click();
    // The model created the Goal inside this Human attempt, which started no
    // nested execution. After that attempt settled, the ordinary admission
    // owner admitted exactly one autonomous round; its provider response is
    // held open here.
    await fixture.gate('goal-round');
    await expect(goal).toContainText('Active Goal');
    await expect(goal).toContainText('Verify the composer docks');
    await expect(goal).toContainText('1/1 rounds');
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
    await message.fill('Queued during the Goal round'); await page.getByRole('button', { name: 'Queue', exact: true }).click();
    await expect(queue.locator('[data-inbound-sequence]')).toContainText('Queued during the Goal round');
    await expect(queue.locator('[data-submission-echo]')).toHaveCount(0);
    const order = () => page.locator('[data-composer-context-stack] > *').evaluateAll(nodes => nodes.map(node => node.getAttribute('aria-label') ?? (node.querySelector('[data-composer-card]') ? 'Composer' : 'unknown')));
    expect(await order()).toEqual(['To-dos', 'Goal', 'Queue', 'Composer']);
    await aligned([todo, goal, queue]);

    // Native mutation while the provider gate prevents claim. Both controls
    // must converge by authoritative reread; no extra provider step is allowed.
    await queue.getByRole('button', { name: 'Edit', exact: true }).click();
    await queue.getByRole('textbox', { name: 'Edit queued message' }).fill('Edited during the Goal round');
    await queue.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(queue.locator('[data-inbound-sequence]')).toContainText('Edited during the Goal round');
    await expect(queue.getByRole('textbox')).toHaveCount(0);
    await queue.getByRole('button', { name: 'Remove', exact: true }).click();
    await expect(queue).toHaveCount(0);
    await message.fill('Queued during the Goal round'); await page.getByRole('button', { name: 'Queue', exact: true }).click();
    await expect(queue.locator('[data-inbound-sequence]')).toContainText('Queued during the Goal round');
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.locator('[data-harness-frame]')).toHaveAttribute('data-sidebar-collapsed', 'true');
    await aligned([todo, goal, queue]);
    await expect(queue.getByRole('button', { name: 'Edit', exact: true })).toBeVisible();
    await expect(queue.getByRole('button', { name: 'Remove', exact: true })).toBeVisible();
    await page.screenshot({ path: 'test-results/queue-mobile.png', fullPage: true });
    await page.setViewportSize({ width: 1280, height: 900 });
    await queue.getByRole('button', { name: 'Edit', exact: true }).click();
    await queue.getByRole('textbox', { name: 'Edit queued message' }).fill('Draft that must not change admitted work');
    await fixture.release('goal-round');
    await expect(page.getByText('Queued input handled.', { exact: true })).toBeVisible();
    // Claim won while the editor was open. Preserve the draft, disable Save,
    // and never send it into canonical or already-admitted work.
    await expect(queue.getByRole('textbox', { name: 'Edit queued message' })).toHaveValue('Draft that must not change admitted work');
    await expect(queue.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
    await expect(queue.getByRole('button', { name: 'Remove', exact: true })).toHaveCount(0);
    await queue.getByRole('button', { name: 'Cancel edit', exact: true }).click();
    await expect(queue).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeVisible();

    // Issue #351: Pause and Resume are the complete lifecycle vocabulary —
    // one control at a time, derived from the durable phase, with no separate
    // arm/play step and no "Inactive Goal". Resume restores Active; because
    // the autonomous budget is already exhausted, no provider work follows.
    let before = await revision();
    await goal.getByRole('button', { name: 'Pause goal' }).click();
    await expect(goal).toContainText('Paused Goal'); expect(await revision()).toBe(before + 1);
    await expect(goal.getByRole('button', { name: 'Pause goal' })).toHaveCount(0);
    await goal.getByRole('button', { name: 'Resume goal' }).click();
    await expect(goal).toContainText('Active Goal'); expect(await revision()).toBe(before + 2);
    // Active offers Pause and nothing else: no second activation control.
    await expect(goal.getByRole('button', { name: 'Resume goal' })).toHaveCount(0);
    await expect(goal.locator('[data-goal-armed]')).toHaveCount(0);
    await goal.getByRole('button', { name: 'Pause goal' }).click();
    await expect(goal).toContainText('Paused Goal');
    before = await revision();
    await goal.getByRole('button', { name: 'Edit round budget' }).click();
    const budget = goal.getByRole('spinbutton', { name: 'Autonomous round budget' });
    await expect(budget).toBeFocused(); await budget.fill('3'); await budget.press('Enter');
    await expect(goal).toContainText('1/3 rounds'); expect(await revision()).toBe(before + 1);
    await goal.getByRole('button', { name: 'Edit goal objective' }).click();
    const objective = goal.getByRole('textbox', { name: 'Goal objective' });
    await objective.fill('Verify the composer docks end to end'); await objective.press('Enter');
    await expect(goal).toContainText('Verify the composer docks end to end');
    await expect(goal.getByRole('button', { name: 'Edit goal objective' })).toBeFocused();
    const settled = await revision();

    // Current domain state never becomes configuration or browser recovery input.
    const settings = readFileSync(fixture.settings, 'utf8');
    expect(settings).toContain('[agent.plugins.goal]\nenabled = true');
    expect(settings).not.toMatch(/Verify the composer docks|Bind native Todo|Queued during/);
    expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toMatch(/Verify the composer docks|Bind native Todo|Queued during/);

    await connectionAction(page, 'Disconnect');
    await expect(page.getByLabel('Session status')).toContainText(/Connection interrupted|Needs verification/);
    await expect(goal.getByRole('button', { name: 'Resume goal' })).toBeDisabled();
    await connectionAction(page, 'Reconnect');
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await expect(goal).toContainText('Paused Goal'); await expect(goal).toContainText('1/3 rounds');
    expect(await revision()).toBe(settled);
    await expect(todo.locator('li')).toHaveCount(2);

    await page.reload(); await connect();
    // Reload rebuilds from the authoritative snapshot; disclosure state is presentation-local.
    await expect(todo.getByRole('button', { expanded: false })).toContainText(/1 in progress\s·\s1 pending/);
    await expect(goal).toContainText('Verify the composer docks end to end');
    expect(await revision()).toBe(settled);
    await expect(queue).toHaveCount(0);
    await aligned([todo, goal]);
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.locator('[data-harness-frame]')).toHaveAttribute('data-sidebar-collapsed', 'true');
    await aligned([todo, goal]);
    await expect(goal.getByRole('button', { name: 'Resume goal' })).toBeVisible();
    await page.screenshot({ path: 'test-results/composer-mobile.png', fullPage: true });
    expect(errors).toEqual([]); passed = true;
  } catch (error) { console.error(fixture.diagnostics(), await page.locator('.notice').allTextContents()); throw error; }
  finally { await page.close(); await fixture.stop(passed); }
});
