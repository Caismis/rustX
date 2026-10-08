import { test, expect, type Locator } from '@playwright/test';
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, choose } from './shell-actions';
import { wireProbe } from './wire-probe';

test('bounded preview workspace keeps exact occurrences, two live PDFs, keyboard geometry and detached reading', async ({ page }, info) => {
  const fixture = await startDogfood('web_preview_workspace');
  let passed = false;
  const errors: string[] = [];
  const reads: string[] = [], derivations: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => { if (request.url().endsWith('/product-host/file-read')) reads.push(request.postData() ?? ''); });
  page.on('request', request => { if (request.url().endsWith('/product-host/document-preview')) derivations.push(request.postData() ?? ''); });
  const wire = await wireProbe(page);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.addInitScript(() => {
    if (window !== window.top) return;
    const urls = new Set<string>(), workers = new Set<Worker>();
    const created = URL.createObjectURL, revoked = URL.revokeObjectURL, NativeWorker = Worker;
    const observation = { urls, workers, maximumUrls: 0, maximumWorkers: 0, revealMaximumWorkers: 0, duplicateRevocations: 0 };
    URL.createObjectURL = blob => { const url = created(blob); urls.add(url); observation.maximumUrls = Math.max(observation.maximumUrls, urls.size); return url; };
    URL.revokeObjectURL = url => { if (!urls.delete(url)) observation.duplicateRevocations++; revoked(url); };
    class ObservedWorker extends NativeWorker {
      constructor(url: string | URL, options?: WorkerOptions) { super(url, options); workers.add(this); observation.maximumWorkers = Math.max(observation.maximumWorkers, workers.size); observation.revealMaximumWorkers = Math.max(observation.revealMaximumWorkers, workers.size); }
      terminate() { workers.delete(this); super.terminate(); }
    }
    Object.assign(window, { Worker: ObservedWorker, previewObservation: observation });
  });
  const panel = page.getByRole('complementary', { name: 'Previews', exact: true });
  const workspace = page.locator('[data-preview-workspace]');
  const panes = workspace.locator('[data-preview-pane]');
  const tabs = workspace.getByRole('tab');
  const liveWorkers = () => page.evaluate(() => (window as any).previewObservation.workers.size);
  const liveUrls = () => page.evaluate(() => (window as any).previewObservation.urls.size);
  const opener = (name: string, index = 0) => page.getByRole('button', { name: `Preview ${name}`, exact: true }).nth(index);
  const open = async (name: string, index = 0) => { await opener(name, index).click(); await expect(panel).toBeVisible(); };
  const download = async (control: Locator, name: string, bytes: Buffer) => {
    const waiting = page.waitForEvent('download'); await control.click(); const result = await waiting;
    expect(result.suggestedFilename()).toBe(name);
    const path = info.outputPath(`original-${name}`); await result.saveAs(path); expect(readFileSync(path)).toEqual(bytes);
  };
  try {
    for (const directory of ['a', 'b']) {
      mkdirSync(join(fixture.workspaceA, directory));
      copyFileSync(new URL('../fixtures/documents/sample.pdf', import.meta.url), join(fixture.workspaceA, directory, 'sample.pdf'));
    }
    for (const name of ['sample.docx', 'sample.pptx', 'sample.xlsx', 'benign.html']) copyFileSync(new URL(`../fixtures/documents/${name}`, import.meta.url), join(fixture.workspaceA, name));
    writeFileSync(join(fixture.workspaceA, 'notes.txt'), Array.from({ length: 180 }, (_, n) => `Saved view position ${n}`).join('\n'));
    writeFileSync(join(fixture.workspaceA, 'second.txt'), 'Second document');
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    const sessionA = await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Preview workspace files');
    await page.getByRole('button', { name: 'Send', exact: true }).click(); await fixture.gate('workspace-stream');
    const modelRequests = (await fixture.control('requests')).requests.length;
    await page.setViewportSize({ width: 1920, height: 1000 });
    await open('sample.pdf'); await expect(panel.locator('.textLayer')).toContainText('page 1');
    await open('sample.pdf', 1); await expect(tabs).toHaveCount(2);
    await expect.poll(liveWorkers).toBe(1);
    const occurrences = await tabs.evaluateAll(elements => elements.map(element => element.getAttribute('data-preview-tab')));
    expect(new Set(occurrences).size).toBe(2);
    await open('sample.pdf'); await expect(tabs).toHaveCount(2);
    expect(await tabs.evaluateAll(elements => elements.map(element => element.getAttribute('data-preview-tab')))).toEqual(occurrences);

    await panel.getByRole('button', { name: 'Split preview', exact: true }).focus(); await page.keyboard.press('Enter');
    await expect(panes).toHaveCount(2); await expect(panel.locator('.textLayer')).toHaveCount(2); await expect.poll(liveWorkers).toBe(2);
    await expect(panel.getByRole('button', { name: 'Split preview', exact: true }).first()).toHaveAttribute('aria-disabled', 'true');
    const beforeDownload = await tabs.evaluateAll(elements => elements.map(element => ({ id: element.getAttribute('data-preview-tab'), selected: element.getAttribute('aria-selected') })));
    const activePaneBeforeDownload = await workspace.locator('[data-preview-active]').getAttribute('data-preview-pane');
    await download(workspace.locator('[data-preview-pane]:not([data-preview-active])').getByRole('button', { name: 'Download artifact', exact: true }), 'sample.pdf', readFileSync(join(fixture.workspaceA, 'b', 'sample.pdf')));
    expect(await workspace.locator('[data-preview-active]').getAttribute('data-preview-pane')).toBe(activePaneBeforeDownload);
    expect(await tabs.evaluateAll(elements => elements.map(element => ({ id: element.getAttribute('data-preview-tab'), selected: element.getAttribute('aria-selected') })))).toEqual(beforeDownload);
    await expect.poll(liveWorkers).toBe(2); await expect.poll(liveUrls).toBe(2);
    await download(page.getByRole('button', { name: 'Download notes.txt', exact: true }), 'notes.txt', readFileSync(join(fixture.workspaceA, 'notes.txt')));
    expect(await tabs.evaluateAll(elements => elements.map(element => ({ id: element.getAttribute('data-preview-tab'), selected: element.getAttribute('aria-selected') })))).toEqual(beforeDownload);
    await expect.poll(liveUrls).toBe(2);

    const separator = workspace.getByRole('separator', { name: 'Resize preview panes', exact: true });
    await separator.focus(); const ratio = Number(await separator.getAttribute('aria-valuenow'));
    await page.keyboard.press('ArrowRight'); expect(Number(await separator.getAttribute('aria-valuenow'))).toBeGreaterThan(ratio);
    const handle = (await separator.boundingBox())!;
    await separator.evaluate(element => element.addEventListener('gotpointercapture', () => element.setAttribute('data-test-pointer-captured', 'true'), { once: true }));
    await page.mouse.move(handle.x + handle.width / 2, handle.y + 100); await page.mouse.down();
    await page.mouse.move(handle.x + handle.width / 2 - 35, handle.y + 100);
    await expect(separator).toHaveAttribute('data-test-pointer-captured', 'true'); await page.mouse.up();
    const dragged = Number(await separator.getAttribute('aria-valuenow'));
    expect(dragged).toBeGreaterThanOrEqual(Number(await separator.getAttribute('aria-valuemin')));
    expect(dragged).toBeLessThanOrEqual(Number(await separator.getAttribute('aria-valuemax')));
    const readsBeforePresentation = reads.length;
    await panel.getByRole('button', { name: 'Fullscreen preview', exact: true }).focus(); await page.keyboard.press('Enter');
    await expect(panel).toHaveAttribute('data-sidebar-right-panel', 'fullscreen');
    const activeTab = workspace.locator('[data-preview-active] [role="tab"][aria-selected="true"]');
    await activeTab.focus();
    await activeTab.dispatchEvent('keydown', { key: 'Escape', repeat: true });
    await activeTab.dispatchEvent('keydown', { key: 'Escape', isComposing: true });
    await expect(panel).toHaveAttribute('data-sidebar-right-panel', 'fullscreen');
    await page.keyboard.press('Escape'); await expect(panel).toHaveAttribute('data-sidebar-right-panel', 'normal');
    await expect.poll(liveWorkers).toBe(2); expect(reads.length).toBe(readsBeforePresentation);

    // Restore can retire a focused divider when the normal right column cannot
    // fit both logical panes. Focus must land on the remaining active tab.
    const splitOccurrences = await tabs.evaluateAll(elements => elements.map(element => element.getAttribute('data-preview-tab')));
    await page.setViewportSize({ width: 1200, height: 1000 });
    await expect(workspace.locator('[data-preview-pane]:visible')).toHaveCount(1);
    await panel.getByRole('button', { name: 'Fullscreen preview', exact: true }).focus(); await page.keyboard.press('Enter');
    await expect(separator).toBeVisible(); await expect.poll(liveWorkers).toBe(2);
    await separator.focus(); await page.keyboard.press('Escape');
    await expect(workspace.locator('[data-preview-pane]:visible')).toHaveCount(1);
    await expect(activeTab).toBeFocused(); await expect.poll(liveWorkers).toBe(1);
    expect(await workspace.locator('[data-preview-tab]').evaluateAll(elements => elements.map(element => element.getAttribute('data-preview-tab')))).toEqual(splitOccurrences);
    await page.setViewportSize({ width: 1920, height: 1000 });
    await expect(workspace.locator('[data-preview-pane]:visible')).toHaveCount(2); await expect.poll(liveWorkers).toBe(2);

    // Wide -> hidden narrow -> reveal is the stale-admission direction.
    for (const hiddenMode of ['collapse', 'Inspector']) {
      if (hiddenMode === 'collapse') await panel.getByRole('button', { name: 'Collapse preview workspace', exact: true }).click();
      else await page.getByRole('button', { name: 'Toggle Inspector', exact: true }).click();
      await expect.poll(liveWorkers).toBe(0);
      await page.setViewportSize({ width: hiddenMode === 'Inspector' ? 1200 : 390, height: 844 });
      const readsBeforeReveal = reads.length;
      await page.evaluate(() => { (window as any).previewObservation.revealMaximumWorkers = 0; });
      if (hiddenMode === 'collapse') await page.getByRole('button', { name: 'Reopen previews', exact: true }).click();
      else await page.getByRole('button', { name: 'Toggle Inspector', exact: true }).click();
      await expect(workspace.locator('[data-preview-pane]:visible')).toHaveCount(1);
      await expect(panel.locator('.textLayer')).toHaveCount(1); await expect.poll(liveWorkers).toBe(1);
      expect(reads.length - readsBeforeReveal).toBe(1);
      expect(await page.evaluate(() => (window as any).previewObservation.revealMaximumWorkers)).toBe(1);
      await page.setViewportSize({ width: 1920, height: 1000 });
      await expect(panel.locator('.textLayer')).toHaveCount(2); await expect.poll(liveWorkers).toBe(2);
    }

    await open('notes.txt'); await expect(tabs).toHaveCount(3); await expect.poll(liveWorkers).toBe(1);
    const textPaneId = await panes.filter({ has: page.locator('pre', { hasText: 'Saved view position 0' }) }).getAttribute('data-preview-pane');
    const textPane = workspace.locator(`[data-preview-pane="${textPaneId}"]`);
    const textScroll = textPane.locator('[data-preview-scroll="body"]');
    await textScroll.evaluate(element => { element.scrollTop = 420; element.dispatchEvent(new Event('scroll')); });
    await expect(textScroll).toHaveJSProperty('scrollTop', 420);
    await textPane.getByRole('button', { name: 'Wrap lines', exact: true }).click();
    await textPane.getByRole('tab', { name: 'sample.pdf', exact: true }).click(); await expect.poll(liveWorkers).toBe(2);
    await textPane.getByRole('tab', { name: 'notes.txt', exact: true }).click();
    await expect(textPane.getByRole('button', { name: 'Wrap lines', exact: true })).toHaveAttribute('aria-pressed', 'false');
    await expect.poll(() => textScroll.evaluate(element => element.scrollTop)).toBe(420);
    await textPane.getByRole('button', { name: 'Move tab to other pane', exact: true }).focus(); await page.keyboard.press('Enter');
    await expect(panes).toHaveCount(2); await expect(tabs).toHaveCount(3);

    await open('sample.docx'); await expect(panel.getByText('Derived preview', { exact: false })).toBeVisible();
    await expect(panel.locator('.textLayer')).toHaveCount(2); await expect.poll(liveWorkers).toBe(2);
    const officePane = panes.filter({ has: page.getByRole('tab', { name: 'sample.docx', exact: true }) });
    const pdfPane = panes.filter({ hasNot: page.getByRole('tab', { name: 'sample.docx', exact: true }) });
    await pdfPane.getByRole('tab', { name: 'sample.pdf', exact: true }).click();
    await open('sample.pptx'); await expect(panel.locator('.textLayer')).toHaveCount(2); await expect.poll(liveWorkers).toBe(2);
    await officePane.getByRole('tab', { name: 'sample.docx', exact: true }).focus();
    const selectedBeforeKeys = await officePane.locator('[role="tab"][aria-selected="true"]').getAttribute('data-preview-tab');
    await page.keyboard.press('Home'); await page.keyboard.press('End');
    expect(await officePane.locator('[role="tab"][aria-selected="true"]').getAttribute('data-preview-tab')).toBe(selectedBeforeKeys);
    expect(await officePane.getByRole('tab').last().evaluate(element => element === document.activeElement)).toBe(true);
    await page.keyboard.press('Enter');
    await page.screenshot({ animations: 'disabled', path: info.outputPath('workspace-two-heavy-light.png') });
    await info.attach('workspace-two-heavy-light', { path: info.outputPath('workspace-two-heavy-light.png'), contentType: 'image/png' });

    for (const name of ['sample.xlsx', 'benign.html', 'second.txt']) await open(name);
    await expect(tabs).toHaveCount(8);
    await officePane.getByRole('tab', { name: 'sample.docx', exact: true }).click();
    await expect(panel.locator('.textLayer')).toHaveCount(2); await expect.poll(liveWorkers).toBe(2);

    await page.getByRole('button', { name: 'Toggle Inspector', exact: true }).click();
    await expect(page.getByRole('complementary', { name: 'Developer inspector', exact: true })).toBeVisible(); await expect.poll(liveWorkers).toBe(0); await expect.poll(liveUrls).toBe(0);
    await page.getByRole('button', { name: 'Toggle Inspector', exact: true }).click(); await expect(panel).toBeVisible();
    await expect(tabs).toHaveCount(8); await expect.poll(liveWorkers).toBe(2);

    // Keep the PDF active and the Office tab selected in the other logical pane.
    // Reopening narrow must not even send hidden Office derivation demand.
    await pdfPane.getByRole('tab', { name: 'sample.pdf', exact: true }).click();
    await panel.getByRole('button', { name: 'Collapse preview workspace', exact: true }).click();
    await expect.poll(liveWorkers).toBe(0);
    await page.setViewportSize({ width: 390, height: 844 });
    const readsBeforeOfficeReveal = reads.length, derivationsBeforeReveal = derivations.length;
    expect(derivationsBeforeReveal).toBeGreaterThan(0);
    await page.getByRole('button', { name: 'Reopen previews', exact: true }).click();
    await expect(panel.locator('.textLayer')).toHaveCount(1); await expect.poll(liveWorkers).toBe(1);
    expect(reads.length - readsBeforeOfficeReveal).toBe(1);
    expect(derivations.length).toBe(derivationsBeforeReveal);
    await page.setViewportSize({ width: 1920, height: 1000 });
    await expect(panel.locator('.textLayer')).toHaveCount(2); await expect.poll(liveWorkers).toBe(2);
    await officePane.getByRole('tab', { name: 'sample.docx', exact: true }).click();

    await page.setViewportSize({ width: 390, height: 844 });
    await expect(workspace.locator('[data-preview-pane]:visible')).toHaveCount(1); await expect.poll(liveWorkers).toBe(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    const strip = workspace.locator('[data-preview-active] [role="tablist"]');
    await expect.poll(() => strip.evaluate(element => element.scrollWidth - element.clientWidth)).toBeGreaterThan(0);
    const selectedBeforeOverflow = await activeTab.getAttribute('data-preview-tab');
    const selectedDocument = workspace.locator('[data-preview-active] .textLayer');
    await expect(selectedDocument).toContainText('rustX document preview');
    const selectedDocumentText = await selectedDocument.innerText();
    const focusedItemBounds = () => strip.evaluate(element => {
      const item = document.activeElement?.closest('[data-preview-occurrence]');
      if (!item) throw new Error('Focused preview occurrence is missing');
      const itemBox = item.getBoundingClientRect(), stripBox = element.getBoundingClientRect();
      return { left: Math.floor(itemBox.left), right: Math.ceil(itemBox.right), minimum: Math.floor(stripBox.left), maximum: Math.ceil(stripBox.right), scroll: element.scrollLeft,
        rawLeft: itemBox.left, rawRight: itemBox.right, rawMinimum: stripBox.left, rawMaximum: stripBox.right, scrollWidth: element.scrollWidth, clientWidth: element.clientWidth };
    });
    await activeTab.focus(); await page.keyboard.press('End');
    await expect(strip.getByRole('tab').last()).toBeFocused();
    const endBounds = await focusedItemBounds(); expect(endBounds.scroll).toBeGreaterThan(0);
    expect(endBounds.left, JSON.stringify(endBounds)).toBeGreaterThanOrEqual(endBounds.minimum); expect(endBounds.right, JSON.stringify(endBounds)).toBeLessThanOrEqual(endBounds.maximum);
    await page.keyboard.press('Home'); await expect(strip.getByRole('tab').first()).toBeFocused();
    const homeBounds = await focusedItemBounds();
    expect(homeBounds.left, JSON.stringify(homeBounds)).toBeGreaterThanOrEqual(homeBounds.minimum); expect(homeBounds.right, JSON.stringify(homeBounds)).toBeLessThanOrEqual(homeBounds.maximum);
    expect(await activeTab.getAttribute('data-preview-tab')).toBe(selectedBeforeOverflow);
    await expect(activeTab).toHaveText('sample.docx'); await expect.poll(liveWorkers).toBe(1);
    expect(await selectedDocument.innerText()).toBe(selectedDocumentText);
    const narrowSplit = panel.getByRole('button', { name: 'Split preview', exact: true });
    await narrowSplit.focus(); await expect(narrowSplit).toHaveAttribute('aria-disabled', 'true');
    await expect(narrowSplit).toHaveAccessibleDescription('Two panes need at least 608 px. Expand the panel or use fullscreen.');
    await page.keyboard.press('Enter'); await expect(panes).toHaveCount(2);
    await panel.getByRole('button', { name: 'Switch preview pane', exact: true }).focus(); await page.keyboard.press('Enter');
    await expect(activeTab).toBeFocused(); await expect.poll(liveWorkers).toBe(1);
    await panel.getByRole('button', { name: 'Collapse preview workspace', exact: true }).focus(); await page.keyboard.press('Enter'); await expect.poll(liveWorkers).toBe(0);
    await page.setViewportSize({ width: 1920, height: 1000 }); await expect(panel).not.toBeVisible();
    await page.getByRole('button', { name: 'Reopen previews', exact: true }).click(); await expect(panes).toHaveCount(2); await expect.poll(liveWorkers).toBe(2);

    // A clipped Session shell must never become a second scroll owner, even
    // while resized descendants temporarily extend beyond its bounds.
    const shellScroll = await page.locator('#session-view').evaluate(element => {
      const overflow = document.createElement('div');
      overflow.style.cssText = 'position:absolute;top:100%;height:32px;width:1px;pointer-events:none';
      element.append(overflow);
      element.scrollTop = 11;
      const top = element.scrollTop;
      overflow.remove();
      return top;
    });
    expect(shellScroll).toBe(0);
    const conversation = page.locator('[data-conversation-scroll]').first();
    await conversation.evaluate(element => { element.scrollTop = 0; element.dispatchEvent(new Event('scroll')); });
    await expect(page.getByRole('button', { name: 'Return to latest', exact: true })).toBeVisible();
    const anchor = page.getByRole('article', { name: 'Your message', exact: true }).getByText('Preview workspace files', { exact: true });
    const top = await anchor.evaluate(element => element.getBoundingClientRect().top);
    await panel.getByRole('button', { name: 'Fullscreen preview', exact: true }).click();
    await panel.getByRole('button', { name: 'Restore preview', exact: true }).click();
    await fixture.release('workspace-stream'); await expect(page.getByText('Workspace files delivered. New streamed workspace content.', { exact: true })).toBeVisible();
    await expect.poll(() => anchor.evaluate(element => element.getBoundingClientRect().top)).toBeCloseTo(top, 0);
    await expect(page.getByRole('button', { name: 'Return to latest', exact: true })).toBeVisible();
    expect(wire.requests.filter(request => request.method === 'turn/cancel')).toHaveLength(0);
    const beforeSession = await tabs.evaluateAll(elements => elements.map(element => element.getAttribute('data-preview-tab')));
    const readsBeforeSession = reads.length;
    const sessionB = await openEmptySession(page, fixture, 'Workspace B');
    await expect.poll(liveWorkers).toBe(0); await expect.poll(liveUrls).toBe(0);
    await page.locator(`button[data-session-id="${sessionA}"]`).click();
    await expect(panel).toBeVisible(); await expect.poll(liveWorkers).toBe(2);
    expect(await tabs.evaluateAll(elements => elements.map(element => element.getAttribute('data-preview-tab')))).toEqual(beforeSession);
    await page.getByRole('button', { name: 'Toggle Inspector', exact: true }).click();
    await page.getByRole('button', { name: 'Close Inspector', exact: true }).click();
    await expect.poll(liveWorkers).toBe(0);
    await page.locator(`button[data-session-id="${sessionB}"]`).click();
    await page.locator(`button[data-session-id="${sessionA}"]`).click();
    await expect(panel).not.toBeVisible(); await expect.poll(liveWorkers).toBe(0);
    await page.setViewportSize({ width: 1800, height: 1000 }); await expect(panel).not.toBeVisible();
    await page.getByRole('button', { name: 'Reopen previews', exact: true }).click();
    await expect(panel).toBeVisible(); await expect.poll(liveWorkers).toBe(2);
    expect(await tabs.evaluateAll(elements => elements.map(element => element.getAttribute('data-preview-tab')))).toEqual(beforeSession);

    const retainedTextPane = panes.filter({ has: page.getByRole('tab', { name: 'notes.txt', exact: true }) });
    const selectedBeforeViewCheck = await retainedTextPane.locator('[role="tab"][aria-selected="true"]').getAttribute('data-preview-tab');
    await retainedTextPane.getByRole('tab', { name: 'notes.txt', exact: true }).click();
    await expect(retainedTextPane.getByRole('button', { name: 'Wrap lines', exact: true })).toHaveAttribute('aria-pressed', 'false');
    await expect.poll(() => retainedTextPane.locator('[data-preview-scroll="body"]').evaluate(element => element.scrollTop)).toBe(420);
    await retainedTextPane.locator(`[data-preview-tab="${selectedBeforeViewCheck}"]`).click();
    await expect.poll(liveWorkers).toBe(2);
    expect(reads.length).toBeGreaterThan(readsBeforeSession);

    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.getByRole('group', { name: 'Appearance', exact: true }).getByRole('button', { name: 'Dark', exact: true }).click();
    await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
    await page.getByRole('button', { name: '关闭设置', exact: true }).click();
    const chinesePanel = page.getByRole('complementary', { name: '预览', exact: true });
    await expect(chinesePanel.locator('.textLayer').nth(0)).toContainText('rustX');
    await expect(chinesePanel.locator('.textLayer').nth(1)).toContainText('rustX');
    await page.screenshot({ animations: 'disabled', path: info.outputPath('workspace-two-heavy-dark-zh.png') });
    await info.attach('workspace-two-heavy-dark-zh', { path: info.outputPath('workspace-two-heavy-dark-zh.png'), contentType: 'image/png' });
    await expect(chinesePanel).toBeVisible();
    while (await tabs.count()) await workspace.getByRole('button', { name: /^关闭预览 / }).first().click();
    await expect(chinesePanel).not.toBeVisible(); await expect.poll(liveWorkers).toBe(0); await expect.poll(liveUrls).toBe(0);
    expect(await page.evaluate(() => document.activeElement !== document.body && !document.activeElement?.closest('[inert]'))).toBe(true);
    await page.getByRole('button', { name: '预览 sample.pdf', exact: true }).first().click();
    await expect.poll(liveWorkers).toBe(1);
    expect(Number(await tabs.first().getAttribute('data-preview-tab'))).toBeGreaterThan(Math.max(...beforeSession.map(Number)));
    await tabs.first().focus(); await page.keyboard.press('Delete');
    await expect.poll(liveWorkers).toBe(0); await expect.poll(liveUrls).toBe(0);
    await expect(page.getByRole('button', { name: '预览 sample.pdf', exact: true }).first()).toBeFocused();
    const counts = await page.evaluate(() => { const state = (window as any).previewObservation; return { workers: state.maximumWorkers, urls: state.maximumUrls, duplicate: state.duplicateRevocations }; });
    expect(counts).toEqual({ workers: 2, urls: 3, duplicate: 0 });
    expect((await fixture.control('requests')).requests).toHaveLength(modelRequests);
    expect(wire.requests.filter(request => request.method === 'turn/start')).toHaveLength(1);
    expect(errors).toEqual([]); passed = true;
  } finally { await fixture.stop(passed); }
});
