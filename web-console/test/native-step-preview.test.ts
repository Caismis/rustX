// @vitest-environment node
import { once } from 'node:events';
import { createServer } from 'node:http';
import { mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { Mesh, MeshStandardMaterial, Vector3, type Object3D } from 'three';
import { workspaceHandler } from '../host/http';
import { NativeStepPreview } from '../host/step-preview';
import { LocalWorkspaceHost } from '../host/workspaces';
import { readGeometrySource } from '../src/client/geometry-source';
import { importGlb } from '../src/app/components/documents/geometry/glb';
import { disposeScene, modelScene } from '../src/app/components/documents/geometry/scene-resources';
import { HttpWorkspaceHost } from '../src/workspaces/http-host';
import type { WorkbenchCall } from '../src/workspaces/workbench';
import { stepRootCases } from './geometry-step-fixtures';

const cleanup = vi.hoisted(() => ({ barrier: undefined as undefined | ((path: string) => Promise<void>) }));
vi.mock('node:fs/promises', async importOriginal => {
  const fs = await importOriginal<typeof import('node:fs/promises')>();
  return {
    ...fs,
    rm: async (...args: Parameters<typeof fs.rm>) => {
      const [path] = args;
      if (typeof path === 'string' && path.split('/').at(-1)?.startsWith('rustx-step-preview-')) await cleanup.barrier?.(path);
      return fs.rm(...args);
    },
  };
});

function latch() {
  let release!: () => void;
  const reached = new Promise<void>(resolve => { release = resolve; });
  return { reached, release };
}

const signal = () => new AbortController().signal;
const glb = await readFile(new URL('./fixtures/geometry/cube.glb', import.meta.url));
const nativeExecutable = process.env.RUSTX_NATIVE_STEP_EXECUTABLE;
if (process.env.RUSTX_REQUIRE_NATIVE_STEP === '1' && (process.platform !== 'linux' || !nativeExecutable))
  throw new Error('The required real OCCT STEP helper is unavailable');

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), 'rustx-native-step-test-'));
  const executable = join(root, 'fake-converter'), ready = join(root, 'ready');
  await writeFile(executable, `#!${process.execPath}
const fs = require('node:fs');
const input = fs.readFileSync(3, 'utf8');
if (input === 'external') process.exit(3);
if (input === 'malformed') process.exit(2);
const publish = () => {
  fs.writeFileSync('preview.glb', Buffer.from(${JSON.stringify(glb.toString('base64'))}, 'base64'));
  fs.writeFileSync('timing.json', JSON.stringify({ occt: '7.6.1', threads: +process.argv[2], readMs: 1, parseMs: 2, transferMs: 3, meshMs: 4, encodeMs: 5, roots: 1, peakRssKiB: 50 }));
};
if (input.startsWith('wait')) {
  fs.writeFileSync(${JSON.stringify(ready)}, String(process.pid));
  setTimeout(publish, input === 'wait' ? 400 : 60_000);
} else publish();
`, { mode: 0o700 });
  const config = { executable, threads: 1 } as const;
  const source = join(root, 'model.stp');
  await writeFile(source, 'model');
  return { root, source, config, pid: () => vi.waitFor(async () => Number(await readFile(ready, 'utf8'))), close: () => rm(root, { recursive: true, force: true }) };
}

