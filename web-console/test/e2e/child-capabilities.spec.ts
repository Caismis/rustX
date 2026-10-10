import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, chooseWorkspace } from './shell-actions';
import { wireProbe } from './wire-probe';

test('native child live cuts, independent title, active and file-only resume uploads, file and image preview', async ({ page }, info) => {
  page.setDefaultTimeout(15_000);
  const fixture = await startDogfood('web_child_capabilities');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  const wire = await wireProbe(page);
  let passed = false;
  try {
    await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token); await chooseWorkspace(page, 'Workspace A');
    await page.getByLabel('Message', { exact: true }).fill('CHILD_CAPABILITIES_PARENT');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await Promise.all([fixture.gate('child-first-0'), fixture.gate('child-first-1')]);
    const recorded = (await fixture.control('requests')).requests;
    const parentIndex = [1, 2].find(index => JSON.stringify(recorded[index]).includes('CHILD_CAPABILITIES_PARENT'))!;
    const childIndex = 3 - parentIndex;
    expect(childIndex).toBeDefined();
    const childGate = childIndex - 1, parentGate = 1 - childGate;
    const parentTitle = await page.locator('#session-title').innerText();
    const open = async () => { await page.getByRole('button', { name: 'Subagents', exact: true }).click(); await page.getByRole('treeitem', { name: /Research UI capabilities/ }).click(); };
    await open();
    const child = page.locator('section[data-agent-id]');
    const id = await child.getAttribute('data-agent-id');
    const activation = await child.getAttribute('data-activation-id');
    await expect(child.getByText('Live child first chunk.', { exact: true })).toBeVisible();
    expect(wire.requests.some(request => request.method === 'agent/conversation')).toBe(true);
    // Leave a live read, reopen from an authoritative cut, then advance one delta.
    await page.getByRole('button', { name: parentTitle, exact: true }).click();
    await expect.poll(() => wire.requests.filter(request => request.method === 'agent/conversationCancel').length).toBeGreaterThan(0);
    await open();
    await expect(child.getByText('Live child first chunk.', { exact: true })).toBeVisible();
    await fixture.release(`child-first-${childGate}`); await fixture.gate(`child-second-${childGate}`);
    await expect(child.getByText('Live child first chunk. Second live chunk.', { exact: true })).toBeVisible();
    await page.screenshot({ path: '/tmp/rustx-native-child-streaming.png' });
    const input = child.getByRole('textbox', { name: 'Message Agent Research UI capabilities', exact: true });
    await child.getByLabel('Attach files', { exact: true }).setInputFiles({ name: 'active.txt', mimeType: 'text/plain', buffer: Buffer.from('Active child attachment') });
    await input.fill('ACTIVE_CHILD_UPLOAD');
    await child.getByRole('button', { name: 'Steer', exact: true }).click();
    await expect(input).toHaveValue('');
    await fixture.release(`child-second-${childGate}`);
    await expect(child).toHaveAttribute('data-agent-state', 'inactive');
    await expect(child.getByText('Canonical child final report.', { exact: true })).toBeVisible();
    // The in-flight message becomes its single canonical message at settlement.
    await expect(child.getByText('Live child first chunk. Second live chunk.', { exact: true })).toHaveCount(1);
    const card = child.locator('[data-delivery-card]').filter({ has: page.locator('[data-presented-name]', { hasText: 'child-report.md' }) }).first();
    await card.getByRole('button', { name: 'Preview child-report.md in sidebar', exact: true }).click();
    const preview = page.getByRole('complementary', { name: 'Previews', exact: true });
    await expect(preview.getByRole('heading', { name: 'Child deliverable' })).toBeVisible();
    const download = page.waitForEvent('download');
    await preview.getByRole('button', { name: 'Download artifact', exact: true }).click();
    const delivered = await download, output = info.outputPath('child-report.md'); await delivered.saveAs(output);
    expect(readFileSync(output)).toEqual(readFileSync(join(fixture.workspaceA, 'child-report.md')));
    // Harness sidebar chrome is two compact rows, not a separate panel heading
    // and text toolbar. Markdown must not inherit plain-text whitespace.
    const strip = preview.locator('[data-preview-strip]');
    const header = preview.locator('[data-preview-header]');
    expect((await strip.boundingBox())!.height).toBe(38);
    expect((await header.boundingBox())!.height).toBe(38);
    expect((await header.boundingBox())!.y).toBe((await strip.boundingBox())!.y + 38);
    await expect(preview.getByRole('button', { name: 'Wrap lines', exact: true })).toHaveCount(0);
    await expect(preview.locator('[data-preview-scroll="body"]')).toHaveCSS('white-space', 'normal');
    await expect(preview.locator('footer')).toHaveCount(0);
    await expect(strip.getByRole('tab').locator('svg')).toHaveCount(1);
    await preview.getByRole('tab').first().focus();
    await page.mouse.move(600, 400);
    await expect(page.getByRole('tooltip')).toHaveCount(0);
    await page.screenshot({ path: '/tmp/rustx-native-child-capabilities.png' });
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(preview.getByRole('heading', { name: 'Child deliverable' })).toBeVisible();
    await expect(preview.getByRole('button', { name: 'Collapse preview workspace', exact: true })).toBeInViewport();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(390);
    await page.screenshot({ path: '/tmp/rustx-native-child-sidebar-mobile.png' });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await preview.getByRole('button', { name: /^Close preview / }).first().click();
    const process = child.locator('[data-turn-process][aria-expanded]').last();
    if (await process.getAttribute('aria-expanded') === 'false') await process.click();
    const tool = child.locator('[data-tool-call-id="child-image"]').last();
    const step = child.locator('[data-step-process]').filter({ has: page.locator('[data-tool-call-id="child-image"]') }).last().locator(':scope > button');
    if (await step.getAttribute('aria-expanded') === 'false') await step.click();
    const disclosure = tool.getByRole('button', { expanded: false });
    if (await disclosure.count()) await disclosure.click();
    await child.getByRole('button', { name: 'Load attachment', exact: true }).click();
    await expect(child.locator('img')).toBeVisible();
    await expect.poll(() => child.locator('img').evaluate((node: HTMLImageElement) => node.complete && node.naturalWidth > 0)).toBe(true);
    expect(wire.requests.some(request => request.method === 'agent/artifactRead' && request.params.agent_id === id)).toBe(true);
    // The native locator, not the currently loaded Trace page, owns Inspect.
    await tool.getByRole('button', { name: 'Inspect', exact: true }).click();
    await expect(child).toHaveAttribute('data-view', 'trajectory');
    await expect.poll(() => wire.responses.filter(row => row.method === 'agent/traceLocateTool').length).toBe(1);
    const childLocation = wire.responses.find(row => row.method === 'agent/traceLocateTool')!.result.location;
    expect(childLocation.record_id).toBeTruthy();
    expect(wire.requests.find(row => row.method === 'agent/traceLocateTool')!.params).toMatchObject({ agent_id: id, locator: { call_id: 'child-image' } });
    await expect(child.locator(`[data-trace-id="${childLocation.record_id}"][data-selected]`)).toBeVisible();
    await expect.poll(() => wire.requests.filter(row => row.method === 'agent/traceDetail' && row.params.record_id === childLocation.record_id).length).toBeGreaterThan(0);
    await expect(child.getByRole('complementary', { name: 'Event details' })).toBeVisible();
    await page.screenshot({ path: '/tmp/rustx-native-tool-inspect-child.png' });
    await page.getByRole('tab', { name: 'Chat', exact: true }).click();
    // An empty text field with an attachment resumes the same native Agent.
    await child.getByLabel('Attach files', { exact: true }).setInputFiles({ name: 'resume.txt', mimeType: 'text/plain', buffer: Buffer.from('Resume child attachment') });
    await child.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(child.getByText('File-only child continuation completed.', { exact: true })).toBeVisible();
    await expect(child).toHaveAttribute('data-agent-id', id!); await expect(child).toHaveAttribute('data-agent-state', 'inactive');
    // The child rail spans its original and resumed native Attempts.
    const rail = page.getByRole('navigation', { name: 'Turn navigation', exact: true });
    await expect(rail).toBeVisible();
    const firstTurn = rail.getByRole('button', { name: 'Jump to turn 1', exact: true });
    await firstTurn.hover();
    await expect(page.getByRole('tooltip')).not.toBeEmpty();
    await firstTurn.click(); await expect(firstTurn).toHaveAttribute('aria-current', 'true');
    const lastTurn = rail.getByRole('button').last();
    await lastTurn.click(); await expect(lastTurn).toHaveAttribute('aria-current', 'true');
    expect(wire.requests.some(request => request.method === 'agent/turns')).toBe(true);
    await page.screenshot({ path: '/tmp/rustx-native-child-turn-navigation.png' });
    expect(wire.responses.filter(row => row.method === 'agent/sendMessage').map(row => row.result)).toMatchObject([
      { type: 'agent_message', activation_id: activation, resumed: false }, { type: 'agent_message', resumed: true },
    ]);
    expect(wire.requests.filter(request => request.method === 'agent/sendMessage').every(request => request.params.attachments.length === 1)).toBe(true);
    await page.getByRole('button', { name: parentTitle, exact: true }).click();
    await fixture.release(`child-first-${parentGate}`); await fixture.release(`child-second-${parentGate}`);
    await expect(page.getByText('Parent and child remain independent.', { exact: true })).toBeVisible();
    const parentProcess = page.locator('[data-turn-process][aria-expanded]:visible').last();
    if (await parentProcess.getAttribute('aria-expanded') === 'false') await parentProcess.click();
    const parentTool = page.locator('[data-tool-call-id="child-capabilities-create"]');
    const parentStep = page.locator('[data-step-process]:visible').filter({ has: parentTool }).last().locator(':scope > button');
    if (await parentStep.isVisible() && await parentStep.getAttribute('aria-expanded') === 'false') await parentStep.click();
    await parentTool.getByRole('button', { expanded: false }).first().click();
    await parentTool.getByRole('button', { name: 'Inspect', exact: true }).click();
    await expect(page.getByRole('tab', { name: 'Trajectory', exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect.poll(() => wire.responses.filter(row => row.method === 'session/traceLocateTool').length).toBe(1);
    const rootLocation = wire.responses.find(row => row.method === 'session/traceLocateTool')!.result.location;
    expect(wire.requests.find(row => row.method === 'session/traceLocateTool')!.params.locator.call_id).toBe('child-capabilities-create');
    await expect(page.locator(`[data-trace-id="${rootLocation.record_id}"][data-selected]:visible`)).toBeVisible();
    await expect.poll(() => wire.requests.filter(row => row.method === 'session/traceDetail' && row.params.record_id === rootLocation.record_id).length).toBeGreaterThan(0);
    await page.screenshot({ path: '/tmp/rustx-native-tool-inspect-root.png' });
    await page.getByRole('button', { name: 'Return to latest', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Return to latest', exact: true })).toHaveCount(0);
    expect(errors).toEqual([]); passed = true;
  } catch (error) { await page.screenshot({ path: '/tmp/rustx-native-child-failure.png' }).catch(() => {}); console.error(await page.locator('body').innerText().catch(() => 'Page closed')); console.error(fixture.diagnostics()); throw error; }
  finally { await page.close(); await fixture.stop(passed); }
});
