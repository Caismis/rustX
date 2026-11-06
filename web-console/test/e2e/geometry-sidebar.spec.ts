import { test, expect, type Page } from '@playwright/test';
import { copyFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { choose, connectRemote, openEmptySession, expectSettled } from './shell-actions';
import { wireProbe } from './wire-probe';
import { instrument, observeGeometryLatency, resources } from './geometry-observation';

test.use({ connectOptions: { wsEndpoint: process.env.RUSTX_BROWSER_WS_ENDPOINT ?? '', exposeNetwork: '<loopback>' } });

function displayDuration(start: number) {
  const value = Math.round((performance.now() - start) * 10) / 10;
  expect(Number.isFinite(value)).toBe(true); expect(value).toBeGreaterThanOrEqual(0); return value;
}
const painted = (page: Page) => page.locator('[data-geometry-format]:visible canvas').evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));

/** Observe creation as well as live counts without changing admission or timing. */
async function observeResidency(page: Page) {
  await page.addInitScript(() => {
    const seen = new WeakSet<object>(), state = { createdContexts: 0, liveContexts: 0, peakContexts: 0, workersStarted: 0 };
    const getContext = HTMLCanvasElement.prototype.getContext;
    Object.defineProperty(HTMLCanvasElement.prototype, 'getContext', { value: function (this: HTMLCanvasElement, ...args: unknown[]) {
      const result = Reflect.apply(getContext, this, args);
      if (args[0] === 'webgl2' && result && !seen.has(result)) {
        seen.add(result); state.createdContexts++; state.liveContexts++; state.peakContexts = Math.max(state.peakContexts, state.liveContexts);
        this.addEventListener('webglcontextlost', () => { state.liveContexts--; }, { once: true });
      }
      return result;
    } });
    const NativeWorker = window.Worker;
    window.Worker = class extends NativeWorker {
      constructor(url: string | URL, options?: WorkerOptions) { super(url, options); state.workersStarted++; }
    };
    Object.assign(window, { geometryResidency: state });
  });
}
const residency = (page: Page) => page.evaluate(() => ({ ...(window as any).geometryResidency, nativeImports: (window as any).geometryImports.length }));
const frameOpportunity = (page: Page) => page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));

async function deliveredModel(page: Page, largeStep?:string) {
  const fixture = await startDogfood('web_geometry_previews');
  try {
    for (const name of ['mounting-bracket.step', 'mounting-bracket.glb']) copyFileSync(new URL(`../fixtures/geometry/${name}`, import.meta.url), join(fixture.workspaceA, name));
    if(largeStep)copyFileSync(largeStep,join(fixture.workspaceA,'mounting-bracket.step'));
    writeFileSync(join(fixture.workspaceA, 'notes.md'), '# Model review\n\nInspect the shape and its parts in the sidebar.');
    const wire = await wireProbe(page);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Inspect the CAD model');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByText('The model files are ready to inspect.', { exact: true })).toBeVisible();
    await expectSettled(page, wire);
    return fixture;
  } catch (error) { await fixture.stop(false); throw error; }
}

