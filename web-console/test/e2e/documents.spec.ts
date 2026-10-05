import { test, expect } from '@playwright/test';
import { copyFileSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, choose } from './shell-actions';

test('real authorized advanced documents, isolated hostile HTML and original downloads', async ({ page }, info) => {
  const fixture = await startDogfood('web_advanced_documents'); let passed = false;
  const names = ['sample.pdf', 'sample.docx', 'sample.pptx', 'sample.xlsx', 'benign.html', 'hostile.html', 'malformed.pdf', 'encrypted.pdf'];
  let documentFonts: number | undefined;
  const external: string[] = [], errors: string[] = [], popups: string[] = [];
  page.on('request', request => { if (request.url().includes('rustx-preview.invalid')) external.push(request.url()); });
  page.on('pageerror', error => errors.push(error.message)); page.on('popup', popup => popups.push(popup.url()));
  await page.addInitScript(() => {
    if (window !== window.top) return;
    localStorage.setItem('document-preview-secret', 'application-private');
    const urls = new Set<string>(), create = URL.createObjectURL, revoke = URL.revokeObjectURL;
    URL.createObjectURL = blob => { const url = create(blob); urls.add(url); return url; };
    URL.revokeObjectURL = url => { urls.delete(url); revoke(url); };
    const native = Worker, live = new Set<Worker>();
    class ObservedWorker extends native {
      constructor(url: string | URL, options?: WorkerOptions) { super(url, options); live.add(this); }
      terminate() { live.delete(this); super.terminate(); }
    }
    Object.assign(window, { Worker: ObservedWorker, documentWorkers: live, documentUrls: urls });
  });
  const panel = page.getByRole('complementary', { name: 'Previews', exact: true });
  const open = async (name: string) => {
    await page.getByRole('button', { name: `Preview ${name}`, exact: true }).click(); await expect(panel).toBeVisible();
  };
  const close = async () => {
    await panel.getByRole('button', { name: /^Close preview / }).first().click();
    await expect.poll(() => page.evaluate(() => (window as any).documentWorkers.size)).toBe(0);
    await expect.poll(() => page.evaluate(() => (window as any).documentUrls.size)).toBe(0);
    if (documentFonts !== undefined) expect(await page.evaluate(() => document.fonts.size)).toBe(documentFonts);
  };
  try {
    for (const name of names) copyFileSync(new URL(`../fixtures/documents/${name}`, import.meta.url), join(fixture.workspaceA, name));
    await routeWorkspaceHost(page, fixture); await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Preview advanced files');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await fixture.gate('document-stream');
    await page.evaluate(() => document.fonts.ready); documentFonts = await page.evaluate(() => document.fonts.size);
    await open('sample.pdf'); await expect(panel.locator('.textLayer')).toContainText('page 1');
    const viewport = page.locator('.conversation-scroll');
    await viewport.hover(); await page.mouse.wheel(0, -600);
    await expect.poll(() => viewport.evaluate(el => el.scrollTop)).toBe(0);
    // The live tool activity folds at settlement; the original user message is the reading anchor.
    const readingCard = page.getByRole('article', { name: 'Your message', exact: true }).getByText('Preview advanced files', { exact: true });
    const streamingPosition = await readingCard.evaluate(el => el.getBoundingClientRect().top);
    await fixture.release('document-stream');
    await expect(page.getByText('Documents delivered. Streaming continues.', { exact: true })).toBeVisible(); await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeVisible();
    expect(await readingCard.evaluate(el => el.getBoundingClientRect().top)).toBeCloseTo(streamingPosition, 0);
    await expect(panel.locator('.textLayer')).toContainText('page 1');
    await page.screenshot({ animations: 'disabled', path: info.outputPath('pdf-light.png') });
    await info.attach('pdf-light', { path: info.outputPath('pdf-light.png'), contentType: 'image/png' }); await close();
    const modelRequests = (await fixture.control('requests')).requests.length;
    for (const name of names) {
      await open(name);
      if (name === 'sample.xlsx') {
        await expect(panel.getByRole('cell', { name: '99', exact: true })).toBeVisible();
        await expect(panel.getByRole('cell', { name: 'A1+B1', exact: true })).toBeVisible();
        await panel.getByLabel('Sheet', { exact: true }).selectOption('1'); await expect(panel.getByText('Stored text only')).toBeVisible();
      } else if (name.endsWith('.html')) {
        await expect(panel.getByTitle('Isolated HTML preview')).toHaveAttribute('sandbox', '');
        const frame = panel.frameLocator('iframe'); await expect(frame.getByRole('heading')).toBeVisible();
        if (name === 'hostile.html') {
          await frame.getByText('External', { exact: true }).click();
          await expect(frame.locator('script,form,iframe,meta[http-equiv="refresh"],[href],[src],[srcset],[action],[formaction],[style]')).toHaveCount(0);
          const isolation = await frame.locator('body').evaluate(() => {
            let parentDenied = false, storageDenied = false;
            try { void parent.document; } catch { parentDenied = true; }
            try { localStorage.getItem('document-preview-secret'); } catch { storageDenied = true; }
            return { parentDenied, storageDenied };
          });
          expect(isolation).toEqual({ parentDenied: true, storageDenied: true });
          expect(await page.evaluate(() => (window as any).PWNED)).toBeUndefined();
          expect(external).toEqual([]); expect(popups).toEqual([]);
        }
        await panel.getByRole('button', { name: 'Source', exact: true }).click();
        await expect(panel.locator('pre')).toHaveText(readFileSync(join(fixture.workspaceA, name), 'utf8'));
        await expect(panel.locator('pre *')).toHaveCount(0);
        await panel.getByRole('button', { name: 'Rendered preview', exact: true }).click();
      } else if (name === 'malformed.pdf') {
        await expect(panel.getByRole('alert')).toContainText('malformed');
      } else if (name === 'encrypted.pdf') {
        await expect(panel.getByRole('alert')).toContainText('Password-protected');
      } else {
        await expect(panel.locator('.textLayer')).toContainText('rustX');
        await expect(panel.getByRole('status')).toHaveCount(0);
        if (name === 'sample.pdf') {
          await panel.getByRole('button', { name: 'Next', exact: true }).click();
          await expect(panel.locator('.textLayer')).toContainText('page 2');
          await panel.getByLabel('Zoom', { exact: true }).selectOption('1.5');
          await expect(panel.getByRole('status')).toHaveCount(0);
          const selected = await panel.locator('.textLayer').evaluate(element => {
            const selection = window.getSelection()!, range = document.createRange(); range.selectNodeContents(element); selection.removeAllRanges(); selection.addRange(range); return selection.toString();
          });
          expect(selected).toContain('page 2');
        }
      }
      const waiting = page.waitForEvent('download'); await panel.getByRole('button', { name: 'Download artifact', exact: true }).click();
      const downloaded = await waiting; expect(downloaded.suggestedFilename()).toBe(name);
      const destination = info.outputPath(name); await downloaded.saveAs(destination);
      expect(readFileSync(destination)).toEqual(readFileSync(join(fixture.workspaceA, name)));
      await close();
    }
    for (const width of [390, 1440]) {
      await page.setViewportSize({ width, height: 1000 });
      // Establish the reading baseline after the measured responsive columns
      // finish reflowing; a transient narrow sidebar width is not that baseline.
      const sidebar = width === 390 ? 56 : 280;
      await expect(page.locator('[data-harness-frame]')).toHaveCSS('grid-template-columns', `${sidebar}px ${width - sidebar}px 0px`);
      await page.getByRole('button', { name: 'Preview sample.pdf', exact: true }).scrollIntoViewIfNeeded();
      await page.getByRole('button', { name: 'Preview sample.pdf', exact: true }).focus();
      const scroll = await page.locator('.conversation-scroll').evaluate(el => el.scrollTop);
      await page.keyboard.press('Enter');
      await expect(panel.locator('.textLayer')).toContainText('page 1');
      if (width === 390) await page.screenshot({ animations: 'disabled', path: info.outputPath('pdf-narrow.png') });
      await close();
      expect(await page.locator('.conversation-scroll').evaluate(el => el.scrollTop)).toBeCloseTo(scroll, 0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
    await page.route('**/pdf.worker*.mjs', route => route.abort());
    await open('sample.pdf'); await expect(panel.getByRole('alert')).toContainText('PDF worker failed');
    await page.unroute('**/pdf.worker*.mjs');
    await panel.getByRole('button', { name: 'Retry preview', exact: true }).click();
    await expect(panel.locator('.textLayer')).toContainText('page 1'); await close();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Theme', 'Dark');
    await page.getByRole('button', { name: 'Close Settings', exact: true }).click();
    await open('sample.pdf'); await expect(panel.locator('.textLayer')).toContainText('page 1');
    await page.screenshot({ animations: 'disabled', path: info.outputPath('pdf-dark.png') });
    await info.attach('pdf-dark', { path: info.outputPath('pdf-dark.png'), contentType: 'image/png' }); await close();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
    await page.getByRole('button', { name: '关闭设置', exact: true }).click();
    await page.getByRole('button', { name: '预览 sample.xlsx', exact: true }).click();
    await expect(page.getByRole('columnheader', { name: '公式（未计算）' })).toBeVisible();
    await page.screenshot({ animations: 'disabled', path: info.outputPath('workbook-chinese.png') });
    await info.attach('workbook-chinese', { path: info.outputPath('workbook-chinese.png'), contentType: 'image/png' });
    expect((await fixture.control('requests')).requests.length).toBe(modelRequests);
    expect(external).toEqual([]); expect(errors).toEqual([]); passed = true;
  } finally { await fixture.stop(passed); }
});
