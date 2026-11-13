/** Paired production-function benchmark. Run from the repository root:
 * node --experimental-transform-types --expose-gc web-console/scripts/geometry-source-benchmark.ts --output docs/issue-534/source-reading.json
 * --smoke uses 8 MiB, one pair and 0/1 ms controlled delay; it is not final evidence.
 */
import fs from 'node:fs';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { AsyncLocalStorage } from 'node:async_hooks';
import { registerHooks, syncBuiltinESMExports } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import type { FileBytes, FileReadOptions } from '../shared/file-preview.ts';

const baselineRevision = 'c076354d647c1a1832e9af5e005696ecb5579285';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const geometryPath = 'web-console/src/client/geometry-source.ts';
const workspacePath = 'web-console/host/workspace-preview.ts';
const smoke = process.argv.includes('--smoke');
const outputIndex = process.argv.indexOf('--output');
if (outputIndex < 0 || !process.argv[outputIndex + 1]) throw new Error('--output <JSON path> is required');
if (typeof global.gc !== 'function') throw new Error('Run Node with --expose-gc');
const outputPath = path.resolve(process.argv[outputIndex + 1]);
const execFileAsync = promisify(execFile);
const git = async (...args: string[]) => (await execFileAsync('git', args, { cwd: root, encoding: 'utf8' })).stdout.trim();
const sha256 = (data: string | Uint8Array) => createHash('sha256').update(data).digest('hex');
// Freeze baseline bytes before loading either implementation or timing samples.
const baselineSources = new Map(await Promise.all([geometryPath, workspacePath].map(async name => [name, (await execFileAsync('git', ['show', `${baselineRevision}:${name}`], { cwd: root, encoding: 'utf8' })).stdout] as const)));
const currentHashes = new Map([geometryPath, workspacePath].map(name => [name, sha256(fs.readFileSync(path.join(root, name)))]));
const loadedSources = new Map<string, string>();
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.startsWith('.') && !/\.[a-z]+$/i.test(specifier)) {
      const candidate = new URL(`${specifier}.ts`, context.parentURL);
      if (fs.existsSync(candidate)) return nextResolve(candidate.href, context);
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    const parsed = new URL(url);
    if (parsed.protocol === 'file:') {
      const relative = path.relative(root, fileURLToPath(parsed));
      if (parsed.search === '?benchmark-baseline' && baselineSources.has(relative)) {
        return { format: 'module-typescript', source: baselineSources.get(relative)!, shortCircuit: true };
      }
      if (!relative.startsWith('..') && !relative.includes('node_modules') && fs.existsSync(parsed) && fs.statSync(parsed).isFile()) {
        loadedSources.set(relative, sha256(fs.readFileSync(parsed)));
      }
    }
    return nextLoad(url, context);
  },
});

type NativeRead = { startMs: number; endMs: number; offset: number | null; requestedBytes: number; readBytes: number };
type Request = { id: number; source: string; offset: number; length: number; submittedMs: number; admittedMs: number; endedMs: number; bytes: number; nativeReads: NativeRead[] };
type Context = { origin: number; request: Request };
const context = new AsyncLocalStorage<Context>();
const originalRead = fs.read;
// The production workspace implementation calls the real fs.read. Preserve its
// behavior, measuring callback settlement and retaining the page association.
fs.read = ((fd: number, buffer: NodeJS.ArrayBufferView, offset: number, length: number, position: number | null, callback: (error: NodeJS.ErrnoException | null, bytesRead: number, buffer: NodeJS.ArrayBufferView) => void) => {
  const scope = context.getStore();
  const start = performance.now();
  return originalRead(fd, buffer, offset, length, position, (error, count, result) => {
    if (scope) scope.request.nativeReads.push({ startMs: start - scope.origin, endMs: performance.now() - scope.origin, offset: position, requestedBytes: length, readBytes: count });
    callback(error, count, result);
  });
}) as typeof fs.read;
syncBuiltinESMExports();