test('approximately 200 MB STEP has an admitted whole-model first picture through the production Host',async({page},info)=>{
  const source=process.env.RUSTX_GEOMETRY_LARGE_STEP_PATH;test.skip(!source,'Provide an original approximately 200 MB STEP for native transport acceptance.');
  await instrument(page);const fixture=await deliveredModel(page,source);let passed=false;
  try{await observeGeometryLatency(page);await page.getByRole('button',{name:'Preview mounting-bracket.step in sidebar',exact:true}).click();
    await expect.poll(()=>page.evaluate(()=>(window as any).geometryLatency.firstModel),{timeout:70000}).toBeGreaterThan(0);
    const firstModelMs=await page.evaluate(()=>(window as any).geometryLatency.firstModel as number),initialCanvas=page.locator('[data-geometry-format] canvas');
    await initialCanvas.screenshot({path:info.outputPath('step-native-first-model.png')});
    const area=(await initialCanvas.boundingBox())!;
    for(const button of ['left','right'] as const)for(let action=0;action<5;action++){const x=area.x+area.width/2,y=area.y+area.height/2,direction=action%2?-1:1;await page.mouse.move(x,y);await page.mouse.down({button});await page.mouse.move(x+32*direction,y+12*direction,{steps:10});await page.mouse.up({button});}
    await expect.poll(()=>page.evaluate(()=>(window as any).geometryLatency.pending)).toBe(0);
    const samples=await page.evaluate(()=>(window as any).geometryLatency.lodSamples.concat((window as any).geometryLatency.samples) as number[]);
    const priorFineSamples=await page.evaluate(()=>(window as any).geometryLatency.samples.length as number);
    await expect.poll(()=>page.evaluate(()=>(window as any).geometryLatency.first),{timeout:70000}).toBeGreaterThan(0);
    const canvas=page.locator('[data-geometry-format] canvas'),fineArea=(await canvas.boundingBox())!;
    await canvas.screenshot({path:info.outputPath('step-native-handoff.png')});
    for(const button of ['left','right'] as const)for(let action=0;action<5;action++){const x=fineArea.x+fineArea.width/2,y=fineArea.y+fineArea.height/2,direction=action%2?-1:1;await page.mouse.move(x,y);await page.mouse.down({button});await page.mouse.move(x+32*direction,y+12*direction,{steps:10});await page.mouse.up({button});}
    await expect.poll(()=>page.evaluate(()=>(window as any).geometryLatency.pending)).toBe(0);
    const fineSamples=await page.evaluate(()=>(window as any).geometryLatency.samples.slice(priorFineSamples) as number[]);
    await page.getByRole('button',{name:'View angle',exact:true}).click();await page.getByRole('menuitem',{name:'Reset view',exact:true}).click();
    await expect(canvas).toHaveAttribute('data-geometry-quality','complete');
    const originalTriangles=await page.evaluate(()=>(window as any).geometryImports.at(-1).reduce((sum:number,mesh:any)=>sum+mesh.triangles,0));
    await expect.poll(()=>page.evaluate(()=>(window as any).geometryDrawCalls.lastModelTriangles)).toBe(originalTriangles);
    const fineFirstMs=await page.evaluate(()=>(window as any).geometryLatency.first);
    await page.screenshot({path:info.outputPath('step-native-complete.png')});
    await page.getByRole('button',{name:'Close preview mounting-bracket.step',exact:true}).click();
    const afterClose={workers:0,contexts:0,buffers:0,textures:0,urls:0};await expect.poll(()=>resources(page)).toEqual(afterClose);
    const result={firstModelMs,lodFirstMs:await page.evaluate(()=>(window as any).geometryLatency.lodFirst||null),fineFirstMs,firstModelInteractions:samples,maximumFirstModelInteractionMs:Math.max(...samples),fineInteractions:fineSamples,maximumFineInteractionMs:Math.max(...fineSamples),originalTriangles,afterClose:await resources(page),phases:await page.evaluate(()=>performance.getEntriesByType('measure').filter(x=>x.name.startsWith('geometry:')).map(x=>({name:x.name,ms:x.duration}))),method:'Actual committed delivery, Product Host source authorization and native contiguous bounded reads. First admitted whole-model CAD LOD or complete mesh, plus complete-model GPU/next-rAF proxies are recorded.'};
    writeFileSync(info.outputPath('step-native-first.json'),JSON.stringify(result,null,2));await info.attach('step-native-first',{body:JSON.stringify(result,null,2),contentType:'application/json'});
    expect(samples.length).toBeGreaterThanOrEqual(80);expect(fineSamples.length).toBeGreaterThanOrEqual(80);if(process.env.RUSTX_GEOMETRY_BUDGET==='1'){expect.soft(firstModelMs).toBeLessThanOrEqual(200);expect.soft(Math.max(...samples,...fineSamples)).toBeLessThan(50);}passed=true;
  }finally{await page.close();await fixture.workspaceHost.stop();await fixture.stop(passed);}
});