describe.runIf(process.platform === 'linux')('Workbench native STEP carrier', () => {
  it('delivers an admitted GLB for STEP/STP with the original file version', async () => {
    const f = await fixture();
    const host = new LocalWorkspaceHost({ stepPreviewConverter: f.config, endpoint: 'ws://127.0.0.1:8080', roots: [{ id: 'root', cwd: f.root, displayName: 'Root' }], metadataFile: join(f.root, 'registry.json'), picker: false });
    const server = createServer(workspaceHandler(host));
    server.listen(0, '127.0.0.1'); await once(server, 'listening');
    try {
      const address = server.address();
      if (!address || typeof address === 'string') throw new Error('Missing Host address');
      const origin = `http://127.0.0.1:${address.port}`;
      vi.stubGlobal('location', new URL(origin));
      const adapter = new HttpWorkspaceHost(`${origin}/product-host`);
      const scope = await adapter.listWorkspaces(), target = { workspace_id: scope.workspaces[0].id };
      const call: WorkbenchCall = { target, request: { kind: 'preview', path: 'model.stp', read: { kind: 'bytes', range: { offset: 0, length: 1 } } } };
      const source = await adapter.workbench(scope, call, signal());
      const result = await readGeometrySource(active => adapter.openWorkspacePreview(scope, call, active), signal());
      expect(result.version).toBe(source.preview?.version);
      expect(result.nativeStep).toMatchObject({ threads: 1, roots: 1, transferMs: 3 });
      expect(Buffer.from(result.data)).toEqual(glb);
      const loaded = await importGlb(result.data, signal()); expect(loaded.bounds.isEmpty()).toBe(false); loaded.dispose();
      await writeFile(f.source, 'malformed');
      await expect(adapter.openWorkspacePreview(scope, call, signal())).rejects.toThrow('parser_failure');
      await writeFile(f.source, 'external');
      await expect(adapter.openWorkspacePreview(scope, call, signal())).rejects.toThrow('native_step_external');
    } finally {
      vi.unstubAllGlobals();
      await host.close();
      await new Promise<void>(resolve => server.close(() => resolve()));
      await f.close();
    }
  });

  it('rejects mutation and retired authority during conversion and final settlement', async () => {
    const f = await fixture(), converter = new NativeStepPreview(f.config);
    try {
      await writeFile(f.source, 'wait');
      const opening = converter.open(f.root, 'model.stp', async () => {}, signal());
      const changed = expect(opening).rejects.toMatchObject({ kind: 'source_changed' });
      await f.pid(); await writeFile(f.source, 'replacement'); await changed;
      const retired = new Error('authorization_retired');
      let live = true;
      const reauthorize = async () => { if (!live) throw retired; };
      const operation = await converter.open(f.root, 'model.stp', reauthorize, signal());
      live = false;
      await expect(operation.read({ offset: 0, length: 20 })).rejects.toBe(retired);
      await expect(operation.close()).rejects.toBe(retired);
      live = true;
      const next = await converter.open(f.root, 'model.stp', reauthorize, signal());
      await writeFile(f.source, 'new revision');
      await expect(next.close()).rejects.toMatchObject({ kind: 'source_changed' });
    } finally { await f.close(); }
  });

  it.each(['source', 'authority'] as const)('rejects %s changes while the native preview asset retires', async change => {
    const f = await fixture(), converter = new NativeStepPreview(f.config);
    const entered = latch(), continueCleanup = latch();
    const revoked = new Error('authorization_retired');
    let authorized = true;
    try {
      const operation = await converter.open(f.root, 'model.stp', async () => { if (!authorized) throw revoked; }, signal());
      cleanup.barrier = async () => { entered.release(); await continueCleanup.reached; };
      const closing = operation.close();
      await entered.reached;
      if (change === 'source') {
        const replacement = join(f.root, 'replacement.stp');
        await writeFile(replacement, 'another STEP version');
        await rename(replacement, f.source);
      } else authorized = false;
      continueCleanup.release();
      if (change === 'source') await expect(closing).rejects.toMatchObject({ kind: 'source_changed' });
      else await expect(closing).rejects.toBe(revoked);
    } finally {
      continueCleanup.release();
      cleanup.barrier = undefined;
      await f.close();
    }
  });

  it('preserves settlement uncertainty if asset cleanup fails during cancellation', async () => {
    const f = await fixture(), abort = new AbortController();
    const entered = latch(), continueCleanup = latch();
    let assetDirectory: string | undefined;
    try {
      const operation = await new NativeStepPreview(f.config).open(f.root, 'model.stp', async () => {}, abort.signal);
      cleanup.barrier = async path => {
        assetDirectory = path;
        entered.release();
        await continueCleanup.reached;
        throw new Error('Asset cleanup failed');
      };
      const closing = operation.close();
      await entered.reached;
      abort.abort();
      continueCleanup.release();
      await expect(closing).rejects.toMatchObject({ kind: 'file_settlement_unknown' });
    } finally {
      continueCleanup.release();
      cleanup.barrier = undefined;
      if (assetDirectory) await rm(assetDirectory, { recursive: true, force: true });
      await f.close();
    }
  });

  it('settles a running converter when the owning Product Host retires', async () => {
    const f = await fixture();
    const host = new LocalWorkspaceHost({ stepPreviewConverter: f.config, endpoint: 'ws://127.0.0.1:8080', roots: [{ id: 'root', cwd: f.root, displayName: 'Root' }], metadataFile: join(f.root, 'registry.json'), picker: false });
    try {
      await writeFile(f.source, 'wait indefinitely');
      const scope = await host.listWorkspaces();
      const opening = host.openWorkspacePreview(scope, {
        target: { workspace_id: scope.workspaces[0].id },
        request: { kind: 'preview', path: 'model.stp', read: { kind: 'bytes' } },
      }, signal());
      const retired = expect(opening).rejects.toHaveProperty('name', 'AbortError');
      const pid = await f.pid();
      await host.close(); await retired;
      expect(() => process.kill(pid, 0)).toThrow(expect.objectContaining({ code: 'ESRCH' }));
    } finally { await host.close(); await f.close(); }
  });

  it.each(['abort', 'timeout'] as const)('waits for child settlement before releasing admission after %s', async kind => {
    const f = await fixture(), converter = new NativeStepPreview(f.config, kind === 'timeout' ? 600 : 10_000);
    const abort = new AbortController();
    try {
      await writeFile(f.source, 'wait indefinitely');
      const opening = converter.open(f.root, 'model.stp', async () => {}, abort.signal);
      const rejected = kind === 'abort' ? expect(opening).rejects.toHaveProperty('name', 'AbortError') : expect(opening).rejects.toThrow('parser_timeout');
      const pid = await f.pid();
      await expect(converter.open(f.root, 'model.stp', async () => {}, signal())).rejects.toMatchObject({ kind: 'capacity' });
      if (kind === 'abort') abort.abort();
      await rejected;
      expect(() => process.kill(pid, 0)).toThrow(expect.objectContaining({ code: 'ESRCH' }));
      await writeFile(f.source, 'model');
      await (await converter.open(f.root, 'model.stp', async () => {}, signal())).close();
    } finally { abort.abort(); await f.close(); }
  });
});