const moduleUrl = (name: string, baseline: boolean) => `${pathToFileURL(path.join(root, name)).href}${baseline ? '?benchmark-baseline' : ''}`;
const baselineGeometry = await import(moduleUrl(geometryPath, true));
const currentGeometry = await import(moduleUrl(geometryPath, false));
const baselineWorkspace = await import(moduleUrl(workspacePath, true));
const currentWorkspace = await import(moduleUrl(workspacePath, false));
const { previewDemands, SESSION_FILE_MAX_TRANSFERS } = await import('../src/client/session-files.ts');
const { FILE_PREVIEW_LIMITS: limits } = await import('../shared/file-preview.ts');
const pageBytes = limits.pageBytes;
const sizeBytes = smoke ? 8 * 1024 * 1024 : 200 * 1024 * 1024;
const sharedBytes = smoke ? 4 * 1024 * 1024 : 16 * 1024 * 1024;
const pairs = smoke ? 1 : 3;
const source = new Uint8Array(sizeBytes);
for (let i = 0; i < source.length; i++) source[i] = i % 251;
const expectedHash = sha256(source), sharedHash = sha256(source.subarray(0, sharedBytes));
const temporaryDirectory = fs.mkdtempSync(path.join(os.tmpdir(), 'rustx534-benchmark-'));
const temporaryFile = path.join(temporaryDirectory, 'source.bin');
fs.writeFileSync(temporaryFile, source);
const stat = fs.statSync(temporaryFile, { bigint: true });
const workspaceVersion = `${stat.dev}:${stat.ino}:${stat.size}:${stat.mtimeNs}:${stat.ctimeNs}`;
const controlledVersion = 'benchmark-fixed-v1';
const pause = (milliseconds: number) => new Promise<void>(resolve => setTimeout(resolve, milliseconds));
const unionMs = (intervals: [number, number][]) => {
  let sum = 0, end = 0;
  for (const [start, finish] of intervals.sort((a, b) => a[0] - b[0])) { sum += Math.max(0, finish - Math.max(end, start)); end = Math.max(end, finish); }
  return sum;
};
const maxConcurrency = (intervals: [number, number][]) => {
  let active = 0, maximum = 0;
  const edges = intervals.flatMap(([start, end]) => [[start, 1], [end, -1]]);
  for (const [, delta] of edges.sort((a, b) => a[0] - b[0] || a[1] - b[1])) { active += delta; maximum = Math.max(maximum, active); }
  return maximum;
};
const memory = () => { const { rss, heapUsed, external, arrayBuffers } = process.memoryUsage(); return { rss, heapUsed, external, arrayBuffers }; };
type Variant = 'baseline' | 'current';
type Mode = 'controlled-single' | 'controlled-shared' | 'workspace-hot-local';

