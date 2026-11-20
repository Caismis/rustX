/** Native-boundary benchmark. Each case runs in a fresh process for OS peak RSS.
 * node scripts/benchmark-cad-native.mjs --output /tmp/samples.json --runs 3 \
 *   --runtime /tmp/build-O2/build/Release --runtime /tmp/build-Oz/build/Release \
 *   --fixture test/fixtures/geometry/lod-assembly.step --fixture /tmp/large.step
 * Cold = compile + fresh initialization + import. Cached module = fresh
 * instance initialization + import, excluding compilation; never a reused heap.
 * Phase clocks are opt-in inclusive native aggregates.
 */
import { readFileSync, writeFileSync, openSync, fstatSync, readSync, closeSync, existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import { createHash } from 'node:crypto';
import { resolve, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { gzipSync } from 'node:zlib';
import { cpus, totalmem } from 'node:os';

const args = process.argv.slice(2);
const values = flag => args.flatMap((value, index) => value === flag ? [args[index + 1]] : []);
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
function buildEvidence(runtime) {
  const verified = resolve(runtime, '../../verified-build.json');
  if (existsSync(verified)) return { effectiveBuild: JSON.parse(readFileSync(verified, 'utf8')) };
  const historical = join(runtime, 'geometry-runtime.json');
  return existsSync(historical) ? { historicalRuntime: JSON.parse(readFileSync(historical, 'utf8')) } : {};
}
function canonical(result) {
  return JSON.stringify(result, (_key, value) => ArrayBuffer.isView(value) ? Array.from(value) : value);
}
// Executed only after clocks and the pre-digest RSS peak snapshot. Normalize
// old boxed numbers to Float64 without JSON's loss of negative-zero bits.
function numericBitsDigest(result) {
  const digest = createHash('sha256');
  for (let meshIndex = 0; meshIndex < result.meshes.length; ++meshIndex) {
    const mesh = result.meshes[meshIndex];
    for (const [label, source] of [['position', mesh.attributes.position.array], ['normal', mesh.attributes.normal?.array], ['index', mesh.index.array]]) {
      if (!source) { digest.update(`${meshIndex}:${label}:absent;`); continue; }
      const doubles = source instanceof Float64Array ? source : Float64Array.from(source);
      digest.update(`${meshIndex}:${label}:${doubles.length};`);
      digest.update(new Uint8Array(doubles.buffer, doubles.byteOffset, doubles.byteLength));
    }
  }
  return digest.digest('hex');
}
function metadataDigest(result) {
  return hash(JSON.stringify({ success: result.success, root: result.root, meshes: result.meshes.map(mesh => ({
    name: mesh.name, color: mesh.color, brep_faces: mesh.brep_faces,
  })) }));
}
if (args.includes('--child')) {
  const [runtime] = values('--runtime'), [fixture] = values('--fixture');
  const tiers = Number(values('--tiers')[0]), profilingEnabled = args.includes('--profile');
  // Exact owned buffer, no full-size intermediate Buffer or pool backing.
  const descriptor = openSync(fixture, 'r');
  let bytes;
  try {
    bytes = new Uint8Array(fstatSync(descriptor).size);
    let offset = 0;
    while (offset < bytes.length) {
      const count = readSync(descriptor, bytes, offset, bytes.length - offset, offset);
      if (!count) throw new Error('Incomplete fixture read');
      offset += count;
    }
  } finally { closeSync(descriptor); }
  const sourceBytes = bytes.byteLength, sourceSha256 = hash(bytes);
  global.gc();
  const initializeStart = performance.now();
  const binary = readFileSync(join(runtime, 'occt-import-js.wasm'));
  const module = await WebAssembly.compile(binary);
  const compileMs = performance.now() - initializeStart;
  const create = createRequire(import.meta.url)(join(runtime, 'occt-import-js.js'));
  const instanceStart = performance.now();
  const occt = await create({ instantiateWasm(imports, receive) {
    const instance = new WebAssembly.Instance(module, imports); receive(instance, module); return instance.exports;
  }, print() {}, printErr() {} });
  const freshInstanceInitializeMs = performance.now() - instanceStart;
  const initializeMs = performance.now() - initializeStart;
  const reads = { step: 'ReadStepFile', stp: 'ReadStepFile', iges: 'ReadIgesFile', igs: 'ReadIgesFile', brep: 'ReadBrepFile' };
  const method = reads[fixture.split('.').at(-1).toLowerCase()];
  if (!method) throw new Error('Unsupported CAD format');
  let spans = [], final, coarseMs, first;
  const start = performance.now(), before = process.memoryUsage(), cpuBefore = process.cpuUsage();
  final = occt[method](bytes, { linearUnit: 'millimeter', linearDeflectionType: 'bounding_box_ratio', linearDeflection: .001, angularDeflection: .5,
    lodTiers: tiers, onInputCopied() { bytes.buffer.transfer(0); },
    ...(profilingEnabled ? { onNativeProfile(value) { spans = value; } } : {}), ...(tiers ? { onLod(level, result) {
      // Only fixed metadata survives each callback, never the mesh buffers.
      if (level === 0) coarseMs = performance.now() - start;
      const item = { level, triangles: result.meshes.reduce((sum, mesh) => sum + mesh.index.array.length / 3, 0), meshes: result.meshes.length, hierarchyDigest: hash(JSON.stringify({ root: result.root, names: result.meshes.map(mesh => mesh.name) })) };
      first ??= []; first.push(item);
    } } : {}) });
  const freshInstanceReadMs = performance.now() - start, after = process.memoryUsage(), importCpu = process.cpuUsage(cpuBefore);
  if (final.success !== true) throw new Error('Native import failed');
  if (bytes.byteLength !== 0) throw new Error('Owned source was not released after native copy');
  const meshStreams = final.meshes.flatMap(mesh => [mesh.attributes.position.array, mesh.attributes.normal?.array, mesh.index.array].filter(Boolean));
  const result = { runtime, fixture, tiers, sourceBytes, sourceSha256, compileMs, initializeMs, freshInstanceInitializeMs, freshInstanceReadMs,
    // These are scoped totals from the same fresh-instance observation, not
    // separate measurements or a warmed native document/heap.
    coldCodeToFinalMs: initializeMs + freshInstanceReadMs,
    cachedModuleToFinalMs: freshInstanceInitializeMs + freshInstanceReadMs,
    coarseMs, spans, profilingEnabled, importCpuUserMs: importCpu.user / 1000, importCpuSystemMs: importCpu.system / 1000, ownedSourceReleased: bytes.byteLength === 0,
    peakRssBytes: process.resourceUsage().maxRSS * 1024, peakRssScope: 'OS process high-water: fixture load, Node/V8, compile/init, one fresh WASM instance and JS-owned raw output; before final digest serialization',
    processBefore: before, processAfter: after, meshes: final.meshes.length, triangles: final.meshes.reduce((sum, mesh) => sum + mesh.index.array.length / 3, 0),
    numericPayloadBytes: meshStreams.reduce((sum, stream) => sum + stream.length * 8, 0),
    rawTypedBufferBytes: meshStreams.every(stream => stream instanceof Float64Array) ? meshStreams.reduce((sum, stream) => sum + stream.byteLength, 0) : null,
    typedRawOutput: meshStreams.every(stream => stream instanceof Float64Array && stream.buffer instanceof ArrayBuffer),
    optional: first ?? [], finalDigest: hash(canonical(final)), finalNumericBitsDigest: numericBitsDigest(final), finalMetadataDigest: metadataDigest(final) };
  writeFileSync(1, JSON.stringify(result));

} else {
  const boxedRuntimes = new Set(values('--boxed-runtime').map(value => resolve(value)));
  const runtimes = [...values('--runtime').map(value => resolve(value)), ...boxedRuntimes];
  const fixtures = values('--fixture').map(value => resolve(value));
  if (!runtimes.length || !fixtures.length || !values('--output').length) throw new Error('Provide --runtime, --fixture and --output');
  const runs = Number(values('--runs')[0] ?? 3), samples = [];
  const policies = values('--tiers').length ? values('--tiers').map(Number) : [0,1,2];
  if (!Number.isInteger(runs) || runs < 1 || policies.some(value => ![0,1,2].includes(value))) throw new Error('Invalid --runs/--tiers');
  const runtimeAssets = runtimes.map(path => ({ path, ...buildEvidence(path), ...Object.fromEntries(['js','wasm'].map(extension => {
    const bytes = readFileSync(join(path, `occt-import-js.${extension}`));
    return [extension, { bytes: bytes.length, gzipBytes: gzipSync(bytes).length, sha256: hash(bytes) }];
  })) }));
  const fingerprints = runtimeAssets.flatMap(runtime => runtime.effectiveBuild ? [JSON.stringify(runtime.effectiveBuild.modifiedSourcesSha256)] : []);
  if (new Set(fingerprints).size > 1) throw new Error('Build variants used different modified source snapshots');
  const environment = { timestampUTC: new Date().toISOString(), platform: process.platform, arch: process.arch, node: process.version, v8: process.versions.v8,
    cpuModel: cpus()[0]?.model, logicalCpus: cpus().length, totalMemoryBytes: totalmem() };
  // Paired order rotates each repetition, rather than measuring one build only
  // after another build's caches have warmed. Do not run compilers concurrently.
  for (let run = 0; run < runs; run++) for (const fixture of fixtures) for (const tiers of policies) {
    const paired = run % 2 ? [...runtimes].reverse() : runtimes;
    for (const runtime of paired) {
      if (tiers === 1 && boxedRuntimes.has(runtime)) continue; // old API supports only final/all
      const child = spawnSync(process.execPath, ['--expose-gc', import.meta.filename, '--child', '--runtime', runtime, '--fixture', fixture, '--tiers', String(tiers), ...(args.includes('--profile') ? ['--profile'] : [])], { encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 });
      if (child.status !== 0 || !child.stdout.trim()) throw new Error(child.stderr || child.stdout || `Child exit ${child.status} without benchmark result`);
      samples.push({ run, ...JSON.parse(child.stdout) });
      process.stderr.write(`run ${run + 1}: ${runtime} ${fixture} tiers=${tiers}\n`);
      writeFileSync(values('--output')[0], JSON.stringify({ scope: 'Native CPU/OS measurements, not browser display or integrated-GPU certification', environment, runtimeAssets, samples }, null, 2) + '\n');
    }
  }
  // Complete final hierarchy, color, face ranges and every double/index must
  // agree both across tier policies and across build variants.
  for (const fixture of fixtures) {
    const results = samples.filter(sample => sample.fixture === fixture);
    for (const field of ['finalDigest', 'finalNumericBitsDigest', 'finalMetadataDigest']) {
      if (new Set(results.map(sample => sample[field])).size !== 1) throw new Error(`Final ${field} mismatch for ${fixture}`);
    }
  }
}
