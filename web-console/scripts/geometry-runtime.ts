/** Verify the reviewed CAD binary/glue and their actual memory maximum. */
import { readFileSync, writeFileSync, readdirSync, mkdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { gzipSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { GEOMETRY_LIMITS } from '../src/app/components/documents/geometry/limits.ts';
const root = new URL('../', import.meta.url), cad = new URL('./', import.meta.resolve('occt-import-js'));
const importer = JSON.parse(readFileSync(new URL('../package.json', cad), 'utf8'));
const renderer = JSON.parse(readFileSync(new URL('../package.json', import.meta.resolve('three')), 'utf8'));
if (importer.version !== '0.0.23-rustx.3' || renderer.version !== '0.187.0') throw new Error('Unreviewed geometry dependency version');
const modifiedSources = ['src/importer-brep.cpp', 'src/importer-brep.hpp', 'src/importer-utils.cpp', 'src/importer.hpp', 'src/js-interface.cpp'];
const actualSources = readdirSync(new URL('../src/', cad)).map(name => `src/${name}`).sort();
if (JSON.stringify(actualSources) !== JSON.stringify(modifiedSources)) throw new Error('Unreviewed CAD modified source set');
const modifiedSourcesSha256 = Object.fromEntries(modifiedSources.map(file => [file,
  createHash('sha256').update(readFileSync(new URL(`../${file}`, cad))).digest('hex')]));
const assets = Object.fromEntries(['occt-import-js.js', 'occt-import-js.wasm', 'license.occt-import-js.txt', 'license.occt.txt'].map(name => {
  const bytes = readFileSync(new URL(name, cad));
  return [name, { bytes: bytes.length, gzipBytes: gzipSync(bytes).length, sha256: createHash('sha256').update(bytes).digest('hex') }];
}));
const glue = readFileSync(new URL('occt-import-js.js', cad), 'utf8');
if (!glue.includes(`getHeapMax=()=>${GEOMETRY_LIMITS.wasmBytes}`)) throw new Error('Unreviewed CAD heap growth policy');
const binary = readFileSync(new URL('occt-import-js.wasm', cad));
let at = 8, maximum: number | undefined;
function leb() { let value = 0, bits = 0, byte: number; do { if (bits > 35 || at >= binary.length) throw new Error('Invalid WASM limits'); byte = binary[at++]; value += (byte & 127) * 2 ** bits; bits += 7; } while (byte & 128); return value; }
while (at < binary.length) {
  const id = binary[at++], length = leb(), end = at + length;
  if (id === 5) { if (leb() !== 1 || leb() !== 1) throw new Error('Unreviewed WASM memory declaration'); leb(); maximum = leb() * 65536; }
  at = end;
}
if (maximum !== GEOMETRY_LIMITS.wasmBytes) throw new Error(`WASM memory maximum changed: ${maximum}`);
const record = { importer: `${importer.name}@${importer.version}`, renderer: `${renderer.name}@${renderer.version}`,
  importerCommit: 'c2148e54b456b571238d35cac037d304053d64b2', occtCommit: 'd2abb6d844231cb8f29be6894440874a4700e4a5',
  modifiedSourcesSha256,
  wasmMaximumBytes: maximum, assets };
const path = new URL('geometry-runtime.json', root), content = `${JSON.stringify(record, null, 2)}\n`;
if (process.argv.includes('--write')) writeFileSync(path, content);
else if (readFileSync(path, 'utf8') !== content) throw new Error('Geometry runtime drift: inspect release artifacts before regenerating geometry-runtime.json');
if (process.argv.includes('--artifact')) {
  const directory = new URL('dist/assets/', root);
  const shipped = readdirSync(directory).filter(name => name.endsWith('.wasm')).map(name => readFileSync(new URL(name, directory)));
  if (!shipped.some(bytes => createHash('sha256').update(bytes).digest('hex') === assets['occt-import-js.wasm'].sha256)) throw new Error('Reviewed CAD WASM missing from production assets');
  // Distribute the complete modified source and rebuild instructions alongside
  // the binary, without requiring access to a particular repository branch.
  for (const file of ['README.md', 'LICENSE.md', 'UPSTREAM.json', 'rebuild.sh', ...modifiedSources, 'dist/license.occt-import-js.txt', 'dist/license.occt.txt']) {
    const source = readFileSync(new URL(`../${file}`, cad)), target = new URL(`dist/cad-runtime-source/${file}`, root);
    mkdirSync(new URL('./', target), { recursive: true }); writeFileSync(target, source);
  }
}
console.log(`Verified geometry runtime assets and ${maximum / 1024 / 1024} MiB WASM maximum (${fileURLToPath(path)})`);
