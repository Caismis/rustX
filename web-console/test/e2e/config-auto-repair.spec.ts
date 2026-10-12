import { expect, test } from '@playwright/test';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { closeSettings, connectRemote, openEmptySession, openWorkspaceSettings, selectedSettingsPage } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';

// Issue #464 against a real App Server: the bound User source is repaired at
// startup, Workspace A before its first Session resolves, and the browser only
// presents the native receipts — a nonblocking notice per receipt identity and
// Settings → Advanced → Auto Repair of the exact owner. The provider scenario
// has no step, so any model request fails the run.
test('automatic configuration repair is announced once and inspected at its exact owner', async ({ page }) => {
  const fixture = await startDogfood('web_config_repair');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  try {
    // Startup already repaired the User document, before policy and transport.
    const user = readFileSync(fixture.settings, 'utf8');
    expect(user).not.toContain('legacy_root');
    expect(user).not.toContain('unknown_permission');
    expect(user).toMatch(/^schema_version = 10\n/);
    const backups = readdirSync(join(fixture.directory, 'runtime/configuration-backups'));
    expect(backups).toHaveLength(1);
    expect(readFileSync(join(fixture.directory, 'runtime/configuration-backups', backups[0]), 'utf8')).toContain('legacy_root = true');

    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    const notice = page.getByRole('status', { name: 'Configuration repair', exact: true });
    await expect(notice).toContainText('Configuration automatically repaired · 3 changes in User configuration');
    // Nonblocking: nothing modal opened and nothing navigated.
    await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toHaveCount(0);
    await notice.screenshot({ path: test.info().outputPath('config-repair-notice-user.png') });
    await notice.getByRole('button', { name: 'View details', exact: true }).click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    await expect(settings).toBeVisible();
    expect(await selectedSettingsPage(page)).toBe('Advanced');
    const section = settings.getByRole('region', { name: 'Auto Repair', exact: true });
    await expect(section.getByRole('status').first()).toHaveText('Automatically repaired: 3 changes committed.');
    for (const line of ['Updated schema_version 8 → 10', 'Removed unknown field legacy_root', 'Removed unknown field agent.plugins.todo.unknown_permission'])
      await expect(section.getByText(line, { exact: true })).toBeVisible();
    await expect(section.getByText('An exact private copy of the original file is kept in rustX storage.')).toBeVisible();
    await section.screenshot({ path: test.info().outputPath('config-repair-advanced-user.png') });
    await closeSettings(page);
    await expect(notice).toHaveCount(0);

    // Workspace A is repaired before its first Session resolves; entering it
    // announces that Workspace receipt to this browser.
    await openEmptySession(page, fixture, 'Workspace A');
    expect(readFileSync(join(fixture.workspaceA, 'rustx.toml'), 'utf8')).toBe('# Workspace A\n[agent]\n');
    await expect(notice).toContainText('Configuration automatically repaired · 1 change in Workspace configuration');
    await notice.getByRole('button', { name: 'View details', exact: true }).click();
    await expect(settings).toBeVisible();
    await expect(settings.getByRole('heading', { name: 'Workspace A' })).toBeVisible();
    expect(await selectedSettingsPage(page)).toBe('Advanced');
    const workspace = settings.getByRole('region', { name: 'Auto Repair', exact: true });
    await expect(workspace.getByText('Removed unknown field agent.future_trust', { exact: true })).toBeVisible();
    await closeSettings(page);

    // A reload is a new connection: its server announces the User receipt after
    // initialize and the Workspace receipt once the Workspace is addressed. On
    // that one socket both notices precede the response of the authoritative
    // Workspace read below, and both receipts stay acknowledged.
    await page.reload();
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openWorkspaceSettings(page, 'Workspace A');
    await settings.getByRole('tab', { name: 'Advanced', exact: true }).click();
    await expect(settings.getByRole('region', { name: 'Auto Repair', exact: true }).getByText('Removed unknown field agent.future_trust', { exact: true })).toBeVisible();
    await expect(page.locator('.repair-notice')).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally { await fixture.stop(); }
});
