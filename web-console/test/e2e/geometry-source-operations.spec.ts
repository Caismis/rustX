import { test, expect, type Page, type Request } from '@playwright/test';
import { copyFileSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { routeWorkspaceHost, type WorkspaceHostResponse } from './workspace-host';
import { connectRemote, openEmptySession, expectSettled } from './shell-actions';
import { wireProbe } from './wire-probe';
import { instrument, resources } from './geometry-observation';
import { denseBoxStl } from '../geometry-fixtures';
import { FILE_PREVIEW_LIMITS } from '../../shared/file-preview';

test.use({ connectOptions: { wsEndpoint: process.env.RUSTX_BROWSER_WS_ENDPOINT ?? '', exposeNetwork: '<loopback>' } });
const names = ['cube.stl', 'cube.glb', 'cube.step', 'cube.iges', 'cube.brep'];
type Record = { method: string; request: any; result: any };
function terminal(response: WorkspaceHostResponse) {
  const boundary = /boundary=([^;]+)/.exec(response.headers['content-type'] ?? '')?.[1];
  if (!boundary) return JSON.parse(response.body.toString('utf8'));
  const start = response.body.indexOf('\r\n\r\n') + 4, end = response.body.indexOf(`\r\n--${boundary}`, start);
  if (start < 4 || end < start) throw new Error('Missing physical terminal envelope');
  return JSON.parse(response.body.subarray(start, end).toString('utf8'));
}
async function fixture(page: Page, observed: (request: Request, response: WorkspaceHostResponse) => void | Promise<void>, stl?: Uint8Array) {
  const fixture = await startDogfood('web_geometry_streams');
  try {
    names.forEach(name => copyFileSync(new URL(`../fixtures/geometry/${name}`, import.meta.url), join(fixture.workspaceA, name)));
    if (stl) writeFileSync(join(fixture.workspaceA, 'cube.stl'), stl);
    writeFileSync(join(fixture.workspaceA, 'notes.md'), '# Source review\n\nThe geometry copy stays resident.');
    const wire = await wireProbe(page); await instrument(page); await routeWorkspaceHost(page, fixture, undefined, observed);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Inspect the geometry sources');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByText('The geometry sources are ready to inspect.', { exact: true })).toBeVisible(); await expectSettled(page, wire);
    return fixture;
  } catch (error) { await fixture.stop(false); throw error; }
}
const method = (request: Request) => new URL(request.url()).pathname.split('/').at(-1)!;
function assertOperation(records: Record[], name: string, entry: string) {
  const opened = records.find(record => record.method === 'preview-open'); expect(opened?.result.ok).toBe(true);
  expect(opened?.request.source).toBe(entry === 'delivery' ? 'delivery' : 'workspace');
  const bound = opened!.result.value; expect(bound.bytes).toBe(readFileSync(new URL(`../fixtures/geometry/${name}`, import.meta.url)).length);
  if (entry === 'delivery') expect(bound.file.name).toBe(name); else expect(opened!.request.call.request.path).toBe(name);
  const pages = records.filter(record => record.method === 'preview-read'); expect(pages).toHaveLength(Math.ceil(bound.bytes / FILE_PREVIEW_LIMITS.pageBytes));
  for (const page of pages) {
    expect(page.request.identity).toBe(bound.identity); expect(page.result.ok).toBe(true); expect(page.result.attachment).toBe('data');
    expect(page.result.value).toMatchObject({ version: bound.version, bytes: bound.bytes, offset: page.request.range.offset });
    expect(page.request.range.length).toBeLessThanOrEqual(FILE_PREVIEW_LIMITS.pageBytes);
  }
  const closed = records.find(record => record.method === 'preview-close'); expect(closed?.request.identity).toBe(bound.identity); expect(closed?.result.ok).toBe(true);
  expect(records.filter(record => record.method === 'preview-open')).toHaveLength(1); expect(records.filter(record => record.method === 'preview-close')).toHaveLength(1);
}

for (const entry of ['delivery', 'workspace']) test(`real ${entry} geometry formats share bound source operations and retain the STL copy`, async ({ page }, info) => {
  const records: Record[] = [], errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  const native = await fixture(page, (request, response) => { const name = method(request); if (name.startsWith('preview-') || name === 'file-read' || name === 'workbench-preview') records.push({ method: name, request: request.postDataJSON(), result: terminal(response) }); });
  let passed = false;
  try {
    if (entry === 'delivery') await page.getByRole('button', { name: 'Show all 6 delivered files', exact: true }).click();
    else { await page.locator('[data-preview-toggle]').click(); await page.locator('[data-workbench]').getByRole('button', { name: /Workspace files/ }).click(); }
    const open = async (name: string) => {
      if (entry === 'delivery') await page.getByRole('button', { name: `Preview ${name} in sidebar`, exact: true }).click();
      else {
        const workbench = page.locator('[data-workbench]'), files = workbench.getByRole('tab', { name: 'Files', exact: false });
        if (await files.isVisible()) await files.click(); await workbench.getByRole('button', { name, exact: true }).click();
      }
    };
    for (const name of names) {
      const before = records.length; await open(name);
      await expect(page.locator(`[data-geometry-format="${name.split('.').at(-1)}"]:visible`)).toContainText('10 × 10 × 10', { timeout: 20_000 });
      await expect.poll(() => resources(page)).toMatchObject({ workers: 0 }); assertOperation(records.slice(before), name, entry);
      if (name === 'cube.stl') {
        const canvas = page.locator('[data-geometry-format]:visible canvas'), retained = (await canvas.elementHandle())!;
        await open('notes.md'); await expect(page.getByRole('heading', { name: 'Source review', exact: true })).toBeVisible();
        const prior = records.filter(record => record.method === 'preview-open').length;
        if (entry === 'delivery') await page.getByRole('tab', { name: 'cube.stl', exact: true }).click();
        else await page.locator('[data-workbench]').getByRole('tab', { name: 'cube.stl', exact: false }).click();
        await expect(canvas).toBeVisible(); expect(await canvas.evaluate((current, previous) => current === previous, retained)).toBe(true);
        expect(records.filter(record => record.method === 'preview-open')).toHaveLength(prior);
      }
    }
    const legacy = records.filter(record => record.method === 'file-read' || record.method === 'workbench-preview');
    expect(legacy.filter(record => (record.request.read?.read ?? record.request.call?.request.read)?.kind === 'bytes').every(record => (record.request.read?.read ?? record.request.call?.request.read).range?.length === 1)).toBe(true);
    expect(errors).toEqual([]); await info.attach('source-operation-contracts', { body: JSON.stringify(records, null, 2), contentType: 'application/json' }); passed = true;
  } finally { await stopDogfoodPage(page, native, passed); }
});

function gate() {
  let release!: () => void, entered!: () => void;
  const pending = new Promise<void>(yes => { release = yes; }), ready = new Promise<void>(yes => { entered = yes; });
  return { pending, ready, release, entered };
}
function scaledStl(bytes: Uint8Array, scale: number) {
  const next = Uint8Array.from(bytes), view = new DataView(next.buffer);
  for (let triangle = 0; triangle < view.getUint32(80, true); triangle++) for (let coordinate = 0; coordinate < 9; coordinate++) {
    const at = 84 + triangle * 50 + 12 + coordinate * 4; view.setFloat32(at, view.getFloat32(at, true) * scale, true);
  }
  return next;
}
for (const entry of ['delivery', 'workspace']) test(`real ${entry} STL validates at close, retires canceled reads and permits a new copy`, async ({ page }, info) => {
  const records: Record[] = [], read = gate(), close = gate(); let holdRead = false, heldReads = 0, holdClose = true;
  const native = await fixture(page, async (request, response) => {
    if (holdRead && method(request) === 'preview-read') {
      // Actual controlled response latency lets the production adaptive window
      // observe transport wait before the two late-response settlement gates.
      if (request.postDataJSON().range.offset < 4 * FILE_PREVIEW_LIMITS.pageBytes) await new Promise(resolve => setTimeout(resolve, 500));
      else { if (++heldReads === 2) { holdRead = false; read.entered(); } await read.pending; }
    }
    if (method(request).startsWith('preview-')) records.push({ method: method(request), request: request.postDataJSON(), result: terminal(response) });
  }, denseBoxStl());
  const routed = new Set<Promise<void>>();
  let passed = false;
  try {
    await page.evaluate(() => {
      const original = HTMLCanvasElement.prototype.getContext, seen = new WeakSet<object>(), contexts: { lost: boolean }[] = [];
      Object.defineProperty(HTMLCanvasElement.prototype, 'getContext', { value: function (this: HTMLCanvasElement, ...args: unknown[]) {
        const value = Reflect.apply(original, this, args);
        if (value && args[0] === 'webgl2' && !seen.has(value)) { seen.add(value); const state = { lost: false }; contexts.push(state); this.addEventListener('webglcontextlost', () => { state.lost = true; }, { once: true }); }
        return value;
      } });
      Object.assign(window, { sourceContextRetirement: contexts });
    });
    await page.route('**/product-host/preview-close', route => {
      const settling = (async () => { if (holdClose) { holdClose = false; close.entered(); await close.pending; } await route.fallback(); })();
      routed.add(settling); return settling.finally(() => { routed.delete(settling); });
    });
    const workbench = page.locator('[data-workbench]');
    if (entry === 'workspace') { await page.locator('[data-preview-toggle]').click(); await workbench.getByRole('button', { name: /Workspace files/ }).click(); }
    const open = async () => {
      if (entry === 'delivery') await page.getByRole('button', { name: 'Preview cube.stl in sidebar', exact: true }).click();
      else {
        const files = workbench.getByRole('tab', { name: 'Files', exact: false }); if (await files.isVisible()) await files.click();
        await workbench.getByRole('button', { name: 'cube.stl', exact: true }).click();
      }
    };
    await open(); await close.ready;
    await expect(page.locator('[data-geometry-format] canvas')).toHaveCount(0);
    expect((await resources(page)).contexts).toBeLessThanOrEqual(1);
    expect(await page.evaluate(() => ({ imports: (window as any).geometryImports.length, draws: (window as any).geometryDrawCalls.modelCalls }))).toEqual({ imports: 0, draws: 0 });
    const original = records.find(record => record.method === 'preview-open')!.result.value;
    writeFileSync(join(native.workspaceA, 'cube.stl'), scaledStl(readFileSync(join(native.workspaceA, 'cube.stl')), 2));
    close.release();
    await expect.poll(() => records.find(record => record.method === 'preview-close' && record.request.identity === original.identity)?.result.ok).toBe(false);
    const rejected = records.find(record => record.method === 'preview-close' && record.request.identity === original.identity)!.result;
    if (entry === 'delivery') expect(rejected.nativeError.data.reason).toBe('replaced'); else expect(rejected.kind).toBe('source_changed');
    await expect.poll(() => page.evaluate(() => (window as any).sourceContextRetirement[0]?.lost)).toBe(true);
    const view = entry === 'delivery' ? page.getByRole('complementary', { name: 'Previews', exact: true }) : workbench.locator('[data-workbench-file="cube.stl"]');
    // Source-change handling may start a new authorized generation itself.
    // Explicit Refresh remains an independent intent and must also reauthorize.
    const beforeRefresh = records.filter(record => record.method === 'preview-open').length;
    await view.getByRole('button', { name: 'Refresh', exact: true }).click();
    await expect.poll(() => records.filter(record => record.method === 'preview-open').length).toBeGreaterThan(beforeRefresh);
    await expect(page.locator('[data-geometry-format]:visible')).toContainText('2 × 2 × 2', { timeout: 20_000 });
    const reopened = records.filter(record => record.method === 'preview-open').at(-1)!.result.value;
    expect(reopened.identity).not.toBe(original.identity); expect(reopened.version).not.toBe(original.version);
    holdRead = true;
    await view.getByRole('button', { name: 'Refresh', exact: true }).click(); await read.ready; expect(heldReads).toBe(2);
    const canceled = records.filter(record => record.method === 'preview-open').at(-1)!.result.value;
    if (entry === 'delivery') await page.getByRole('button', { name: 'Close preview cube.stl', exact: true }).click();
    else {
      await workbench.getByRole('tab', { name: 'cube.stl', exact: false }).click({ button: 'right' });
      await page.locator('[data-dockkit-tab-menu]').getByRole('menuitem', { name: 'Close', exact: true }).click();
    }
    await expect.poll(() => records.some(record => record.method === 'preview-abort' && record.request.identity === canceled.identity)).toBe(true);
    await expect(page.locator('[data-geometry-format] canvas')).toHaveCount(0);
    read.release();
    await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
    await open(); await expect(page.locator('[data-geometry-format]:visible')).toContainText('2 × 2 × 2', { timeout: 20_000 });
    const fresh = records.filter(record => record.method === 'preview-open').at(-1)!.result.value;
    expect(fresh.identity).not.toBe(canceled.identity); expect(fresh.version).toBe(reopened.version);
    expect(records.find(record => record.method === 'preview-close' && record.request.identity === fresh.identity)?.result.ok).toBe(true);
    expect(records.some(record => record.result?.kind === 'file_settlement_unknown')).toBe(false);
    await info.attach('source-operation-retirement', { body: JSON.stringify(records, null, 2), contentType: 'application/json' }); passed = true;
  } finally { read.release(); close.release(); await Promise.allSettled([...routed]); await stopDogfoodPage(page, native, passed); }
});
