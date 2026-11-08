import { expect, test } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { returnHome, connectRemote, openEmptySession, openSettingsPage, openWorkspaceSettings } from './shell-actions';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';

test('parseable invalid Model orders expose whole-source repair and restore native editing', async ({ page }) => {
  const fixture = await startDogfood();
  const source = join(fixture.workspaceA, 'rustx.toml');
  const document = (second: string) => ['a', 'b'].map(id => `
[models.${id}]
provider = "fixture"
id = "console-model"
context_window = 128000
max_output_tokens = 4096
capabilities = { input_modalities = ["text"], output_modalities = ["text"] }
default_profile = "high"
profiles = { low = {}, high = {} }
reasoning_profile_order = [${id === 'a' ? '"low", "high"' : second}]
`).join('\n');
  const invalid = document('"low"').replace('["low", "high"]', '["low"]');
  writeFileSync(source, invalid);
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await returnHome(page);
    await openWorkspaceSettings(page, 'Workspace A');
    await openSettingsPage(page, 'Models');
    const settings = page.getByRole('region', { name: 'Settings', exact: true });
    const repair = settings.getByRole('form', { name: 'Repair invalid source', exact: true });
    await expect(repair).toBeVisible();
    await expect(settings.getByRole('alert')).toContainText('invalid Model reasoning Profile order at models.*.reasoning_profile_order');
    await expect(settings.getByRole('button', { name: 'Add Model', exact: true })).toHaveCount(0);
    await repair.getByLabel('Replacement TOML').fill(document('"low"'));
    await repair.getByRole('button', { name: 'Save Repair invalid source', exact: true }).click();
    await expect(repair.getByRole('alert')).toBeVisible();
    expect(readFileSync(source, 'utf8')).toBe(invalid);
    await repair.getByLabel('Replacement TOML').fill(document('"high", "low"'));
    await repair.getByRole('button', { name: 'Save Repair invalid source', exact: true }).click();
    await expect(repair).toHaveCount(0);
    await settings.getByRole('button', { name: 'Reload configuration', exact: true }).click();
    await settings.getByRole('button', { name: /^All Models/ }).click();
    await settings.getByRole('grid', { name: 'All Models', exact: true }).getByRole('row', { name: 'b', exact: true }).click();
    const modal = page.getByRole('dialog', { name: 'Model b', exact: true });
    await modal.getByRole('button', { name: 'Advanced configuration', exact: true }).click();
    const list = modal.getByRole('list', { name: 'Reasoning order (low to high)' });
    expect(await list.locator('li').evaluateAll(rows => rows.map(row => row.getAttribute('data-profile-order')))).toEqual(['high', 'low']);
    for (const id of ['high', 'low']) await expect(modal.getByRole('button', { name: `Profile ${id}`, exact: true })).toBeVisible();
    await modal.getByRole('button', { name: 'Move low up', exact: true }).click();
    await modal.getByRole('button', { name: 'Save Model b', exact: true }).click();
    await expect(page.getByText('Model b saved.', { exact: true })).toBeVisible();
    expect(readFileSync(source, 'utf8')).toMatch(/\[models\.b\][\s\S]*?reasoning_profile_order = \[\s*"low",\s*"high",?\s*\]/s);
  } finally { const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});

