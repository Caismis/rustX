import { expect, test } from '@playwright/test';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';
import { routeWorkspaceHost } from './workspace-host';

test('default foreground Bash promotes the original process after 120 seconds and lets chat continue', async ({ page }) => {
  test.setTimeout(210_000);
  const fixture = await startDogfood('web_bash_promotion');
  const wire = await wireProbe(page);
  let passed = false;
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    const session = await openEmptySession(page, fixture, 'Workspace A');
    const send = async (text: string) => {
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill(text);
      await page.getByRole('button', { name: 'Send', exact: true }).click();
    };
    await send('Run the foreground promotion probe');
    const allow = page.getByRole('button', { name: 'Allow once', exact: true });
    await expect(allow).toBeVisible();
    const approvedAt = performance.now();
    await allow.click();
    const launches = () => readFile(join(fixture.workspaceA, 'promotion.pids'), 'utf8');
    await expect(async () => expect(await launches()).toMatch(/^[1-9]\d*\n$/)).toPass({ timeout: 5000 });
    const pid = (await launches()).trim();
    const trigger = page.locator('[data-session-panel-toggle]');
    const target = await wire.attachedTarget(session);
    const initial = (await wire.read('session/checkpoint', target)).checkpoint.snapshot;
    expect(initial.jobs ?? []).toHaveLength(0);
    const promoted = page.locator('[data-jobs-running], [data-session-panel-section="background-jobs"] [data-job-id]').first();
    await expect(promoted).toBeHidden();
    await expect(promoted).toBeVisible({ timeout: 130_000 });
    const elapsedMs = performance.now() - approvedAt;
    expect(elapsedMs).toBeGreaterThanOrEqual(120_000);
    expect(elapsedMs).toBeLessThan(130_000);
    await expect(page.getByText('The shell is continuing in the background.', { exact: true })).toBeVisible();
    const turn = page.locator('[data-turn-process][aria-expanded]').first();
    if (await turn.getAttribute('aria-expanded') === 'false') await turn.click();
    const call = page.locator('[data-tool-name="bash"][data-tool-call-id]').first();
    const step = page.locator('[data-step-process]').filter({ has: call }).locator(':scope > button[aria-expanded="false"]');
    if (await step.count()) await step.click();
    await expect(call.getByLabel('Tool status', { exact: true })).toHaveText('Moved to background');
    await call.getByRole('button', { expanded: false }).click();
    await expect(call.locator('[data-terminal][data-background]').getByText(`before pid=${pid}`, { exact: true })).toBeVisible();
    await expect(call.locator('[data-terminal] [data-state]')).toHaveCount(0);
    await expect(call.locator('code')).toHaveText(/\.output$/);
    await page.screenshot({ path: test.info().outputPath('bash-promotion.png') });
    await call.getByRole('button', { name: 'Inspect', exact: true }).click();
    await expect(page.getByRole('tab', { name: 'Trajectory', exact: true })).toHaveAttribute('aria-selected', 'true');
    await page.getByRole('tab', { name: 'Result', exact: true }).click();
    await expect(page.getByText(`before pid=${pid}`, { exact: true })).toBeVisible();
    await page.getByRole('tab', { name: 'Chat', exact: true }).click();
    await send('Continue while the shell runs');
    await expect(page.getByText('Chat continued while the shell ran.', { exact: true })).toBeVisible();

    if (await trigger.getAttribute('aria-expanded') !== 'true') await trigger.click();
    const panel = page.locator('[data-session-panel-section="background-jobs"]');
    const row = panel.locator('[data-job-id]');
    await expect(row).toHaveCount(1);
    await expect(row).toHaveAttribute('data-job-state', 'running');
    await row.getByRole('button', { expanded: false }).click();
    await expect(row.getByText(`before pid=${pid}`, { exact: true })).toBeVisible();
    await writeFile(join(fixture.workspaceA, 'promotion.release'), '');
    await expect(row.getByText(`after pid=${pid}`, { exact: true })).toBeVisible();
    await row.getByRole('button', { name: /^Stop task / }).click();
    await expect(row).toHaveAttribute('data-job-state', 'running');
    await row.getByRole('button', { name: 'Click again to confirm', exact: true }).click();
    await expect(panel.locator('[data-job-state="cancelled"]')).toHaveCount(1);
    await expect.poll(() => {
      try { process.kill(Number(pid), 0); return false; }
      catch (error) { if ((error as NodeJS.ErrnoException).code === 'ESRCH') return true; throw error; }
    }).toBe(true);
    await expect(page.getByText('The shell has stopped.', { exact: true })).toBeVisible();
    const notice = page.locator('[data-inbound-source="job"]');
    await notice.getByRole('button', { expanded: false }).click();
    await expect(notice).toContainText(`after pid=${pid}`);
    await expect(notice).not.toContainText('"combined":');
    expect(await launches()).toBe(`${pid}\n`);
    await test.info().attach('promotion', { body: JSON.stringify({ pid, elapsedMs }), contentType: 'application/json' });
    passed = true;
  } finally {
    await writeFile(join(fixture.workspaceA, 'promotion.cleanup'), '');
    await stopDogfoodPage(page, fixture, passed);
  }
});