test('delivery CAD occupies the existing document sidebar without clipping on a short window', async ({ page }, info) => {
  await instrument(page); await observeResidency(page);
  let sourceReads = 0;
  page.on('request', request => { if (/\/(?:file-read|workbench-preview)$/.test(new URL(request.url()).pathname)) sourceReads++; });
  const fixture = await deliveredModel(page); let passed = false;
  await observeGeometryLatency(page);
  const timings: { entry: string; name: string; phase: string; displayMs: number; firstFrameReadyProxyMs?: number }[] = [];
  try {
    let start = performance.now();
    await page.getByRole('button', { name: 'Preview mounting-bracket.step in sidebar', exact: true }).click();
    const sidebar = page.getByRole('complementary', { name: 'Previews', exact: true });
    await expect(sidebar).toBeVisible();
    await expect(sidebar).toHaveAttribute('data-sidebar-right-panel', 'normal');
    await expect(sidebar.getByRole('tab', { name: 'mounting-bracket.step', exact: true })).toBeVisible();
    await expect(sidebar.locator('[data-geometry-format] canvas')).toBeVisible();
    await expect(sidebar.locator('[data-geometry-format]')).toContainText('120 × 72 × 80');
    await painted(page); const displayMs = displayDuration(start);
    await expect.poll(() => page.evaluate(() => (window as any).geometryLatency.first), { timeout: 20_000 }).toBeGreaterThan(0);
    timings.push({ entry: 'delivery', name: 'mounting-bracket.step', phase: 'first-open', displayMs, firstFrameReadyProxyMs: await page.evaluate(() => (window as any).geometryLatency.first) });
    const controls = sidebar.getByRole('toolbar', { name: 'Model controls', exact: true });
    await expect(controls).toHaveAttribute('aria-orientation', 'vertical');
    const toolsBox = (await controls.boundingBox())!, canvasBox = (await sidebar.locator('[data-geometry-format] canvas').boundingBox())!;
    expect(toolsBox.width).toBeLessThanOrEqual(40); expect(toolsBox.x - canvasBox.x).toBeLessThanOrEqual(20);
    await expect(sidebar.getByRole('button', { name: 'Refresh', exact: true })).toBeVisible();
    await expect(sidebar.getByRole('button', { name: 'Download artifact', exact: true })).toBeVisible();
    await expect(sidebar.getByRole('note')).toHaveCount(0);
    const parts = sidebar.getByRole('button', { name: 'Parts', exact: true });
    await expect(parts).toHaveAttribute('aria-expanded', 'false');
    await expect(sidebar.getByRole('list', { name: 'Parts', exact: true })).toHaveCount(0);
    await page.screenshot({ path: info.outputPath('geometry-sidebar-light.png'), animations: 'disabled' });
    const canvas = sidebar.locator('[data-geometry-format] canvas'), beforeOrbit = await canvas.screenshot();
    const orbit = (await canvas.boundingBox())!;
    await page.mouse.move(orbit.x + orbit.width / 2, orbit.y + orbit.height / 2); await page.mouse.down();
    await page.mouse.move(orbit.x + orbit.width / 2 + 40, orbit.y + orbit.height / 2 + 18, { steps: 8 }); await page.mouse.up();
    expect((await canvas.screenshot()).equals(beforeOrbit)).toBe(false);
    await sidebar.getByRole('button', { name: 'Fit view', exact: true }).click();
    await parts.click();
    const expand = sidebar.getByRole('button', { name: /^(?:Expand|Collapse): Mounting bracket$/ });
    await expect(expand).toHaveAttribute('aria-expanded', 'false');
    await expand.click();
    await sidebar.getByRole('button', { name: 'Base plate', exact: true }).click();
    await expect(sidebar.getByRole('button', { name: 'Base plate', exact: true })).toHaveAttribute('aria-pressed', 'true');
    const base = sidebar.getByRole('checkbox', { name: 'Visible: Base plate', exact: true });
    await base.uncheck(); await expect(base).not.toBeChecked(); await base.check();
    await page.screenshot({ path: info.outputPath('geometry-sidebar-parts.png'), animations: 'disabled' });
    await sidebar.getByRole('button', { name: 'Close parts', exact: true }).click();
    await expect(parts).toBeFocused();
    await page.setViewportSize({ width: 1440, height: 480 });
    await page.screenshot({ path: info.outputPath('geometry-sidebar-short.png'), animations: 'disabled' });
    await expect.poll(() => sidebar.locator('[data-geometry-format]').evaluate(element => {
      const panel = element.closest('[data-preview-pane]')!.getBoundingClientRect();
      const model = element.getBoundingClientRect();
      return Math.max(model.bottom - panel.bottom, element.scrollHeight - element.clientHeight);
    })).toBeLessThanOrEqual(1);
    await parts.click();
    await expect.poll(() => sidebar.getByRole('list', { name: 'Parts', exact: true }).evaluate(element => {
      const stage = element.closest('[data-geometry-format]')!.querySelector('canvas')!.getBoundingClientRect();
      const tree = element.parentElement!.getBoundingClientRect();
      return Math.max(stage.top - tree.top, tree.bottom - stage.bottom, stage.left - tree.left, tree.right - stage.right);
    })).toBeLessThanOrEqual(0);
    await page.keyboard.press('Escape'); await expect(parts).toHaveAttribute('aria-expanded', 'false'); await expect(sidebar).toBeVisible();
    await page.setViewportSize({ width: 1440, height: 1000 });
    // Preserve an intentionally open tree, expanded branch and selection too.
    await parts.click(); await expect(parts).toHaveAttribute('aria-expanded', 'true');
    await expect(expand).toHaveAttribute('aria-expanded', 'true');
    const step = sidebar.locator('[data-geometry-format="step"]'), stepCanvas = step.locator('canvas');
    await expect(stepCanvas).toHaveAttribute('data-geometry-quality', 'complete');
    await expect.poll(async () => (await resources(page)).workers).toBe(0);
    const originalStepCanvas = (await stepCanvas.elementHandle())!, initialResidency = await residency(page);
    expect(initialResidency.createdContexts).toBe(1); expect(initialResidency.liveContexts).toBe(1);
    await page.getByRole('button', { name: 'Preview notes.md in sidebar', exact: true }).click();
    await expect(sidebar.getByRole('heading', { name: 'Model review', exact: true })).toBeVisible();
    await expect(step).toHaveCount(1); await expect(step).toBeHidden();
    await expect(step.locator('xpath=ancestor::*[@role="tabpanel"][1]')).toHaveAttribute('inert', '');
    const readsAfterNotes = sourceReads;
    await frameOpportunity(page);
    const hiddenDraws = await page.evaluate(() => (window as any).geometryDrawCalls.calls);
    await page.waitForTimeout(250);
    expect(await page.evaluate(() => (window as any).geometryDrawCalls.calls)).toBe(hiddenDraws);
    expect(await residency(page)).toEqual(initialResidency);
    start = performance.now(); await sidebar.getByRole('tab', { name: 'mounting-bracket.step', exact: true }).click();
    await expect(stepCanvas).toBeVisible();
    expect(await stepCanvas.evaluate((element, original) => element === original, originalStepCanvas)).toBe(true);
    await painted(page); timings.push({ entry: 'delivery', name: 'mounting-bracket.step', phase: 'retained-reopen', displayMs: displayDuration(start) });
    expect(sourceReads).toBe(readsAfterNotes); expect(await residency(page)).toEqual(initialResidency);
    await expect(parts).toHaveAttribute('aria-expanded', 'true'); await expect(expand).toHaveAttribute('aria-expanded', 'true');
    await expect(sidebar.getByRole('button', { name: 'Base plate', exact: true })).toHaveAttribute('aria-pressed', 'true');
    start = performance.now(); await page.getByRole('button', { name: 'Preview mounting-bracket.glb in sidebar', exact: true }).click();
    const glb = sidebar.locator('[data-geometry-format="glb"]'), glbCanvas = glb.locator('canvas');
    await expect(sidebar.locator('[data-geometry-format]:visible')).toContainText('0.12 × 0.072 × 0.08');
    await painted(page); timings.push({ entry: 'delivery', name: 'mounting-bracket.glb', phase: 'first-open', displayMs: displayDuration(start) });
    await expect(parts).toHaveAttribute('aria-expanded', 'false');
    await expect(glbCanvas).toHaveAttribute('data-geometry-quality', 'complete');
    await expect.poll(async () => (await resources(page)).workers).toBe(0);
    const originalGlbCanvas = (await glbCanvas.elementHandle())!, twoModels = await residency(page), readsAfterGlb = sourceReads;
    expect(twoModels.createdContexts).toBe(2); expect(twoModels.liveContexts).toBe(2); expect(twoModels.peakContexts).toBe(2);
    await page.screenshot({ path: info.outputPath('geometry-sidebar-glb.png'), animations: 'disabled' });
    await sidebar.getByRole('tab', { name: 'mounting-bracket.step', exact: true }).click(); await expect(stepCanvas).toBeVisible();
    expect(await stepCanvas.evaluate((element, original) => element === original, originalStepCanvas)).toBe(true);
    await expect(parts).toHaveAttribute('aria-expanded', 'true'); await expect(expand).toHaveAttribute('aria-expanded', 'true');
    await sidebar.getByRole('tab', { name: 'mounting-bracket.glb', exact: true }).click(); await expect(glbCanvas).toBeVisible();
    expect(await glbCanvas.evaluate((element, original) => element === original, originalGlbCanvas)).toBe(true);
    expect(sourceReads).toBe(readsAfterGlb); expect(await residency(page)).toEqual(twoModels);
    const retired = { workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 };
    await sidebar.getByRole('button', { name: 'Collapse preview workspace', exact: true }).click();
    await expect.poll(() => resources(page)).toEqual(retired);
    await page.getByRole('button', { name: 'Preview mounting-bracket.step in sidebar', exact: true }).click(); await expect(stepCanvas).toBeVisible();
    await sidebar.getByRole('tab', { name: 'mounting-bracket.glb', exact: true }).hover();
    const closeGlb = sidebar.getByRole('button', { name: 'Close preview mounting-bracket.glb', exact: true });
    await expect(closeGlb).toHaveCSS('pointer-events', 'auto');
    await closeGlb.click();
    await sidebar.getByRole('button', { name: 'Close preview mounting-bracket.step', exact: true }).click();
    await expect.poll(() => resources(page)).toEqual(retired);
    await info.attach('native-geometry-residency', { body: JSON.stringify({ initialResidency, twoModels, hiddenDraws, readsAfterNotes, readsAfterGlb, retired: await resources(page), method: 'Same canvas identities and unchanged native import/Worker/context creation and source-read counts prove reuse of the authorized copies. A notes tab submits zero hidden geometry draws. Collapse and close release all resources.' }, null, 2), contentType: 'application/json' });
    writeFileSync(info.outputPath('native-display-timings.json'), JSON.stringify(timings, null, 2));
    passed = true;
  } finally { await page.close(); await fixture.workspaceHost.stop(); await fixture.stop(passed); }
});

