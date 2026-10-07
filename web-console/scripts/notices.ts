/** Reproduce notices for the entire pinned production install closure (a superset of bundled code). */
import { readFileSync, readdirSync, realpathSync, existsSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
const root = resolve(import.meta.dirname, '..');
function locate(from: string, name: string): string {
  for (let at = from; ; at = dirname(at)) {
    const candidate = join(at, 'node_modules', name, 'package.json');
    if (existsSync(candidate)) return realpathSync(candidate);
    if (dirname(at) === at) throw new Error(`Missing installed dependency ${name}`);
  }
}
const packages = new Map<string, string>();
/** Reviewed packages that declare a license but publish no license file. Each
 * is pinned to one exact version and one declared license, so an upgrade or a
 * license change fails closed instead of inheriting this record. */
const declaredOnly: Record<string, { license: string | undefined; notice: string }> = {
  '@fortune-sheet/core 1.0.4': { license: 'MIT', notice: readFileSync(join(root, 'licenses/fortune-sheet-1.0.4.txt'), 'utf8') },
  '@fortune-sheet/react 1.0.4': { license: 'MIT', notice: readFileSync(join(root, 'licenses/fortune-sheet-1.0.4.txt'), 'utf8') },
  'brotli 1.3.3': { license: 'MIT', notice: readFileSync(join(root, 'licenses/brotli-1.3.3.txt'), 'utf8') },
  'dfa 1.2.0': { license: 'MIT', notice: 'License: MIT, as declared in the published package manifest. This distribution contains no standalone license text. Project: https://github.com/devongovett/dfa' },
  'fontkit 2.0.4': { license: 'MIT', notice: 'License: MIT, as declared in the published package manifest and README. This distribution contains no standalone license text. Project: https://github.com/foliojs/fontkit' },
  'isarray 1.0.0': { license: 'MIT', notice: readFileSync(join(root, 'licenses/isarray-1.0.0.txt'), 'utf8') },
  'saxes 5.0.1': { license: 'ISC', notice: readFileSync(join(root, 'licenses/saxes-5.0.1.txt'), 'utf8') },
  'binary 0.3.0': { license: 'MIT', notice: readFileSync(join(root, 'licenses/binary-0.3.0.txt'), 'utf8') },
  'chainsaw 0.1.0': { license: 'MIT/X11', notice: readFileSync(join(root, 'licenses/chainsaw-0.1.0.txt'), 'utf8') },
  'buffers 0.1.1': { license: undefined, notice: readFileSync(join(root, 'licenses/buffers-0.1.1.txt'), 'utf8') },
  '@nodable/entities 3.1.0': { license: 'MIT', notice: readFileSync(join(root, 'licenses/nodable-entities-3.1.0.txt'), 'utf8') },
  'saxes 6.0.0': { license: 'ISC', notice: readFileSync(join(root, 'licenses/saxes-6.0.0.txt'), 'utf8') },
  'client-only 0.0.1': {
    license: 'MIT',
    notice: 'License: MIT, as declared in the package manifest; the published package contains no license file.\n'
      + 'An empty marker module (index.js is empty; error.js throws a fixed message) published by the React project, https://github.com/facebook/react. Required by react-aria-components.',
  },
};
function visit(file: string) {
  const pkg = JSON.parse(readFileSync(file, 'utf8'));
  const key = `${pkg.name} ${pkg.version}`;
  if (packages.has(key)) return;
  const files = readdirSync(dirname(file)).filter(name => /^(license|licence|copying|notice)(\.|$)/i.test(name)).sort();
  const declared = declaredOnly[key];
  if (!files.length && declared) {
    if (pkg.license !== declared.license) throw new Error(`Declared license changed: ${key}`);
    packages.set(key, declared.notice);
    for (const name of Object.keys(pkg.dependencies ?? {})) visit(locate(dirname(file), name));
    return;
  }
  if (!files.length) throw new Error(`Missing license text: ${key}`);
  // Normalize line endings and trailing whitespace without changing license text.
  packages.set(key, files.map(name => readFileSync(join(dirname(file), name), 'utf8').replace(/\r\n/g, '\n')).join('\n'));
  for (const name of Object.keys(pkg.dependencies ?? {})) visit(locate(dirname(file), name));
}
const manifest = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
for (const name of Object.keys(manifest.dependencies)) visit(locate(root, name));
const output = 'Production install dependency notices for rustX Web Console\nIncludes the complete pinned install closure; only selected modules enter the browser bundle.\n\n' + [...packages].sort(([a], [b]) => a.localeCompare(b)).map(([name, license]) => `===== ${name} =====\n${license.replace(/[\t ]+$/gm, '').trim()}\n`).join('\n');
const path = join(root, 'public/THIRD-PARTY-NOTICES.txt');
if (process.argv.includes('--write')) writeFileSync(path, output);
else if (readFileSync(path, 'utf8') !== output) throw new Error('Dependency notices drifted: run node scripts/notices.ts --write');
console.log(`Verified notices for ${packages.size} production packages`);
