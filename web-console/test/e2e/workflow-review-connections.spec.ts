import { test, expect } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parse } from 'yaml';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { setNodePresentation } from '../../src/app/workflows/document/geometry';
import type { WorkflowDocument } from '../../src/app/workflows/document/model';

for (const renamed of [false, true]) test(`Human Input middle exit connects and persists (renamed: ${renamed})`, async ({ page }) => {
  page.setDefaultTimeout(10_000);
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  try {
    const middle = renamed ? 'action_2' : 'NO_BARK';
    let document: WorkflowDocument = { description: 'Human Input middle exit', block: {
      input: { type: 'object', properties: {}, additionalProperties: false },
      nodes: {
        start: { type: 'start' },
        human: { type: 'review', content: [{ type: 'literal', value: 'Choose an action.' }], actions: [{ id: 'BARK', title: 'Bark' }, { id: middle, title: 'No bark' }], timeout_ms: '60000' },
        done: { type: 'return', outputs: [] },
      },
      edges: [{ from: 'start', to: 'human' }, ...['BARK', middle, 'timeout'].map(port => ({ from: 'human', to: 'done', port }))],
    } };
    document = setNodePresentation(document, [], 'human', { position: { x: 260, y: 120 } });
    document = setNodePresentation(document, [], 'done', { position: { x: 620, y: 120 } });
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'review_connections.yaml');
    // The wire carries u64 as a string; native files hold the integer.
    writeFileSync(file, JSON.stringify(document).replace('"timeout_ms":"60000"', '"timeout_ms":60000'));
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await page.getByRole('button', { name: 'review_connections', exact: true }).click();
    const canvas = page.getByRole('region', { name: 'Workflow canvas' });
    const human = canvas.getByLabel('Human Input node human', { exact: true });
    await human.focus(); await human.press('Enter');
    const panel = page.getByRole('complementary', { name: 'Inspector' });
    // Leave only the middle action unconnected, as in the reported graph.
    await panel.getByRole('button', { name: 'Disconnect done', exact: true }).nth(1).click();
    const height = (await human.boundingBox())!.height;
    if (renamed) {
      const id = panel.getByRole('textbox', { name: 'Action ID', exact: true }).nth(1);
      await id.fill('NO_BARK'); await id.press('Enter');
      await expect(human.locator('[data-handleid="NO_BARK"]')).toHaveCount(1);
      // Renaming changes the handle identity without triggering a size observer.
      expect((await human.boundingBox())!.height).toBe(height);
    }
    const source = human.locator('[data-handleid="NO_BARK"]');
    const target = canvas.getByLabel('Output node done', { exact: true }).locator('[data-handleid="in"]');
    const dragMiddle = async () => {
      const from = (await source.boundingBox())!, to = (await target.boundingBox())!;
      await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
      await page.mouse.down();
      await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 16 });
      await expect(target).toHaveClass(/connectingto/);
      await expect(target).toHaveClass(/\bvalid\b/);
      await page.mouse.up();
      await expect(canvas.locator('.react-flow__edge')).toHaveCount(4);
    };
    await dragMiddle();
    if (renamed) {
      // Undo the edge and rename, then restore the ID and drag from it again.
      await page.getByRole('button', { name: 'Undo', exact: true }).click();
      await page.getByRole('button', { name: 'Undo', exact: true }).click();
      await expect(human.locator('[data-handleid="action_2"]')).toHaveCount(1);
      await page.getByRole('button', { name: 'Redo', exact: true }).click();
      await dragMiddle();
    }
    await page.getByRole('button', { name: 'Save File', exact: true }).click();
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    const saved = parse(readFileSync(file, 'utf8'));
    expect(saved.block.edges).toEqual(expect.arrayContaining(['BARK', 'NO_BARK', 'timeout'].map(port => ({ from: 'human', to: 'done', port }))));
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).first().click();
    await page.getByRole('button', { name: 'review_connections', exact: true }).click();
    await expect(canvas.locator('.react-flow__edge')).toHaveCount(4);
    await page.screenshot({ animations: 'disabled', path: `/tmp/rustx-review-connections-${renamed}.png` });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally { await stopDogfoodPage(page, fixture, false); }
});
