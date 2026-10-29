import { test, expect, type Page } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parse } from 'yaml';
import { startDogfood } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { captureEvidence } from './screenshot';
import { emptyBlock } from '../../src/app/workflows/document/commands';
import { setNodePresentation } from '../../src/app/workflows/document/geometry';
import type { WorkflowDocument } from '../../src/app/workflows/document/model';

const inspector = (page: Page) => page.getByRole('complementary', { name: 'Inspector' });
const canvas = (page: Page) => page.getByRole('region', { name: 'Workflow canvas' });
async function activate(page: Page) {
  const node = canvas(page).getByLabel('Branch node choose', { exact: true }); await node.focus(); await node.press('Enter');
}

test('authors ordered IF ELIF ELSE conditions and stable exits with the Dify-style card and panel', async ({ page }, info) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  try {
    let document: WorkflowDocument = { description: 'Ordered cases', block: emptyBlock() };
    document.block.input = { type: 'object', properties: { status: { type: 'string' }, flag: { type: 'boolean' } }, required: ['status', 'flag'], additionalProperties: false };
    document.block.nodes.choose = { type: 'branch', cases: [{ id: 'true', condition: { type: 'and', predicates: [] } }] };
    document.block.edges = [{ from: 'start', to: 'choose' }, { from: 'choose', to: 'done', port: 'true' }, { from: 'choose', to: 'done', port: 'false' }];
    document = setNodePresentation(document, [], 'choose', { title: 'Conditions', description: 'Select the first matching path.', position: { x: 380, y: 100 } });
    document = setNodePresentation(document, [], 'done', { position: { x: 740, y: 100 } });
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'branch_cases.yaml'); writeFileSync(file, JSON.stringify(document));
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await page.getByRole('button', { name: 'branch_cases', exact: true }).click();
    await expect(page).toHaveTitle(/rustX$/); await activate(page);
    const card = canvas(page).getByLabel('Branch node choose', { exact: true });
    await expect(card).toContainText('IF'); await expect(card).toContainText('ELSE');
    await expect(card).not.toContainText('Select the first matching path.');
    expect((await card.boundingBox())!.height).toBeLessThan(130);
    await captureEvidence(page, 'branch-empty', info);
    const settings = inspector(page).getByRole('region', { name: 'Settings', exact: true });
    const first = settings.getByRole('region', { name: 'IF', exact: true });
    await first.getByRole('button', { name: 'Add condition', exact: true }).click();
    const picker = page.getByRole('dialog', { name: 'Choose a variable', exact: true });
    await expect(picker.getByRole('textbox', { name: 'Search reference paths…' })).toBeFocused();
    const popupBounds = (await picker.boundingBox())!;
    expect(popupBounds.x).toBeGreaterThanOrEqual(0); expect(popupBounds.x + popupBounds.width).toBeLessThanOrEqual(1440);
    await captureEvidence(page, 'branch-variable-picker', info);
    await picker.getByRole('button', { name: /^args \/ status ·/ }).click();
    await first.getByRole('textbox', { name: 'Value', exact: true }).fill('approved');
    await first.getByRole('textbox', { name: 'Value', exact: true }).blur();
    await first.getByRole('button', { name: 'Add condition', exact: true }).click();
    await picker.getByRole('button', { name: /^args \/ flag ·/ }).click();
    await first.getByLabel('Match', { exact: true }).selectOption('or');
    await settings.getByRole('button', { name: 'ELIF', exact: true }).click();
    const second = settings.getByRole('region', { name: 'ELIF CASE 2', exact: true });
    await second.getByRole('button', { name: 'Add condition', exact: true }).click();
    await picker.getByRole('button', { name: /^args \/ flag ·/ }).click();
    await second.getByLabel('Value', { exact: true }).selectOption('false');
    await expect(card.getByText('or', { exact: true })).toBeVisible();
    await expect(card).toContainText('CASE 1'); await expect(card).toContainText('CASE 2'); await expect(card).toContainText('ELIF');
    await expect(card.locator('[data-handleid="case_2"]')).toHaveCount(1);
    const next = inspector(page).getByRole('region', { name: 'Next step', exact: true });
    await next.getByRole('button', { name: 'Choose next node for CASE 2', exact: true }).click();
    const nodes = page.getByRole('dialog', { name: 'Choose a workflow node' });
    await nodes.getByRole('tab', { name: 'Existing' }).click(); await nodes.locator('[data-node-id="done"]').click();
    for (const [port, label] of [['true', 'IF'], ['case_2', 'ELIF'], ['false', 'ELSE']]) {
      const handle = (await card.locator(`[data-handleid="${port}"]`).boundingBox())!, text = (await card.getByText(label, { exact: true }).boundingBox())!;
      expect(Math.abs(handle.y + handle.height / 2 - text.y - text.height / 2)).toBeLessThan(2);
    }
    await first.getByRole('button', { name: 'Add condition', exact: true }).scrollIntoViewIfNeeded();
    await captureEvidence(page, 'branch-configured', info);
    await settings.getByRole('button', { name: 'Remove CASE 1', exact: true }).click();
    await expect(card.locator('[data-handleid="case_2"]')).toHaveCount(1);
    await expect(card.locator('[data-handleid="true"]')).toHaveCount(0);
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(card.locator('[data-handleid="true"]')).toHaveCount(1);
    await page.getByRole('button', { name: 'Check', exact: true }).click(); await expect(inspector(page)).toContainText('Structure check passed');
    await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    const saved = parse(readFileSync(file, 'utf8'));
    expect(saved.block.nodes.choose).not.toHaveProperty('condition');
    expect(saved.block.nodes.choose.cases.map((item: any) => item.id)).toEqual(['true', 'case_2']);
    expect(saved.block.nodes.choose.cases[0].condition.type).toBe('or');
    expect(saved.block.edges.filter((edge: any) => edge.from === 'choose').map((edge: any) => String(edge.port)).sort()).toEqual(['case_2', 'false', 'true']);
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click(); await page.getByRole('button', { name: 'branch_cases', exact: true }).click(); await activate(page);
    await expect(inspector(page).getByRole('textbox', { name: 'Value', exact: true })).toHaveValue('approved');
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(inspector(page)).toHaveCSS('position', 'absolute'); await captureEvidence(page, 'branch-mobile', info);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});
