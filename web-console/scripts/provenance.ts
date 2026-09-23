import { readFileSync, existsSync, readdirSync } from 'node:fs';
import { resolve, relative, dirname } from 'node:path';
import { createHash } from 'node:crypto';
import ts from 'typescript';
import { execFileSync } from 'node:child_process';
const web = resolve(import.meta.dirname, '..');
const repository = resolve(web, '..');
const inventory = JSON.parse(readFileSync(resolve(web, 'source-inventory.json'), 'utf8'));
function assert(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
assert(inventory.commit === 'ddefc45fbc7f8e46dd73185e68295696d1297887', 'Unreviewed Harness revision');
assert(inventory.repository === 'https://github.com/deepseek-ai/deepseek-harness', 'Wrong upstream');
// Optional maintainer audit against the external checkout. Ordinary build/CI
// remains offline and never downloads or executes upstream source.
const referenceIndex = process.argv.indexOf('--reference');
const reference = referenceIndex < 0 ? undefined : process.argv[referenceIndex + 1];
if (referenceIndex >= 0) {
  assert(reference && !reference.startsWith('--'), '--reference requires a checkout path');
  assert(execFileSync('git', ['-C', reference, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim() === inventory.commit, 'Reference HEAD differs from pinned Harness');
  assert(execFileSync('git', ['-C', reference, 'status', '--porcelain'], { encoding: 'utf8' }).trim() === '', 'Reference checkout is dirty');
}
function verifyReference(source: { upstream: string; upstream_sha256: string; commit?: string }) {
  if (!reference) return;
  assert(!source.upstream.includes('..') && !source.upstream.startsWith('/'), 'Invalid upstream path');
  assert(createHash('sha256').update(execFileSync('git', ['-C', reference, 'show', `${source.commit ?? inventory.commit}:${source.upstream}`])).digest('hex') === source.upstream_sha256, `Upstream hash mismatch: ${source.upstream}`);
}
const destinations = new Set<string>();
for (const entry of inventory.files) {
  assert(!destinations.has(entry.destination), `Duplicate destination ${entry.destination}`);
  destinations.add(entry.destination);
  assert(entry.destination.startsWith('web-console/') && !entry.destination.includes('..'), 'Invalid destination');
  assert(entry.upstream && entry.treatment && /^[a-f0-9]{64}$/.test(entry.upstream_sha256), 'Incomplete provenance');
  verifyReference(entry);
  assert(entry.license.includes('DeepSeek') && Array.isArray(entry.retained_dependencies) && Array.isArray(entry.excluded_dependencies), 'Missing closure/license');
  for (const source of entry.additional_sources ?? []) {
    assert(source.upstream && source.treatment && /^[a-f0-9]{64}$/.test(source.upstream_sha256), 'Incomplete additional source provenance');
    verifyReference({ ...source, commit: source.commit ?? entry.commit });
  }
  const file = resolve(repository, entry.destination);
  assert(/^[a-f0-9]{40}$/.test(entry.commit), 'Missing immutable per-file baseline');
  assert(createHash('sha256').update(readFileSync(file)).digest('hex') === entry.local_sha256, `Local source drift ${file}`);
  assert(existsSync(file), `Missing destination ${entry.destination}`);
  if (/\.(tsx?|css)$/.test(file)) assert(readFileSync(file, 'utf8').includes('Copyright (c) 2026 DeepSeek'), `Missing header ${file}`);
  if (/\.tsx?$/.test(file)) {
    const imports = new Set<string>();
    function collect(node: ts.Node) {
      if ((ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) && node.moduleSpecifier && ts.isStringLiteral(node.moduleSpecifier)) imports.add(node.moduleSpecifier.text);
      if (ts.isCallExpression(node) && node.expression.kind === ts.SyntaxKind.ImportKeyword && ts.isStringLiteral(node.arguments[0])) imports.add(node.arguments[0].text);
      ts.forEachChild(node, collect);
    }
    collect(ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true));
    assert(JSON.stringify([...imports].sort()) === JSON.stringify(entry.retained_dependencies), `Dependency inventory drift ${file}`);
  }

}
function walk(directory: string): string[] { return readdirSync(directory, { withFileTypes: true }).flatMap(item => item.isDirectory() ? walk(resolve(directory, item.name)) : [resolve(directory, item.name)]); }
for (const file of [...walk(resolve(web, 'src')), ...walk(resolve(web, 'test'))]) {
  const content = readFileSync(file, 'utf8');
  if (content.includes('Copyright (c) 2026 DeepSeek')) assert(destinations.has(relative(repository, file)), `Uninventoried derived source ${file}`);
  if (!/\.tsx?$/.test(file) || !file.includes('/src/presentation/')) continue;
  assert(!/\b(localStorage|sessionStorage|indexedDB|WebSocket)\b|\bfetch\s*\(/.test(content), `Presentation authority or persistence API in ${file}`);
  const ast = ts.createSourceFile(file, content, ts.ScriptTarget.Latest, true);
  function check(node: ts.Node) {
    let specifier: string | undefined;
    if ((ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) && node.moduleSpecifier && ts.isStringLiteral(node.moduleSpecifier)) specifier = node.moduleSpecifier.text;
    if (ts.isCallExpression(node) && node.expression.kind === ts.SyntaxKind.ImportKeyword && ts.isStringLiteral(node.arguments[0])) specifier = node.arguments[0].text;
    if (specifier) {
      assert(!/deepseek|cordis|protocol|client|bindings/.test(specifier), `Authority import ${specifier} in ${file}`);
      if (specifier.startsWith('.')) assert(resolve(dirname(file), specifier).startsWith(resolve(web, 'src/presentation') + '/'), `Layer escape ${file}: ${specifier}`);
    }
    ts.forEachChild(node, check);
  }
  check(ast);
}
for (const entry of inventory.inspected_only) verifyReference(entry);
const notice = inventory.files.find((entry: { upstream: string }) => entry.upstream === 'LICENSE');
assert(createHash('sha256').update(readFileSync(resolve(repository, notice.destination))).digest('hex') === notice.upstream_sha256, 'Harness license changed');
if (process.argv.includes('--artifact')) {
  for (const name of ['LICENSE-DeepSeek-Harness.txt', 'THIRD-PARTY-NOTICES.txt']) assert(readFileSync(resolve(web, 'dist', name)).equals(readFileSync(resolve(web, 'public', name))), `Missing production notice ${name}`);
  // Form state, secret field values included, must have no devtools channel in
  // the shipped bundle (see src/app/settings/forms/inert-devtools-event-client.ts).
  for (const file of walk(resolve(web, 'dist')).filter(path => path.endsWith('.js'))) assert(!readFileSync(file, 'utf8').includes('tanstack-connect'), `Form devtools channel shipped in ${file}`);
}
console.log(`Verified ${destinations.size} source records and presentation dependency boundary`);