async function sample(variant: Variant, mode: Mode, delayMs: number, pair: number, order: number) {
  global.gc!();
  const readGeometrySource = (variant === 'baseline' ? baselineGeometry : currentGeometry).readGeometrySource as typeof import('../src/client/geometry-source.ts').readGeometrySource;
  const readWorkspacePreview = (variant === 'baseline' ? baselineWorkspace : currentWorkspace).readWorkspacePreview as typeof import('../host/workspace-preview.ts').readWorkspacePreview;
  const signal = new AbortController().signal;
  const demand = mode === 'controlled-shared' ? previewDemands({} as Parameters<typeof previewDemands>[0], undefined).privateReads : undefined;
  const requests: Request[] = [];
  const completed: { source: string; endedMs: number; version: string; data: Uint8Array }[] = [];
  let active = 0, maxActive = 0;
  const memorySamples: { atMs: number; rss: number; heapUsed: number; external: number; arrayBuffers: number }[] = [];
  const origin = performance.now();
  const sampleMemory = () => memorySamples.push({ atMs: performance.now() - origin, ...memory() });
  sampleMemory();
  const sampler = setInterval(sampleMemory, 5);
  const readFor = (name: string, bytes: number) => async (options: FileReadOptions, reading: AbortSignal): Promise<FileBytes> => {
    if (options.kind !== 'bytes' || !options.range) throw new Error('Expected geometry range read');
    const { offset, length } = options.range;
    const request: Request = { id: requests.length, source: name, offset, length, submittedMs: performance.now() - origin, admittedMs: 0, endedMs: 0, bytes: 0, nativeReads: [] };
    requests.push(request);
    const physical = () => context.run({ origin, request }, async () => {
      request.admittedMs = performance.now() - origin;
      active++; maxActive = Math.max(maxActive, active);
      try {
        let result: FileBytes;
        if (mode === 'workspace-hot-local') result = await readWorkspacePreview(temporaryDirectory, 'source.bin', options, reading);
        else {
          if (delayMs) await pause(delayMs);
          reading.throwIfAborted();
          const data = source.slice(offset, Math.min(offset + length, bytes));
          result = { data, version: controlledVersion, bytes, offset, eof: offset + data.length === bytes };
        }
        request.bytes = result.data.length;
        return result;
      } finally { request.endedMs = performance.now() - origin; active--; }
    });
    return demand ? demand.run(reading, physical) : physical();
  };
  const geometry = async (name: string, bytes: number) => {
    const result = await readGeometrySource(readFor(name, bytes), signal);
    completed.push({ source: name, endedMs: performance.now() - origin, ...result });
  };
  let elapsedMs: number;
  try {
    if (mode === 'controlled-shared') {
      const first = geometry('geometry-a', sharedBytes), second = geometry('geometry-b', sharedBytes);
      // Original-byte download demand joins the same real PreviewDemand after
      // the two initial geometry pages; it must get the next available slot.
      const download = readFor('download', pageBytes)({ kind: 'bytes', range: { offset: 0, length: pageBytes } }, signal).then(result => {
        completed.push({ source: 'download', endedMs: performance.now() - origin, data: result.data, version: result.version });
      });
      await Promise.all([first, second, download]);
    } else await geometry('geometry', sizeBytes);
    elapsedMs = performance.now() - origin;
  } finally { clearInterval(sampler); sampleMemory(); }
  // Hashing, result checks and summary calculations are outside the interval.
  const results = completed.map(result => {
    const expected = result.source === 'download' ? sha256(source.subarray(0, pageBytes)) : mode === 'controlled-shared' ? sharedHash : expectedHash;
    const digest = sha256(result.data);
    const expectedVersion = mode === 'workspace-hot-local' ? workspaceVersion : controlledVersion;
    if (digest !== expected || result.version !== expectedVersion) throw new Error(`Incorrect result: ${variant}/${mode}/${result.source}`);
    return { source: result.source, endedMs: result.endedMs, bytes: result.data.length, sha256: digest, expectedSha256: expected, hashMatches: true, version: result.version, expectedVersion, versionMatches: true };
  });
  if (maxActive > SESSION_FILE_MAX_TRANSFERS) throw new Error(`Physical concurrency exceeded ${SESSION_FILE_MAX_TRANSFERS}`);
  const admittedOrder = [...requests].sort((a, b) => a.admittedMs - b.admittedMs);
  const downloadAdmissionIndex = admittedOrder.findIndex(request => request.source === 'download');
  if (mode === 'controlled-shared' && downloadAdmissionIndex !== 2) throw new Error('Queued download did not receive the next available slot');
  const requestUnion = unionMs(requests.map(request => [request.admittedMs, request.endedMs]));
  const nativeReads = requests.flatMap(request => request.nativeReads);
  const nativeUnion = unionMs(nativeReads.map(read => [read.startMs, read.endMs]));
  return {
    variant, mode, delayMs, pair, order, sizeBytes: mode === 'controlled-shared' ? sharedBytes : sizeBytes, pageBytes, elapsedMs,
    stages: { firstPageMs: requests[0]?.endedMs, physicalPageActiveUnionMs: requestUnion, outsidePhysicalPageUnionMs: elapsedMs! - requestUnion,
      nativeReadCallbackActiveUnionMs: mode === 'workspace-hot-local' ? nativeUnion : null,
      outsideNativeReadCallbackUnionMs: mode === 'workspace-hot-local' ? elapsedMs! - nativeUnion : null },
    pageRequests: requests.length, physicalReadCalls: mode === 'workspace-hot-local' ? nativeReads.length : requests.length,
    maxActivePhysicalPages: maxActive, sharedAdmissionCapacity: demand ? SESSION_FILE_MAX_TRANSFERS : null,
    maxConcurrentPhysicalReadCalls: mode === 'workspace-hot-local' ? maxConcurrency(nativeReads.map(read => [read.startMs, read.endMs])) : maxActive,
    downloadAdmissionIndex: demand ? downloadAdmissionIndex : null, admittedSourceOrder: demand ? admittedOrder.map(request => request.source) : null,
    results, requests,
    memory: { samplingIntervalMs: 5, sampledMax: Object.fromEntries(['rss', 'heapUsed', 'external', 'arrayBuffers'].map(key => [key, Math.max(...memorySamples.map(value => value[key as keyof typeof value]))])), samples: memorySamples },
  };
}

