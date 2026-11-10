// Resolve the unmodified reference sources against an existing dependency tree.
// No files, links, or build configuration are written into either reference repo.
import { registerHooks } from 'node:module';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const checkout = process.env.REFERENCE_PI_CHECKOUT;
const dependencyParent = pathToFileURL(join(process.env.REFERENCE_PI_DEPS, 'package.json')).href;
const packages = new Map();
for (const name of ['ai', 'chord', 'durable', 'telemetry', 'env']) {
  const root = join(checkout, 'packages', name);
  const manifest = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
  packages.set(manifest.name, { root, exports: manifest.exports });
}
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.startsWith('@earendil-works/')) {
      const pieces = specifier.split('/');
      const pkg = packages.get(pieces.slice(0, 2).join('/'));
      if (pkg) {
        const subpath = pieces.length === 2 ? '.' : './' + pieces.slice(2).join('/');
        let entry = pkg.exports[subpath];
        let wildcard;
        if (!entry) {
          for (const [key, value] of Object.entries(pkg.exports)) {
            if (key.endsWith('*') && subpath.startsWith(key.slice(0, -1))) {
              entry = value;
              wildcard = subpath.slice(key.length - 1);
              break;
            }
          }
        }
        if (!entry) throw new Error(`Unmapped workspace import: ${specifier}`);
        let target = typeof entry === 'string' ? entry : entry.source ?? entry.import;
        if (wildcard !== undefined) target = target.replace('*', wildcard);
        if (target.startsWith('./dist/')) target = target.replace('./dist/', './src/').replace(/\.js$/, '.ts');
        return nextResolve(pathToFileURL(join(pkg.root, target)).href, context);
      }
    }
    try {
      return nextResolve(specifier, context);
    } catch (error) {
      if (error.code !== 'ERR_MODULE_NOT_FOUND' || specifier.startsWith('.') || specifier.startsWith('/') || specifier.includes(':')) throw error;
      return nextResolve(specifier, { ...context, parentURL: dependencyParent });
    }
  },
});