for (const width of [1440, 390]) test(`reasoning order ${width}: native save, reload, reset, cancel and clear`, async ({ page }) => {
  await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
  const fixture = await startDogfood();
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', event => { if (event.type() === 'error') errors.push(event.text()); });
  const source = join(fixture.workspaceA, 'rustx.toml');
  writeFileSync(source, `
[models.ordered]
provider = "fixture"
id = "console-model"
context_window = 128000
max_output_tokens = 4096
capabilities = { input_modalities = ["text"], output_modalities = ["text"] }
default_profile = "a-high"
profiles = { a-high = { request_params = '{"effort":"high"}' }, z-off = { max_output_tokens = 512, request_params = '{"effort":"off"}' } }
`);
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await expect(page).toHaveTitle(/rustX/);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await returnHome(page);
    const expand = page.getByRole('button', { name: 'Home', exact: true });
    if (await expand.isVisible()) await expand.click();
    await openWorkspaceSettings(page, 'Workspace A');
    const settings = page.getByRole('region', { name: 'Settings', exact: true });
    await settings.getByRole('button', { name: /^All Models/ }).click();
    const open = async () => {
      await settings.getByRole('grid', { name: 'All Models', exact: true }).getByRole('row', { name: 'ordered', exact: true }).click();
      await page.getByRole('dialog', { name: 'Model ordered', exact: true }).getByRole('button', { name: 'Advanced configuration', exact: true }).click();
    };
    await open();
    const modal = page.getByRole('dialog', { name: 'Model ordered', exact: true });
    const list = modal.getByRole('list', { name: 'Reasoning order (low to high)' });
    const ids = () => list.locator('li').evaluateAll(rows => rows.map(row => row.getAttribute('data-profile-order')));
    const save = async () => {
      await modal.getByRole('button', { name: 'Save Model ordered', exact: true }).click();
      await expect(page.getByText('Model ordered saved.', { exact: true })).toBeVisible();
      await expect(modal.getByRole('button', { name: 'Save Model ordered', exact: true })).toBeDisabled();
    };
    await expect(modal.getByText('Not configured', { exact: true })).toBeVisible();
    expect(readFileSync(source, 'utf8')).not.toContain('reasoning_profile_order');
    await modal.getByRole('button', { name: 'Configure reasoning order', exact: true }).click();
    expect(await ids()).toEqual(['a-high', 'z-off']);
    const up = modal.getByRole('button', { name: 'Move z-off up', exact: true });
    await up.focus(); await up.press('Enter');
    await expect(up).toBeFocused();
    expect(await ids()).toEqual(['z-off', 'a-high']);
    // Reorder leaves the current invalid parameter buffer mounted and unsaved.
    const json = modal.getByRole('textbox', { name: 'Profile parameters (JSON)', exact: true });
    await json.fill('{"request_params":');
    await modal.getByRole('button', { name: 'Move a-high up', exact: true }).click();
    await expect(json).toHaveText('{"request_params":', { useInnerText: true });
    await modal.getByRole('button', { name: 'Profile z-off', exact: true }).click();
    await expect(json).toHaveText('{"request_params":', { useInnerText: true });
    await modal.getByRole('button', { name: 'Reset form', exact: true }).click();
    await expect(list).toHaveCount(0);
    await modal.getByRole('button', { name: 'Configure reasoning order', exact: true }).click();
    const custom = 'custom-profile-with-an-exceptionally-long-independent-identity-for-keyboard-review';
    await modal.getByRole('button', { name: 'Add profile', exact: true }).click();
    await modal.getByLabel('New profile identity').fill(custom);
    await modal.getByLabel('New profile identity').press('Enter');
    expect(await ids()).toEqual(['a-high', 'z-off', custom]);
    await expect(modal.getByText(`Profile ${custom} was placed last in this draft. Adjust its position before Save.`, { exact: true })).toBeVisible();
    await list.scrollIntoViewIfNeeded();
    await page.screenshot({ path: test.info().outputPath(`reasoning-order-long-${width}.png`) });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await modal.getByRole('button', { name: `Delete profile ${custom}`, exact: true }).click();
    await up.click(); await save();
    const written = readFileSync(source, 'utf8');
    expect(written).toMatch(/reasoning_profile_order = \[\s*"z-off",\s*"a-high",?\s*\]/s);
    expect(written).toContain('default_profile = "a-high"');
    await modal.getByRole('button', { name: 'Close', exact: true }).click();
    await settings.getByRole('button', { name: 'Reload configuration', exact: true }).click();
    await open(); expect(await ids()).toEqual(['z-off', 'a-high']);
    await modal.getByRole('button', { name: 'Move a-high up', exact: true }).click();
    await modal.getByRole('button', { name: 'Delete profile z-off', exact: true }).click();
    await modal.getByRole('button', { name: 'Cancel', exact: true }).click();
    await open(); expect(await ids()).toEqual(['z-off', 'a-high']);
    await expect(modal.getByRole('button', { name: 'Profile z-off', exact: true })).toBeVisible();
    await list.scrollIntoViewIfNeeded();
    await page.screenshot({ path: test.info().outputPath(`reasoning-order-${width}.png`) });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await modal.getByRole('button', { name: 'Clear reasoning order', exact: true }).click();
    await save();
    const cleared = readFileSync(source, 'utf8');
    expect(cleared).not.toContain('reasoning_profile_order');
    for (const retained of ['a-high', 'z-off', '512', 'effort']) expect(cleared).toContain(retained);
    // The final deletion removes the order and Profile collection together.
    await modal.getByRole('button', { name: 'Configure reasoning order', exact: true }).click();
    await modal.getByRole('button', { name: 'Delete profile a-high', exact: true }).click();
    expect(await ids()).toEqual(['z-off']);
    await modal.getByRole('button', { name: 'Delete profile z-off', exact: true }).click();
    await expect(list).toHaveCount(0);
    await expect(modal.getByText('Add Profiles before configuring reasoning order.')).toBeVisible();
    await save();
    const empty = readFileSync(source, 'utf8');
    expect(empty).not.toContain('reasoning_profile_order');
    expect(empty).not.toContain('default_profile');
    expect(empty).not.toContain('.profiles');
    expect(errors).toEqual([]);
  } finally { const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});
