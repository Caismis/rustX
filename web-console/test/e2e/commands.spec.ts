import { openEmptySession } from './shell-actions';
import { connectRemote } from './shell-actions';
import { expectSettled } from './shell-actions';
import { sessionTree } from './shell-actions';
import { showInspector } from './shell-actions';
import { connectionAction } from './shell-actions';
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
  const command = async (name: string) => { await message.fill(`/${name}`); await expect(page.getByRole('listbox', { name: 'Commands' }).getByRole('option').first()).toBeVisible(); await message.press('Enter'); return page.getByRole('dialog', { name: `/${name}`, exact: true }); };
  const settled = () => expectSettled(page);
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await expect(page).toHaveTitle(/rustX/);
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByLabel('Transport token')).toHaveCount(0); await showInspector(page);
    await openEmptySession(page, fixture, 'Workspace A');
    await expect(message).toBeEnabled();
    await expect(facts).toContainText(/"ConversationId": "conv_/);
    const originalId = JSON.parse(await facts.innerText()).SessionId as string;
    const originalConversation = JSON.parse(await facts.innerText()).ConversationId as string;
    await message.fill('/model'); await expect(page.getByRole('option', { name: 'Model model', exact: true })).toBeVisible(); await message.press('Enter');
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
    const usageButton = page.getByRole('button', { name: 'Usage 120 tok', exact: true });
    // The inherited response keeps its exact native usage across every lineage.
    const usageDetails = async () => {
      await usageButton.click();
      const detail = page.getByRole('dialog', { name: 'Turn usage', exact: true });
      const text = await detail.locator('[data-turn-usage-details]').innerText();
      await page.keyboard.press('Escape'); await expect(detail).toHaveCount(0);
      return text;
    };
    await usageButton.click();
    const usageDetail = page.getByRole('dialog', { name: 'Turn usage', exact: true });
    await expect(usageDetail.getByText('Output', { exact: true })).toBeVisible();
    await expect(usageDetail.getByText('20 tok', { exact: true })).toBeVisible();
    await expect(usageDetail.getByText('Cached input', { exact: true })).toHaveCount(0);
    await page.keyboard.press('Escape'); await expect(usageDetail).toHaveCount(0);
    const originalUsage = await usageDetails();
    phase = 'regenerating the completed response';
    // A completed response names its exact boundary: Regenerate runs on the click.
    await page.getByRole('button', { name: 'Regenerate', exact: true }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);
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
      await expect(page.getByLabel('Session status')).toHaveCount(0);
    await expect(page.getByRole('button', { name: /^Deep diving/ }).first()).toBeVisible();
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
    await page.getByRole('button', { name: 'Branch into a new Session', exact: true }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    // The Fork runs on the click; the attached Fork replaces the selected Session.
    await expect.poll(async () => JSON.parse(await facts.innerText()).SessionId).not.toBe(originalId);
    await expect(message).toHaveValue('');
    const forkId = JSON.parse(await facts.innerText()).SessionId as string;
    const forkConversation = JSON.parse(await facts.innerText()).ConversationId as string;
    await expect(transcript.getByText('Original native answer', { exact: true })).toBeVisible();
    await expect(page.getByText(/Native restored upload batch/)).toHaveCount(0);
    await expect(page.getByLabel('Completed Turn', { exact: true })).toHaveCount(1);
    await expect(usageButton).toHaveCount(1);
    expect(await usageDetails()).toBe(originalUsage);
    // As a Harness fork folds its copied prefix, the composer's session totals
    // include the inherited turn rather than starting empty.
    await expect(page.getByLabel('Session statistics', { exact: true }).getByRole('button', { name: '120 tok', exact: true })).toBeVisible();
    // Native #319 copies uploads in the inherited post-response prefix before publishing the child.
    const root = join(fixture.workspaceA, '.agents/uploads', forkId);
    const batches = readdirSync(root);
    expect(batches).toHaveLength(1);
    expect(readFileSync(join(root, batches[0], 'note.txt'), 'utf8')).toBe('Owned by native Session');
    await connectionAction(page, 'Reconnect');
    await expect(page.getByLabel('Transport token')).toHaveCount(0); await showInspector(page);
    await expect(message).toBeEnabled();
    // Model selection belongs to Session identity and is inherited by its Fork;
    // cold lineage replacement does not select the global default again.
    await expect(facts).toContainText('fixture/second-model');
    await expect(facts).toContainText('policy');
    // A reopened inherited Assistant remains a valid native continuation anchor.
    await expect(page.getByLabel('Completed Turn', { exact: true })).toHaveCount(1);
    await expect(usageButton).toHaveCount(1);
    expect(await usageDetails()).toBe(originalUsage);
    await page.getByRole('button', { name: 'Regenerate', exact: true }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    expect(await fixture.gate('inherited-retry-reached')).toMatchObject({ kind: 'gate_reached', requestIndex: 2 });
    await fixture.release('inherited-retry-reached');
    await expect(transcript.getByText('Inherited replay answer', { exact: true })).toBeVisible();
    await settled();
    await expect(transcript.getByText('Regenerate my uploaded note', { exact: true })).toHaveCount(1);
    await expect(transcript.getByText('Original native answer', { exact: true })).toHaveCount(0);
    await sessionTree(page);
    await page.getByRole('dialog', { name: 'Session tree', exact: true }).getByRole('option').filter({ hasText: forkConversation }).click();
    await expect(transcript.getByText('Original native answer', { exact: true })).toBeVisible();
    await expect(transcript.getByText('Inherited replay answer', { exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Close Inspector' }).click();
    await page.setViewportSize({ width: 390, height: 844 });
    await usageButton.click();
    await expect(page.getByRole('dialog', { name: 'Turn usage', exact: true })).toBeVisible();
    await page.screenshot({ path: 'test-results/turn-usage-mobile.png' });
    await page.keyboard.press('Escape');
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
    // Close the browser connection before shutting down its Host so reconnect
    // cannot race Playwright context teardown.
    await page.close();
    const report = await fixture.stop(passed);
    await test.info().attach('retry-native-fixture-report', { contentType: 'application/json',
      body: JSON.stringify({ phase, report, diagnostics: fixture.diagnostics() }, null, 2) });
  }
});