it.runIf(process.platform === 'linux' && !!nativeExecutable)('requires real OCCT transfer coverage for all declared STEP roots', async () => {
  const root = await mkdtemp(join(tmpdir(), 'rustx-step-roots-'));
  const cases = stepRootCases(await readFile(new URL('./fixtures/geometry/cube.step', import.meta.url), 'utf8'));
  const converter = new NativeStepPreview({ executable: nativeExecutable!, threads: 1 });
  async function write(name: string, next: string) {
    await writeFile(join(root, name), next);
    return name;
  }
  const source = (name: string) => readGeometrySource(active => converter.open(root, name, async () => {}, active), signal());
  try {
    const complete = await write('complete.step', cases.complete);
    expect((await source(complete)).nativeStep?.roots).toBe(2);
    const metadata = await write('metadata-only.step', cases.metadata);
    expect((await source(metadata)).nativeStep?.roots).toBe(1);
    const wire = await write('wire.step', cases.wire);
    expect((await source(wire)).nativeStep?.roots).toBe(1);
    const partial = await write('partial.step', cases.partial);
    await expect(source(partial)).rejects.toThrow('parser_failure');
    const emptyWire = await write('empty-wire.step', cases.emptyWire);
    await expect(source(emptyWire)).rejects.toThrow('parser_failure');
  } finally { await rm(root, { recursive: true, force: true }); }
});

it.runIf(process.platform === 'linux' && !!nativeExecutable)('preserves full-detail bounds and triangle counts against the pinned WASM STEP path', async () => {
  const { importCad } = await import('../src/app/components/documents/geometry/cad');
  const module = await WebAssembly.compile(await readFile(new URL('../node_modules/occt-import-js/dist/occt-import-js.wasm', import.meta.url)));
  const root = await mkdtemp(join(tmpdir(), 'rustx-step-parity-'));
  const compareScene = (scene: Object3D, meters: number) => {
    const occurrences: string[] = [], palette = new Set<string>();
    let triangles = 0;
    scene.updateMatrixWorld(true);
    scene.traverse(item => {
      if (!(item instanceof Mesh)) return;
      triangles += (item.geometry.index?.count ?? item.geometry.getAttribute('position').count) / 3;
      const bounds = item.geometry.boundingBox!.clone().applyMatrix4(item.matrixWorld);
      occurrences.push([...bounds.min.toArray(), ...bounds.max.toArray()].map(value => Math.round(value * meters * 1e6)).join(','));
      for (const material of Array.isArray(item.material) ? item.material : [item.material]) if (material instanceof MeshStandardMaterial)
        palette.add(material.color.toArray().map(value => Math.round(value * 1000)).join(','));
    });
    return { triangles, occurrences: occurrences.sort(), palette: [...palette].sort() };
  };
  try {
    for (const name of ['cube.step', 'cube-inch.step', 'pole-sphere.step', 'mounting-bracket.step', 'bracket-array.step']) {
      const source = await readFile(new URL(`./fixtures/geometry/${name}`, import.meta.url));
      await writeFile(join(root, name), source);
      const wasm = modelScene(await importCad(Uint8Array.from(source), 'step', module));
      try {
        const min = wasm.bounds.min.clone().multiplyScalar(.001), max = wasm.bounds.max.clone().multiplyScalar(.001);
        const tolerance = wasm.bounds.getSize(new Vector3()).length() * .001 * 1e-5;
        const expected = compareScene(wasm.scene, .001);
        for (const threads of [1, 8] as const) {
          const converter = new NativeStepPreview({ executable: nativeExecutable!, threads });
          const result = await readGeometrySource(active => converter.open(root, name, async () => {}, active), signal());
          const native = await importGlb(result.data, signal(), true);
          try {
            expect(native.bounds.min.distanceTo(min), `${name} min, ${threads} threads`).toBeLessThan(tolerance);
            expect(native.bounds.max.distanceTo(max), `${name} max, ${threads} threads`).toBeLessThan(tolerance);
            const actual = compareScene(native.scene, 1);
            expect(actual.triangles, `${name} triangles, ${threads} threads`).toBe(expected.triangles);
            expect(actual.occurrences, `${name} occurrence bounds, ${threads} threads`).toEqual(expected.occurrences);
            if (name === 'mounting-bracket.step' || name === 'bracket-array.step')
              expect(actual.palette, `${name} authored colors, ${threads} threads`).toEqual(expected.palette);
            if (name === 'bracket-array.step') {
              expect(actual.occurrences).toHaveLength(96);
              expect(new Set(actual.occurrences).size).toBe(96);
            }
          } finally { native.dispose(); }
        }
      } finally { disposeScene([wasm.scene]); }
    }
  } finally { await rm(root, { recursive: true, force: true }); }
}, 30_000);