const samples: Awaited<ReturnType<typeof sample>>[] = [];
try {
  // Warm production geometry and workspace paths outside recorded intervals.
  // Eight MiB exercises multiple pages without another large disk fixture.
  const warmBytes = Math.min(sizeBytes, 8 * 1024 * 1024);
  for (const baseline of [true, false]) {
    const readGeometrySource = (baseline ? baselineGeometry : currentGeometry).readGeometrySource as typeof import('../src/client/geometry-source.ts').readGeometrySource;
    const readWorkspacePreview = (baseline ? baselineWorkspace : currentWorkspace).readWorkspacePreview as typeof import('../host/workspace-preview.ts').readWorkspacePreview;
    await readGeometrySource(async options => {
      if (options.kind !== 'bytes' || !options.range) throw new Error('Expected warmup range');
      const { offset, length } = options.range;
      const data = source.slice(offset, Math.min(offset + length, warmBytes));
      return { data, bytes: warmBytes, version: controlledVersion, offset, eof: offset + data.length === warmBytes };
    }, new AbortController().signal);
    await readWorkspacePreview(temporaryDirectory, 'source.bin', { kind: 'bytes', range: { offset: 0, length: pageBytes } }, new AbortController().signal);
  }
  const groups: { mode: Mode; delays: number[] }[] = [
    { mode: 'controlled-single', delays: smoke ? [0, 1] : [0, 10, 50] },
    { mode: 'controlled-shared', delays: [smoke ? 1 : 10] },
    { mode: 'workspace-hot-local', delays: [0] },
  ];
  for (const group of groups) for (const delay of group.delays) for (let pair = 1; pair <= pairs; pair++) {
    const variants: Variant[] = pair % 2 ? ['baseline', 'current'] : ['current', 'baseline'];
    for (const [order, variant] of variants.entries()) {
      const result = await sample(variant, group.mode, delay, pair, order);
      samples.push(result);
      process.stderr.write(`${group.mode} delay=${delay} pair=${pair} ${variant}: ${result.elapsedMs.toFixed(2)}ms, ${result.physicalReadCalls} physical reads\n`);
    }
  }
  for (const [name, hash] of currentHashes) if (sha256(fs.readFileSync(path.join(root, name))) !== hash) throw new Error(`Production source changed during benchmark: ${name}`);
  const report = {
    schema: 'issue-534-source-reading-v1', createdAt: new Date().toISOString(), smoke,
    command: [process.execPath, ...process.execArgv, ...process.argv.slice(1)],
    source: { baselineRevision, currentRevision: await git('rev-parse', 'HEAD'), workingTreeStatus: await git('status', '--short'),
      productionFiles: [geometryPath, workspacePath].map(name => ({ path: name, baselineSha256: sha256(baselineSources.get(name)!), currentSha256: currentHashes.get(name) })),
      loadedRepositoryFileSha256: Object.fromEntries([...loadedSources].sort(([a], [b]) => a.localeCompare(b))) },
    environment: { node: process.version, versions: process.versions, platform: process.platform, architecture: process.arch, os: { type: os.type(), release: os.release(), version: os.version() }, cpu: os.cpus()[0], logicalCpuCount: os.cpus().length, totalMemoryBytes: os.totalmem(), uvThreadpoolSize: process.env.UV_THREADPOOL_SIZE ?? 'Node default' },
    methodology: {
      pairedSamplesPerGroup: pairs, order: 'odd pair baseline/current; even pair current/baseline',
      controlledDelaysMs: smoke ? [0, 1] : [0, 10, 50], fixtureBytes: sizeBytes, expectedSha256: expectedHash,
      shared: { geometrySources: 2, bytesPerGeometrySource: sharedBytes, downloadBytes: pageBytes, delayMs: smoke ? 1 : 10, capacity: SESSION_FILE_MAX_TRANSFERS, admission: 'actual previewDemands().privateReads; download queued after initial two geometry pages; same in-memory fixture' },
      timerExcludes: ['module loading', 'fixture allocation/write', 'explicit GC', 'result hashing/validation', 'report serialization'],
      warmup: 'each production geometry reader consumes an 8 MiB memory prefix; each workspace reader consumes one 2 MiB local page, all before recorded samples',
      timerIncludes: ['5 ms memory sampler overhead', 'production validation/yields/allocation/assembly', 'page adapter and read instrumentation'],
      local: 'real readGeometrySource -> readWorkspacePreview -> descriptor-relative workspace open and fs.read; recently written local file, no cold-cache claim',
      controlled: 'in-memory page copies with artificial per-read timer delay, not measured network latency',
      stages: 'page and fs.read callback spans are overlapping interval unions, not additive waterfall segments; outside spans include scheduling, validation, copying, yielding, admission and instrumentation; they do not isolate assembly CPU',
      memory: '5 ms sampled process RSS/heap/external/arrayBuffers with start/end samples; sampled maxima are not exact peaks',
      scopeExcludes: ['HTTP', 'native delivery transport', 'browser', 'geometry parse', 'GPU', 'first-screen 200 ms validation', 'interaction 50 ms validation'],
      temporaryDisk: 'one fixture only, at most 200 MiB; removed in finally; no tested production file writes',
    },
    samples,
  };
  fs.mkdirSync(path.dirname(outputPath), { recursive: true });
  fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`);
  process.stderr.write(`Wrote ${outputPath}\n`);
} finally {
  fs.rmSync(temporaryDirectory, { recursive: true, force: true });
  fs.read = originalRead;
  syncBuiltinESMExports();
}
