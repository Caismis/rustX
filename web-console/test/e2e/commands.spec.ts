import { connectRemote } from './shell-actions';
import { expectSettled } from './shell-actions';
import { sessionTree } from './shell-actions';
import { showInspector } from './shell-actions';
import { chooseWorkspace, connectionAction } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { test, expect } from '@playwright/test';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';

test('typed selectors, native upload-bearing retry branch, original lineage and independent Fork', async ({ page }) => {
  const fixture = await startDogfood('web_commands');
  let passed = false;
  let phase = 'original turn setup';
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  const message = page.getByRole('textbox', { name: 'Message', exact: true });
  const facts = page.getByLabel('Native diagnostic JSON', { exact: true });
  const command = async (name: string) => { await message.fill(`/${name}`); await message.press('Enter'); return page.getByRole('dialog', { name: `/${name}`, exact: true }); };
  const settled = () => expectSettled(page);
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await expect(page).toHaveTitle(/rustX/);
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByLabel('Transport token')).toHaveCount(0); await showInspector(page);
    await chooseWorkspace(page, 'Workspace A');
    await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(message).toBeEnabled();
    const originalId = JSON.parse(await facts.innerText()).SessionId as string;
    const originalConversation = JSON.parse(await facts.innerText()).ConversationId as string;
    await message.fill('/model'); await message.press('Enter');
    let popup = page.getByRole('dialog', { name: '/model', exact: true });
    await popup.getByLabel('Filter options').fill('second');
    await popup.getByRole('option', { name: /fixture\/second-model/ }).click();
    await expect(popup).toHaveCount(0); await expect(message).toHaveValue('');
    await expect(facts).toContainText('fixture/second-model');
    popup = await command('tools');
    await expect(message).toHaveValue(''); await expect(popup).toBeVisible();
    await popup.getByRole('button', { name: 'Close dialog' }).click();
    popup = await command('model'); await popup.getByLabel('Filter options').press('Escape');
    await expect(popup).toHaveCount(0); await expect(message).toBeFocused();
    await expect(message).toHaveValue('/model');
    await connectionAction(page, 'Reconnect');
    await expect(page.getByLabel('Transport token')).toHaveCount(0); await showInspector(page);
    await expect(facts).toContainText('fixture/second-model');
    await message.fill('/not-a-command'); await message.press('Enter');
    await expect(page.getByRole('alert')).toContainText('Unsupported command');
    await expect(message).toHaveValue('/not-a-command');
    await message.fill('Regenerate my uploaded note');
    await page.getByLabel('Attach files').setInputFiles({ name: 'note.txt', mimeType: 'text/plain', buffer: Buffer.from('Owned by native Session') });
    await expect(page.getByText('Uploaded', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByText('Original native answer', { exact: true })).toBeVisible(); await settled();
    phase = 'selecting exact historical Retry boundary';
    await page.getByRole('button', { name: 'Lineage', exact: true }).click();
    await page.getByRole('button', { name: 'Retry / Regenerate', exact: true }).click();
    await page.getByRole('dialog', { name: 'Retry / Regenerate', exact: true }).getByRole('option', { name: /Replay the original input once/ }).click();
    phase = 'awaiting validated retry provider request (branch → switchNode → attach → turn/start)';
    await test.step('native retry reaches the provider with validated editor content', async () => {
      // The gate is reached only after the real provider validates model, native
      // upload-bearing input and absence of old output. Its existing deadline is
      // deadlock protection, not a five-second budget for the entire transition.
      expect(await fixture.gate('retry-request-reached')).toMatchObject({ kind: 'gate_reached', name: 'retry-request-reached', requestIndex: 1 });
      phase = 'validated retry request reached; checking attached native lineage while provider is parked';
      await expect(facts).toContainText(originalId);
      await expect(facts).not.toContainText(`"ConversationId": "${originalConversation}"`);
      expect(JSON.parse(await facts.innerText()).SessionId).toBe(originalId);
      expect(JSON.parse(await facts.innerText()).ConversationId).not.toBe(originalConversation);
      await expect(page.getByLabel('Session status')).toContainText('Working…');
      await expect(page.getByText('Regenerated native answer', { exact: true })).toHaveCount(0);
    });
    phase = 'validated retry request reached; awaiting provider output and canonical settlement';
    await fixture.release('retry-request-reached');
    await expect(page.getByText('Regenerated native answer', { exact: true })).toBeVisible(); await settled();
    phase = 'retry settled; verifying original lineage and independent Fork';
    const transcript = page.getByLabel('Canonical conversation');
    await expect(transcript.getByText('Original native answer', { exact: true })).toHaveCount(0);
    await expect(transcript.getByText('Regenerate my uploaded note', { exact: true })).toHaveCount(1);
    await page.screenshot({ path: 'test-results/commands-retry-desktop.png' });
    await sessionTree(page);
    await page.getByRole('dialog', { name: 'Session tree', exact: true }).getByRole('option').filter({ hasText: originalConversation }).click();
    await expect(transcript.getByText('Original native answer', { exact: true })).toBeVisible();
    await expect(transcript.getByText('Regenerated native answer', { exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Lineage', exact: true }).click();
    await page.getByRole('button', { name: 'Fork to new Session', exact: true }).click();
    await page.getByRole('dialog', { name: '/fork', exact: true }).getByRole('option', { name: /Continue after this response/ }).click();
    await expect(page.getByRole('dialog', { name: '/fork', exact: true })).toHaveCount(0);
    await expect(message).toHaveValue('');
    const forkId = JSON.parse(await facts.innerText()).SessionId as string;
    expect(forkId).not.toBe(originalId);
    await expect(transcript.getByText('Original native answer', { exact: true })).toBeVisible();
    await expect(page.getByText(/Native restored upload batch/)).toHaveCount(0);
    // Native #319 copies uploads in the inherited post-response prefix before publishing the child.
    const root = join(fixture.workspaceA, '.agents/uploads', forkId);
    const batches = readdirSync(root);
    expect(batches).toHaveLength(1);
    expect(readFileSync(join(root, batches[0], 'note.txt'), 'utf8')).toBe('Owned by native Session');
    await connectionAction(page, 'Reconnect');
    await expect(page.getByLabel('Transport token')).toHaveCount(0); await showInspector(page);
    await expect(message).toBeEnabled();
    // Live settings are Conversation-owned. A new cold lineage uses the native
    // Session/launch configuration, never a browser copy of the old live values.
    await expect(facts).toContainText('fixture/console-model');
    await expect(facts).toContainText('policy');
    await page.getByRole('button', { name: 'Close Inspector' }).click();
    await page.setViewportSize({ width: 390, height: 844 });
    popup = await command('model'); await expect(popup.getByRole('option', { name: /fixture\/second-model/ })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: 'test-results/commands-selector-mobile.png' });
    expect(errors).toEqual([]); passed = true;
  } catch (error) {
    // Capture before shutdown: teardown closes the socket and can otherwise
    // obscure whether branch, attach, admission or provider settlement stalled.
    await test.info().attach('retry-native-frontier', {
      contentType: 'application/json',
      body: JSON.stringify({ phase, diagnostics: fixture.diagnostics(),
        facts: await facts.innerText(),
        notices: await page.locator('.notice').allTextContents(),
        wire: await page.locator('.protocol-log pre').allTextContents(),
        provider: await Promise.allSettled(['state', 'requests', 'observations'].map(path => fixture.control(path))),
      }, null, 2),
    });
    throw error;
  } finally {
    const report = await fixture.stop(passed);
    await test.info().attach('retry-native-fixture-report', { contentType: 'application/json',
      body: JSON.stringify({ phase, report, diagnostics: fixture.diagnostics() }, null, 2) });
  }
});
