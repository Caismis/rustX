import { selectedCheckpoint } from './shell-actions';
import { test, expect, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { AppServerHost } from '../../../tui/src/app-server/host';
import type { SessionModelConfig } from '../../../protocol/app-server/v50';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';
import { materializeEffective } from '../../../protocol/app-server/effective-observation';

async function chooseCurrentModel(page: Page, model: string) {
  await page.getByRole('button', { name: 'Model and profile', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Model', exact: true }).click();
  await page.getByRole('menuitem', { name: model, exact: true }).click();
}

async function openExistingSession(page: Page, id: string, width: number) {
  const expand = page.getByRole('button', { name: 'Expand Sidebar', exact: true });
  if (await expand.isVisible()) await expand.click();
  await page.locator(`button[data-session-id="${id}"]`).click();
  await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
  const collapse = page.getByRole('button', { name: 'Collapse Sidebar', exact: true });
  if (width < 600 && await collapse.isVisible()) await collapse.click();
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
}

for (const width of [1440, 390]) {
  test(`explicit current model persists inherited intent then repeats as native NoOp (${width})`, async ({ page }) => {
    const fixture = await startDogfood('tui_multi_session');
    let remote: AppServerHost | undefined;
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => {
      if (message.type() === 'error' || message.type() === 'warning') errors.push(message.text());
    });
    const wire = await wireProbe(page);
    const complete: SessionModelConfig = {
      model: 'fixture/integration-model', profile: 'high',
      requestParams: { temperature: 0.2, seed: 17 }, maxOutputTokens: 777,
      summaryModel: { mode: 'explicit', model: 'fixture/integration-model', profile: 'low',
        request_params: { temperature: 0.4, seed: 19 }, max_output_tokens: 333 },
    };
    try {
      // Creation normally persists the default. Only the stopped native fixture
      // may seed absence; its replacement owner performs all tested mutations.
      const id = await fixture.seedInheritedSession(`profile = "high"
request_params = '{"temperature":0.2,"seed":17}'
max_output_tokens = { mode = "limit", tokens = 777 }
[agent.model.summary_model]
mode = "explicit"
model = "fixture/integration-model"
profile = "low"
request_params = '{"temperature":0.4,"seed":19}'
max_output_tokens = { mode = "limit", tokens = 333 }
`);
      remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
      const before = await remote.readSettings(id);
      expect(before.settings.model).toBeUndefined();
      await page.setViewportSize({ width, height: 1000 });
      await routeWorkspaceHost(page, fixture);
      await page.goto('/');
      await expect(page).toHaveTitle(/rustX/);
      await connectRemote(page, fixture.endpoint, fixture.token);
      await openExistingSession(page, id, width);
      const attached = wire.responses.filter(response => response.method === 'session/attach').at(-1)!.result;
      expect(attached.type).toBe('attached');
      expect(attached).not.toHaveProperty('snapshot');
      expect((await wire.read('session/checkpoint', attached.target)).checkpoint.snapshot.model!.configured).toEqual(complete);
      const checkpoint = () => wire.read('session/checkpoint', attached.target);
      const configuration = async () => materializeEffective(
        (await wire.read('session/effectiveConfiguration', attached.target)).projection,
        (reference, offset) => wire.effectiveValueRead(attached.target, reference, offset), () => true,
      );
      const trigger = page.getByRole('button', { name: 'Model and profile', exact: true });
      await expect(trigger).toContainText('high');
      expect(wire.requests.filter(request => request.method === 'session/setModel')).toHaveLength(0);

      await chooseCurrentModel(page, complete.model);
      await expect.poll(() => wire.responses.filter(response => response.method === 'session/setModel').length).toBe(1);
      expect(wire.requests.filter(request => request.method === 'session/setModel').map(request => request.params.config)).toEqual([complete]);
      expect(wire.responses.filter(response => response.method === 'session/setModel')[0].result).toMatchObject({ type: 'model_set', settings_revision: (BigInt(before.revision) + 1n).toString() });
      const explicit = await remote.readSettings(id);
      expect(explicit.settings.model).toEqual(complete);
      expect(BigInt(explicit.revision)).toBe(BigInt(before.revision) + 1n);
      expect((await checkpoint()).checkpoint.snapshot.model?.configured).toEqual(complete);
      expect((await configuration()).session_model).toEqual(complete);
      await expect.poll(async () => {
        const application = (await configuration()).application;
        return Object.values(application?.units ?? {}).some(unit => unit?.status === 'preparing');
      }).toBe(false);
      // Finish the checkpoint's native read-domain repairs before measuring native NoOp.
      await checkpoint();
      const stableSnapshot = await checkpoint();
      const stableConfiguration = await configuration();
      const catalogPath = join(fixture.directory, 'runtime/sessions/catalog.json');
      const stableBytes = readFileSync(catalogPath);

      // The slash picker is a second real deliberate gesture. Its old local
      // equality guard must not suppress the request either.
      const input = page.getByRole('textbox', { name: 'Message', exact: true });
      await input.fill('/model');
      const picker = page.locator('[data-model-picker]');
      await picker.getByRole('option', { name: complete.model, exact: true }).click();
      await expect(picker).toHaveCount(0);
      await expect(input).toHaveValue('');
      await expect.poll(() => wire.responses.filter(response => response.method === 'session/setModel').length).toBe(2);
      expect(wire.requests.filter(request => request.method === 'session/setModel').map(request => request.params.config)).toEqual([complete, complete]);
      expect(wire.responses.filter(response => response.method === 'session/setModel')[1].result).toMatchObject({ type: 'model_set', settings_revision: (BigInt(before.revision) + 1n).toString() });
      expect(await remote.readSettings(id)).toEqual(explicit);
      expect(readFileSync(catalogPath)).toEqual(stableBytes);
      const repeated = await checkpoint();
      expect(repeated.checkpoint.cursor).toEqual(stableSnapshot.checkpoint.cursor);
      expect(repeated.checkpoint.snapshot.model).toEqual(stableSnapshot.checkpoint.snapshot.model);
      expect(repeated.checkpoint.snapshot.resources).toEqual(stableSnapshot.checkpoint.snapshot.resources);
      expect(await configuration()).toEqual(stableConfiguration);
      // Internal Ready identity/preparation-worker invariants remain owned by
      // model_selection_noop.rs; this boundary checks their public projection.
      await page.screenshot({ path: `/tmp/rustx-471-explicit-current-model-${width}.png` });
      await page.reload();
      await expect(page).toHaveTitle(/rustX/);
      await connectRemote(page, fixture.endpoint, fixture.token);
      await openExistingSession(page, id, width);
      await expect(trigger).toContainText('high');

      expect((await selectedCheckpoint(page, wire)).snapshot.model!.configured).toEqual(complete);
      expect(await remote.readSettings(id)).toEqual(explicit);
      expect(wire.requests.filter(request => request.method === 'session/setModel')).toHaveLength(2);
      expect((await fixture.control('requests')).requests).toHaveLength(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      expect(errors).toEqual([]);
    } finally {
      await remote?.shutdown();
      const report = await fixture.stop(false);
      expect(report.requestCount).toBe(0);
    }
  });
}

// Each acknowledgement outcome once, each at the other width: a received
// acknowledgement on desktop and a lost one, recovered through reconnect, in
// the narrow layout. Neither the width-specific placement nor the lost-ACK
// recovery depends on the other.
for (const [width, acknowledgement] of [[1440, 'received'], [390, 'lost']] as const) {
  test(`live same-model profile selection (${width}, acknowledgement ${acknowledgement})`, { tag: acknowledgement === 'lost' ? ['@smoke'] : [] }, async ({ page }) => {
    // Reuse the shared two-request provider scenario. Its first stream is held
    // at an explicit gate until the profile mutation and recovery are proven.
    const fixture = await startDogfood('tui_multi_session');
    let passed = false;
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => {
      if (message.type() === 'error' || message.type() === 'warning') errors.push(message.text());
    });
    const wire = await wireProbe(page);
    try {
      await page.setViewportSize({ width, height: 1000 });
      await routeWorkspaceHost(page, fixture);
      await page.goto('/');
      await expect(page).toHaveTitle(/rustX/);
      await connectRemote(page, fixture.endpoint, fixture.token);
      const sessionId = await openEmptySession(page, fixture, 'Workspace A');
      const collapse = page.getByRole('button', { name: 'Collapse Sidebar', exact: true });
      if (width < 600 && await collapse.isVisible()) await collapse.click();
      const input = page.getByRole('textbox', { name: 'Message', exact: true });
      await input.fill('tui multi-session: session A long task');
      await input.press('Enter');
      await fixture.gate('session-a-holding');
      const running = page.locator('[data-running-model]');
      await expect(running).toHaveText('Running: fixture/integration-model / high');
      const trigger = page.getByRole('button', { name: 'Model and profile', exact: true });
      await expect(trigger).toBeEnabled();
      const attached = wire.responses.filter(response => response.method === 'session/attach').at(-1)!.result;
      const original = (await wire.read('session/checkpoint', attached.target)).checkpoint.snapshot.model!.configured;
      await running.click();
      const admittedFrozen = JSON.parse(await page.locator('[data-running-model-details] pre').innerText());
      await running.click();
      await chooseCurrentModel(page, original.model);
      await expect.poll(() => wire.responses.filter(response => response.method === 'session/setModel').length).toBe(1);
      expect(wire.requests.filter(request => request.method === 'session/setModel').map(request => request.params.config)).toEqual([original]);
      expect(wire.responses.filter(response => response.method === 'session/setModel')[0].result).toMatchObject({ type: 'model_set' });
      await expect(running).toHaveText('Running: fixture/integration-model / high');
      if (acknowledgement === 'lost') wire.loseNext('session/setModel');
      await trigger.click();
      await page.getByRole('menuitem', { name: 'Profile', exact: true }).click();
      await page.getByRole('menuitem', { name: 'low', exact: true }).click();
      if (acknowledgement === 'lost') {
        await expect.poll(wire.lost).toBe(1);
        await connectRemote(page, fixture.endpoint, fixture.token);
      }
      await expect(trigger).toContainText('low');
      await expect(running).toHaveText('Running: fixture/integration-model / high');
      await running.click();
      const details = page.locator('[data-running-model-details]');
      await expect(details.locator('[data-session-model]')).toHaveText('Session selection: fixture/integration-model / low');
      await expect(details.getByText(/Frozen at admission/)).toBeVisible();
      const bounds = await details.boundingBox();
      expect(bounds).not.toBeNull();
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
      await expect(trigger).toBeVisible();
      const frozen = JSON.parse(await details.locator('pre').innerText());
      expect(frozen).toEqual(admittedFrozen);
      expect(frozen.primary.profile).toBe('high');
      expect(frozen.primary.reasoningEnabled).toBe(true);
      expect(frozen.primary.requestParams.reasoning_effort).toBe('high');
      expect(frozen.primary.maxOutputTokens).toBe(4096);
      expect(frozen.summary.mode).toBe('session');
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: `/tmp/rustx-471-live-model-${width}-${acknowledgement}.png` });
      await running.click();
      expect(wire.requests.filter(request => request.method === 'session/setModel').map(request => request.params.config)).toEqual([original, { ...original, profile: 'low' }]);

      // Reload while the original request remains physically active. Native
      // attach must restore both authorities, with no mutation replay.
      await page.reload();
      await connectRemote(page, fixture.endpoint, fixture.token);
      const expand = page.getByRole('button', { name: 'Expand Sidebar', exact: true });
      if (await expand.isVisible()) await expand.click();
      await page.locator(`button[data-session-id="${sessionId}"]`).click();
      // Let the native attachment publish its route before dismissing the
      // sidebar: dismissing a pending navigation deliberately cancels it.
      await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
      if (width < 600 && await collapse.isVisible()) await collapse.click();
      await expect(trigger).toContainText('low');
      await expect(running).toHaveText('Running: fixture/integration-model / high');
      expect(wire.requests.filter(request => request.method === 'session/setModel').map(request => request.params.config)).toEqual([original, { ...original, profile: 'low' }]);
      await running.click();
      expect(JSON.parse(await page.locator('[data-running-model-details] pre').innerText())).toEqual(admittedFrozen);
      await running.click();
      await fixture.release('session-a-holding');
      await expect(running).toHaveCount(0);
      try { await expect(input).toBeEnabled(); }
      catch (cause) {

        await test.info().attach('recovery-authority', { body: JSON.stringify(await selectedCheckpoint(page, wire)), contentType: 'application/json' });
        throw cause;
      }
      await input.fill('tui multi-session: session B quick task');
      await input.press('Enter');
      await expect(page.getByText('B answered while A was still working', { exact: true })).toBeVisible();
      const captured = await fixture.control('requests') as { requests: { body: Record<string, unknown> }[] };
      expect(captured.requests).toHaveLength(2);
      const [high, low] = captured.requests.map(request => request.body);
      expect(high.reasoning_effort).toBe('high');
      expect(low.reasoning_effort).toBeUndefined();
      expect(high.max_tokens ?? high.max_completion_tokens).toBe(4096);
      expect(low.max_tokens ?? low.max_completion_tokens).toBe(1024);
      expect(errors).toEqual([]);
      passed = true;
    } finally { await fixture.stop(passed); }
  });
}