test('Workbench CAD uses the shared document body and defaults to revealing the original file', async ({ page }, info) => {
  await instrument(page); await observeResidency(page);
  let sourceReads = 0;
  page.on('request', request => { if (/\/(?:file-read|workbench-preview)$/.test(new URL(request.url()).pathname)) sourceReads++; });
  const fixture = await deliveredModel(page); let passed = false;
  try {
    await page.locator('[data-preview-toggle]').click();
    const workbench = page.locator('[data-workbench]');
    await workbench.getByRole('button', { name: /Workspace files/ }).click();
    await observeGeometryLatency(page, '[data-files-path="mounting-bracket.step"] > button');
    const start = performance.now(); await workbench.getByRole('button', { name: 'mounting-bracket.step', exact: true }).click();
    const file = workbench.locator('[data-workbench-file="mounting-bracket.step"]');
    await expect(file.locator('[data-geometry-format] canvas')).toBeVisible();
    await painted(page);
    const displayMs = displayDuration(start);
    await expect.poll(() => page.evaluate(() => (window as any).geometryLatency.first), { timeout: 20_000 }).toBeGreaterThan(0);
    writeFileSync(info.outputPath('native-workbench-display.json'), JSON.stringify({ entry: 'workbench', name: 'mounting-bracket.step', phase: 'first-open', displayMs, firstFrameReadyProxyMs: await page.evaluate(() => (window as any).geometryLatency.first) }, null, 2));
    await page.screenshot({ path: info.outputPath('geometry-workbench.png'), animations: 'disabled' });
    await expect.poll(async () => (await resources(page)).workers).toBe(0);
    const originalCanvas = (await file.locator('canvas').elementHandle())!, beforeSwitch = await residency(page);
    const parts = file.getByRole('button', { name: 'Parts', exact: true });
    await parts.click();
    const branch = file.getByRole('button', { name: /^(?:Expand|Collapse): Mounting bracket$/ });
    await branch.click();
    await workbench.getByRole('tab', { name: 'Files', exact: false }).click();
    await workbench.getByRole('button', { name: 'notes.md', exact: true }).click();
    await expect(workbench.getByRole('heading', { name: 'Model review', exact: true })).toBeVisible();
    await expect(file).toHaveCount(1); await expect(file).toBeHidden();
    const readsAfterNotes = sourceReads;
    await frameOpportunity(page);
    const hiddenDraws = await page.evaluate(() => (window as any).geometryDrawCalls.calls);
    await page.waitForTimeout(250);
    expect(await page.evaluate(() => (window as any).geometryDrawCalls.calls)).toBe(hiddenDraws);
    await workbench.getByRole('tab', { name: 'mounting-bracket.step', exact: false }).click();
    await expect(file.locator('canvas')).toBeVisible();
    expect(await file.locator('canvas').evaluate((element, original) => element === original, originalCanvas)).toBe(true);
    expect(sourceReads).toBe(readsAfterNotes); expect(await residency(page)).toEqual(beforeSwitch);
    await expect(parts).toHaveAttribute('aria-expanded', 'true'); await expect(branch).toHaveAttribute('aria-expanded', 'true');
    await file.getByRole('button', { name: 'Close parts', exact: true }).click();
    // The existing later mobile check starts from the original collapsed tree.
    await parts.click();
    await file.getByRole('button', { name: 'Collapse: Mounting bracket', exact: true }).click();
    await expect(branch).toHaveAttribute('aria-expanded', 'false');
    await file.getByRole('button', { name: 'Close parts', exact: true }).click();
    await info.attach('native-workbench-residency', { body: JSON.stringify({ beforeSwitch, afterSwitch: await residency(page), readsAfterNotes, hiddenDraws }, null, 2), contentType: 'application/json' });
    await expect(file.getByRole('button', { name: 'Open file location', exact: true })).toBeVisible();
    await expect(file.getByRole('button', { name: 'Choose application', exact: true })).toBeVisible();
    await expect(file.getByRole('note')).toHaveCount(0);
    await expect(file.getByRole('button', { name: 'Refresh', exact: true })).toBeVisible();
    await expect(file.getByRole('button', { name: 'Parts', exact: true })).toHaveAttribute('aria-expanded', 'false');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.getByRole('group', { name: 'Appearance', exact: true }).getByRole('button', { name: 'Dark', exact: true }).click();
    await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
    await page.getByRole('button', { name: '关闭设置', exact: true }).click();
    await page.screenshot({ path: info.outputPath('geometry-workbench-zh-dark.png'), animations: 'disabled' });
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(file.getByRole('button', { name: '打开文件位置', exact: true })).toBeVisible();
    await expect(file.getByRole('button', { name: '选择应用', exact: true })).toBeVisible();
    await expect(file.locator('[data-geometry-format] canvas')).toBeVisible();
    await expect(file.getByRole('note')).toHaveCount(0);
    await page.screenshot({ path: info.outputPath('geometry-sidebar-zh-dark-narrow.png'), animations: 'disabled' });
    await file.getByRole('button', { name: '部件', exact: true }).click();
    await expect(file.getByRole('button', { name: '展开: Mounting bracket', exact: true })).toHaveAttribute('aria-expanded', 'false');
    await page.screenshot({ path: info.outputPath('geometry-sidebar-parts-zh-dark-narrow.png'), animations: 'disabled' });
    await file.getByRole('button', { name: '收起部件', exact: true }).click();
    await expect(file.getByRole('button', { name: '部件', exact: true })).toBeFocused();
    passed = true;
  } finally { await page.close(); await fixture.workspaceHost.stop(); await fixture.stop(passed); }
});
