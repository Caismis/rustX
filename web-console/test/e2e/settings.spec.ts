import { expandModelAuthoring } from './shell-actions';
import { openEmptySession } from './shell-actions';
import {
  choose, closeSettings, confirmSettingsAction, connectRemote, openSettingsPage, openWorkspaceSettings, selectedSettingsPage,
} from './shell-actions';
import { expect, test } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';

test('CFG3 atomic Provider and Model editing, Root selections, automatic application and rescan failure', async ({ page }) => {
  const fixture = await startDogfood();
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await openEmptySession(page, fixture, 'Workspace A');
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    // Global Settings opens at General, one of exactly six product pages.
    await expect(page.getByRole('tablist', { name: 'Settings pages' }).getByRole('tab')).toHaveText(['General', 'Models', 'Agent', 'Tools & Permissions', 'Extensions', 'Advanced']);
    expect(await selectedSettingsPage(page)).toBe('General');
    const saved = async (unit: string) => { await expect(settings.getByText(`${unit} saved. Native coordination owns application.`)).toBeVisible(); };
    const row = (name: string) => settings.getByRole('row', { name, exact: true });
    await openSettingsPage(page, 'Models'); await expandModelAuthoring(page);
    await settings.getByLabel('New Provider identity').fill('acceptance');
    await settings.getByRole('button', { name: 'Add Provider', exact: true }).click();
    await settings.getByLabel('Endpoint', { exact: true }).fill('http://127.0.0.1:1/v1');
    await settings.getByLabel('Environment variable', { exact: true }).fill('RUSTX_CONSOLE_FIXTURE_KEY');
    await settings.getByRole('button', { name: 'Save Provider acceptance', exact: true }).click(); await saved('Provider acceptance');
    await settings.screenshot({ path: test.info().outputPath('cfg3-user-provider-pending.png') });
    // Provider → Model drill-down: the new Model is created from its Provider.
    await settings.getByLabel('New Model identity').fill('independent');
    await settings.getByRole('button', { name: 'Add Model to acceptance', exact: true }).click();
    await expect(settings.getByLabel('Provider identity', { exact: true })).toHaveValue('acceptance');
    await settings.getByLabel('Wire model identity').fill('wire-a');
    await settings.getByRole('button', { name: 'Save Model independent', exact: true }).click(); await saved('Model independent');
    await settings.screenshot({ path: test.info().outputPath('cfg3-user-model.png') });
    await closeSettings(page);
    await openWorkspaceSettings(page, 'Workspace A'); await expandModelAuthoring(page);
    // A Workspace surface is constrained and lands on Models.
    expect(await selectedSettingsPage(page)).toBe('Models');
    await expect(page.getByRole('tablist', { name: 'Settings pages' }).getByRole('tab', { name: 'General', exact: true })).toHaveCount(0);
    // The User Model is the native effective definition for this identity, so
    // it is listed in this Workspace although the Workspace authors no override.
    await settings.getByRole('button', { name: /^All Models/ }).click();
    await row('independent').click();
    // The Workspace editor shows that native effective definition while
    // authoring nothing: there is nothing to save until an override is authored.
    await expect(settings.getByLabel('Wire model identity')).toHaveValue('wire-a');
    await expect(settings.getByRole('button', { name: 'Save Model independent', exact: true })).toBeDisabled();
    await settings.getByLabel('Wire model identity').fill('wire-workspace');
    await settings.getByLabel('Provider identity', { exact: true }).fill('acceptance');
    await settings.getByRole('button', { name: 'Save Model independent', exact: true }).click(); await saved('Model independent');
    await settings.getByRole('button', { name: '← Models', exact: true }).click();
    // The inherited User Provider is listed with its native origin.
    await expect(row('acceptance')).toContainText('Inherited from User');
    await row('acceptance').click();
    // A Provider credential is never read back from a shadowed definition, so
    // this editor inherits no authoring state even though the same-name User
    // Provider is the native effective one.
    await expect(settings.getByLabel('Endpoint', { exact: true })).toHaveValue('');
    await settings.getByRole('button', { name: /Credential source$/ }).click();
    await expect(page.getByRole('listbox').getByRole('option')).toHaveText(['Read it from an environment variable', 'Enter a literal secret']);
    await page.keyboard.press('Escape');
    await expect(settings.getByText(/Native effective Provider acceptance/)).toBeVisible();
    await settings.getByLabel('Endpoint', { exact: true }).fill('http://127.0.0.1:2/v1');
    await settings.getByLabel('Environment variable', { exact: true }).fill('RUSTX_CONSOLE_FIXTURE_KEY');
    await settings.getByRole('button', { name: 'Save Provider acceptance', exact: true }).click(); await saved('Provider acceptance');
    // Source revisions are diagnostics: they are on Advanced, not on Models.
    await expect(settings.getByText(/Revision:/)).toHaveCount(0);
    await openSettingsPage(page, 'Advanced');
    await expect(settings.getByText(/Revision:/)).toBeVisible();
    await expect(settings.getByRole('button', { name: 'Adopt prepared context', exact: true })).toHaveCount(0);
    await openSettingsPage(page, 'Tools & Permissions');
    // The User source authors every Native Tool; this Workspace inherits that
    // effective value and authors nothing until an explicit edit.
    await expect(settings.getByLabel('read', { exact: true })).toBeChecked();
    await expect(settings.getByRole('button', { name: 'Save Native Tools', exact: true })).toBeDisabled();
    await settings.getByLabel('read', { exact: true }).uncheck();
    await settings.getByRole('button', { name: 'Save Native Tools', exact: true }).click(); await saved('Native Tools');
    // Source families share exact/all/none selection without activating definitions.
    for (const [family, id] of [['MCP', 'absent-mcp'], ['Managed Python', 'absent-python']]) {
      await choose(settings, 'Source family', family);
      await settings.getByLabel('Source identity', { exact: true }).fill(id);
      await settings.getByRole('button', { name: 'Add source selection', exact: true }).click();
      const name = family === 'Managed Python' ? `python:${id}` : id;
      const source = settings.getByRole('form', { name: `Source ${name}`, exact: true });
      await choose(source, 'Selection', 'All');
      await choose(source, 'Selection', 'Exact identities');
      await source.getByRole('textbox', { name: `${name} identities 1`, exact: true }).fill('inspect');
      await choose(source, 'Selection', 'None');
      await source.getByRole('button', { name: `Save Source ${name}`, exact: true }).click(); await saved(`Source ${name}`);
    }
    await expect(settings).toContainText('not a filesystem ACL');
    await expect(settings).toContainText(`${fixture.workspaceA}/.agents/skills`);
    const skills = settings.getByRole('form', { name: 'Skill visibility', exact: true });
    await choose(skills, 'Selection', 'All');
    await skills.getByRole('button', { name: 'Save Skill visibility', exact: true }).click(); await saved('Skill visibility');
    await expect(settings.getByRole('group', { name: 'agents', exact: true }).getByRole('textbox')).toHaveCount(0);
    // An explicitly empty Workspace allowlist is authored, not inferred: the
    // unit must be overridden before an empty list becomes a written value.
    await expect(settings.getByRole('button', { name: 'Save Agent allowlist', exact: true })).toBeDisabled();
    await settings.getByRole('button', { name: 'Override Agent allowlist', exact: true }).click();
    await settings.getByRole('button', { name: 'Save Agent allowlist', exact: true }).click(); await saved('Agent allowlist');
    await openSettingsPage(page, 'Extensions');
    await settings.getByRole('tab', { name: 'Native', exact: true }).click();
    await expect(settings.getByRole('switch', { name: 'Enable Todo', exact: true })).not.toBeChecked();
    // The visually hidden switch input is operated through its visible label.
    await settings.getByRole('switch', { name: 'Enable Todo', exact: true }).locator('xpath=ancestor::label').click();
    await expect(settings.getByRole('switch', { name: 'Enable Todo', exact: true })).toBeChecked();
    await settings.getByRole('button', { name: 'Save Todo extension', exact: true }).click(); await saved('Todo extension');
    await settings.screenshot({ path: test.info().outputPath('cfg3-native-extensions.png') });
    await expect(settings.getByRole('button', { name: 'Adopt prepared context', exact: true })).toHaveCount(0);
    await openSettingsPage(page, 'Advanced');
    const authored = readFileSync(fixture.settings, 'utf8');
    writeFileSync(fixture.settings, 'invalid = [');
    await settings.getByRole('button', { name: 'Rescan configuration files', exact: true }).click();
    await expect(settings).toContainText('Source cannot be resolved');
    await settings.getByRole('button', { name: 'Source and application diagnostics', exact: true }).click();
    await expect(settings).toContainText('invalid rustx.toml');
    await settings.screenshot({ path: test.info().outputPath('cfg3-application-failed.png') });
    writeFileSync(fixture.settings, authored);
    await settings.getByRole('button', { name: 'Rescan configuration files', exact: true }).click();
    await expect(settings.getByText(/Revision:/)).toBeVisible();
    // Independent removal of each Workspace override is revision fenced, and
    // it is confirmed as what it is: inheriting the global definition again.
    await openSettingsPage(page, 'Models'); await expandModelAuthoring(page);
    await settings.getByRole('button', { name: '← Models', exact: true }).click();
    await settings.getByRole('button', { name: /^All Models/ }).click();
    await row('independent').click();
    await confirmSettingsAction(page, 'Use global default Model independent'); await saved('Model independent');
    await settings.getByRole('button', { name: '← Models', exact: true }).click();
    await row('acceptance').click();
    await confirmSettingsAction(page, 'Use global default Provider acceptance'); await saved('Provider acceptance');
    // Removing the Workspace override returns the identity to the inherited
    // User definition instead of deleting it from this catalog.
    await settings.getByRole('button', { name: '← Models', exact: true }).click();
    await expect(row('acceptance')).toContainText('Inherited from User');
    expect(errors).toEqual([]);
  } finally { const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});
