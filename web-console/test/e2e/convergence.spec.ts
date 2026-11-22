import { AppServerHost } from '../../../tui/src/app-server/host';
import { test, expect } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, openWorkspaceSettings, openSettingsPage, returnHome } from './shell-actions';
import { wireProbe } from './wire-probe';
import { transitionFacts } from '../../../protocol/app-server/projection';
import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import type { Notification, RuntimeClientEvent } from '../../../protocol/app-server/v81';

function gate() { let resolve!: () => void; const promise = new Promise<void>(done => { resolve = done; }); return { promise, resolve }; }

test('Harness New Conversation, permission, model, native process and Models convergence', async ({ page }) => {
  const fixture = await startDogfood('web_harness_convergence');
  let observeNotification: ((notification: Notification) => void) | undefined;
  let observeRetirement: (() => void) | undefined;
  const wire = await wireProbe(page, {
    notification: notification => observeNotification?.(notification),
    retired: () => observeRetirement?.(),
  });
  let passed = false;
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const shot = async (name: string) => {
    expect((await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze()).violations).toEqual([]);
    await page.screenshot({ path: test.info().outputPath(`${name}.png`), fullPage: true });
  };
  const noOverflow = async () => expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  try {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.addInitScript(() => localStorage.setItem('rustx-appearance-v1', 'dark'));
    const beforeWrite = gate(), releaseWrite = gate();
    const observedWrite = gate(), releaseObservation = gate();
    await routeWorkspaceHost(page, fixture, {
      before: async () => { beforeWrite.resolve(); await releaseWrite.promise; },
      observed: async () => { observedWrite.resolve(); await releaseObservation.promise; },
    }); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByRole('region', { name: 'New Conversation', exact: true })).toBeVisible();
    expect(wire.requests.filter(r => r.method === 'thread/start')).toHaveLength(0);
    await page.getByRole('button', { name: 'Choose Workspace', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Workspace A', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Workspace permissions', exact: true })).toBeEnabled();
    await shot('01-new-conversation-desktop-dark');
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.locator('[data-harness-frame]')).toHaveAttribute('data-sidebar-collapsed', 'true');
    await expect(page.locator('[data-harness-frame]')).toHaveAttribute('data-sidebar-collapsed', 'true');
    await noOverflow(); await shot('02-new-conversation-mobile-dark');
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.getByRole('button', { name: 'Choose Workspace', exact: true }).click();
    await shot('03-workspace-picker');
    await page.getByRole('menuitem', { name: '+ Add Workspace', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Add Workspace', exact: true })).toBeVisible();
    await shot('03-add-workspace');
    const adoption = page.waitForResponse(response => response.url().endsWith('/product-host/adopt'));
    await page.getByRole('dialog', { name: 'Add Workspace', exact: true }).getByRole('button', { name: 'Workspace A', exact: true }).click();
    expect((await adoption).ok()).toBe(true);
    await expect(page.getByRole('dialog', { name: 'Add Workspace', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Choose Workspace', exact: true })).toContainText('Workspace A');
    await page.getByRole('button', { name: 'Choose Workspace', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Workspace A', exact: true }).click();
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Converge the conversation');
    const residentInput = await page.getByRole('textbox', { name: 'Message', exact: true }).elementHandle();
    const residentSeat = await page.locator('[data-resident-composer]').elementHandle();
    await page.getByRole('button', { name: 'Add', exact: true }).click();
    await expect(page.getByRole('listbox', { name: 'Commands' })).toBeVisible();
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveValue('Converge the conversation');
    await page.getByRole('textbox', { name: 'Message', exact: true }).press('Escape');
    await page.getByRole('button', { name: 'Choose Workspace', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Workspace B', exact: true }).click();
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveValue('Converge the conversation');
    await page.getByRole('button', { name: 'Choose Workspace', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Workspace A', exact: true }).click();
    expect(wire.requests.filter(r => r.method === 'thread/start')).toHaveLength(0);
    const permissions = page.getByRole('button', { name: 'Workspace permissions', exact: true });
    await expect(permissions).toBeEnabled(); await permissions.focus(); await page.keyboard.press('Enter');
    await shot('04-permission-menu');
    await page.getByRole('menuitem', { name: 'Full access', exact: true }).focus(); await page.keyboard.press('Enter');
    const risk = page.getByRole('dialog', { name: 'Enable full access?', exact: true });
    await expect(risk).toBeVisible(); await shot('05-elevated-confirmation');
    await risk.getByRole('checkbox').focus(); await page.keyboard.press('Space');
    await risk.getByRole('button', { name: 'Enable full access', exact: true }).focus(); await page.keyboard.press('Enter');
    await beforeWrite.promise;
    await expect(permissions).toHaveText('Full access');
    const send = page.getByRole('button', { name: 'Send', exact: true });
    const noAdmission = () => { expect(wire.requests.filter(r => r.method === 'thread/start' || r.method === 'inbound/queue')).toHaveLength(0); };
    await expect(send).toBeDisabled();
    await page.getByRole('textbox', { name: 'Message', exact: true }).press('Enter'); noAdmission();
    const permissionSeat = page.locator('[aria-busy="true"]').filter({ has: permissions }).last();
    await expect(permissionSeat).toHaveAttribute('aria-busy', 'true');
    await expect(permissionSeat.locator('small')).toHaveCount(0);
    await expect(page.getByText('Applying Workspace permission…', { exact: true })).toHaveCount(0);
    await shot('05-permission-write-pending');
    releaseWrite.resolve(); await observedWrite.promise;
    // Native write and Host reread have completed, but their authoritative
    // result has not reached the Settings actor. Admission must remain closed.
    await expect(send).toBeDisabled();
    await expect(page.getByText('Saved; awaiting authoritative observation.', { exact: true })).toHaveCount(0);
    await page.getByRole('textbox', { name: 'Message', exact: true }).press('Enter'); noAdmission();
    releaseObservation.resolve();
    await expect(permissions).toContainText('Full access');
    await expect(send).toBeEnabled();
    const model = page.getByRole('button', { name: 'Model and thinking effort', exact: true });
    await expect(model).toBeEnabled(); await model.click();
    await page.getByRole('menuitem', { name: 'Model', exact: true }).click();
    await shot('06-new-conversation-model-picker');
    await page.getByRole('menuitemradio', { name: 'fixture/second-model', exact: true }).click();
    expect(wire.requests.filter(r => r.method === 'thread/start')).toHaveLength(0);
    expect(wire.requests.filter(r => r.method === 'configuration/sourceWrite' && r.params.mutation?.unit === 'default_model')).toHaveLength(0);
    // Hold the real creation acknowledgement until the fixture's native manual
    // label commits. The browser still performs all creation/admission work.
    wire.holdNext('thread/start');
    const startupBanners = await page.evaluateHandle(() => {
      const labels = new Set<string>();
      const observer = new MutationObserver(() => {
        document.querySelectorAll('#session-view [aria-label="Session status"], #session-view [aria-label="Session configuration"]').forEach(node => labels.add(node.textContent ?? ''));
      });
      observer.observe(document.querySelector('#session-view')!, { subtree: true, childList: true, characterData: true });
      return () => { observer.disconnect(); return [...labels]; };
    });
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect.poll(() => wire.held()).toBe(1);
    const created = wire.responses.find(row => row.method === 'thread/start')!.result.session;
    const named = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    try { await named.renameSession(created.id, 'New session'); }
    finally { await named.shutdown(); }
    wire.releaseHeld();
    const target = await wire.attachedTarget(created.id);
    // Creation ACK is not execution settlement. Observe the real browser
    // target's same Attempt before applying the unchanged rendering deadline.
    type Settled = Extract<RuntimeClientEvent, { type: 'attempt_settled' }>;
    const completed = new Promise<Settled>((resolve, reject) => {
      let attemptId: string | undefined;
      observeRetirement = () => reject(new Error('Browser transport retired before Attempt settlement.'));
      observeNotification = notification => {
        if (notification.method === 'session/closed' && sameTarget(notification.params.target, target)) {
          reject(new Error('Native target closed before Attempt settlement.'));
        }
        if (notification.method !== 'session/event' || !sameTarget(notification.params.target, target)) return;
        for (const fact of transitionFacts(notification.params.event)) {
          if (fact.type === 'attempt_started') {
            if (attemptId !== undefined) reject(new Error('Expected exactly one admitted Attempt.'));
            attemptId = fact.attempt_id;
          }
          if (fact.type === 'attempt_settled' && fact.attempt_id === attemptId) resolve(fact);
        }
      };
      // Install live observation and inspect already forwarded native facts in
      // the same synchronous turn, including an immediately completed Attempt.
      for (const notification of wire.notifications) observeNotification(notification as Notification);
    });
    try { expect((await completed).outcome).toMatchObject({ type: 'completed', finish_reason: { type: 'stop' } }); }
    finally { observeNotification = undefined; observeRetirement = undefined; }
    await expect(page.getByRole('heading', { name: 'Workspace ready' })).toBeVisible();
    expect(await startupBanners.evaluate(finish => finish())).toEqual([]);
    expect(await residentInput!.evaluate(node => node === document.querySelector('textarea[aria-label="Message"]'))).toBe(true);
    expect(await residentSeat!.evaluate(node => node === document.querySelector('[data-resident-composer]'))).toBe(true);
    await expect(page.getByLabel('Session location', { exact: true })).toHaveCount(0);
    await expect(page.getByText(/^(Creating|Attaching)…$/)).toHaveCount(0);
    expect(wire.requests.filter(r => r.method === 'thread/start')).toHaveLength(1);
    expect(wire.requests.filter(r => r.method === 'inbound/queue')).toHaveLength(1);
    const admitted = wire.notifications.filter(n => n.method === 'session/event' && transitionFacts(n.params.event).some(fact => fact.type === 'attempt_started'));
    expect(admitted).toHaveLength(1);
    const started = transitionFacts(admitted[0].params.event).find(fact => fact.type === 'attempt_started');
    if (started?.type !== 'attempt_started' || !started.execution_settings) throw new Error('Missing original admitted Attempt execution settings.');
    expect(started.execution_settings.approval_mode).toBe('full_access');
    const methods = wire.requests.map(r => r.method);
    expect(wire.requests.find(request => request.method === 'thread/start')!.params.settings.model).toMatchObject({ model: 'fixture/second-model' });
    expect(methods.filter(method => method === 'session/setModel')).toHaveLength(0);
    const process = page.locator('[data-turn-process]');
    await expect(process).toHaveAttribute('aria-expanded', 'false');
    await expect(page.getByRole('note', { name: 'Agent Status' })).toHaveCount(0);
    await shot('07-process-folded');
    await process.focus(); await page.keyboard.press('Enter');
    for (const reasoning of await page.getByRole('button', { name: /^Reasoning/ }).all()) await reasoning.click();
    await expect(page.getByRole('note', { name: 'Agent Status' }).first()).toBeVisible();
    expect(await page.locator('[aria-label="Canonical conversation"]').evaluate(root => { const disclosure = root.querySelector('[data-turn-process]')!; return [...root.querySelectorAll('[role="note"]')].every(note => !!(disclosure.compareDocumentPosition(note) & Node.DOCUMENT_POSITION_FOLLOWING)); })).toBe(true);
    await shot('08-process-expanded');
    const bash = page.locator('[data-tool-call-id="bash-402"]');
    // The settled Turn groups its tool steps, collapsed as in Harness.
    const steps = page.locator('[data-step-process]').filter({ has: bash }).locator(':scope > button');
    await expect(steps).toHaveAttribute('aria-expanded', 'false'); await steps.click();
    await bash.getByRole('button').first().click(); await shot('09-terminal');
    const write = page.locator('[data-tool-call-id="write-402"]');
    await write.getByRole('button').first().click(); await shot('10-write-diff');
    await openWorkspaceSettings(page, 'Workspace A'); await openSettingsPage(page, 'Models');
    await shot('11-models-desktop');
    await page.setViewportSize({ width: 390, height: 844 }); await noOverflow(); await shot('12-models-mobile');
    expect((await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze()).violations).toEqual([]);
    await returnHome(page); await noOverflow();
    expect(errors).toEqual([]); passed = true;
  } finally { await stopDogfoodPage(page, fixture, passed); }
});

test('changing permissions in an existing idle session keeps the header and sidebar stable', async ({ page }) => {
  const fixture = await startDogfood('web_artifact_document');
  const releaseWrite = gate(), releaseObservation = gate(), observed = gate();
  let passed = false;
  try {
    await routeWorkspaceHost(page, fixture, {
      before: () => releaseWrite.promise,
      observed: async () => { observed.resolve(); await releaseObservation.promise; },
    });
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    const id = await openEmptySession(page, fixture, 'Workspace A');
    const permission = page.getByRole('button', { name: 'Workspace permissions', exact: true });
    await expect(permission).toHaveText('Plan');
    const finish = await page.evaluateHandle(id => {
      const flashes = new Set<string>();
      const sample = () => {
        for (const banner of document.querySelectorAll('#session-view [aria-label="Session configuration"], #session-view [aria-label="Session status"]')) {
          flashes.add(banner.textContent ?? '');
        }
        const row = document.querySelector(`button[data-session-id="${id}"]`)?.closest('[role="treeitem"]');
        if (row?.textContent?.includes('Observation unavailable')) flashes.add('Observation unavailable');
      };
      const observer = new MutationObserver(sample);
      observer.observe(document.body, { childList: true, subtree: true, characterData: true });
      return () => { sample(); observer.disconnect(); return [...flashes]; };
    }, id);
    await permission.click();
    await page.getByRole('menuitem', { name: 'Automatic', exact: true }).click();
    await expect(permission).toHaveText('Automatic');
    releaseWrite.resolve();
    await observed.promise;
    await expect(permission).toHaveText('Automatic');
    releaseObservation.resolve();
    await expect(permission).toBeEnabled();
    await expect(permission).toHaveText('Automatic');
    expect(await finish.evaluate(read => read())).toEqual([]);
    passed = true;
  } finally {
    releaseWrite.resolve();
    releaseObservation.resolve();
    await stopDogfoodPage(page, fixture, passed);
  }
});
