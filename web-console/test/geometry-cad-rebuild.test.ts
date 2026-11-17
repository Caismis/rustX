// @vitest-environment node
import { execFileSync, spawnSync } from 'node:child_process';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, expect, it } from 'vitest';

const directories: string[] = [];
const realGit = execFileSync('sh', ['-c', 'command -v git'], { encoding: 'utf8' }).trim();
const recipe = fileURLToPath(new URL('../vendor/occt-import-js/rebuild.sh', import.meta.url));
const offeredSource = fileURLToPath(new URL('../vendor/occt-import-js/src/importer.hpp', import.meta.url));
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });
function checkout() {
  const root = mkdtempSync(join(tmpdir(), 'rustx-cad-rebuild-')); directories.push(root);
  const importer = join(root, 'upstream'), occt = join(importer, 'occt'), bin = join(root, 'bin');
  mkdirSync(join(importer, 'occt-import-js/src'), { recursive: true }); mkdirSync(occt); mkdirSync(bin);
  const git = (repository: string, ...args: string[]) => execFileSync(realGit, ['-C', repository, ...args], { encoding: 'utf8' });
  writeFileSync(join(importer, '.gitignore'), 'occt/\n');
  writeFileSync(join(importer, 'CMakeLists.txt'), '\ttarget_compile_options (OcctImportJS PUBLIC "$<$<CONFIG:Release>:-Oz>")\n');
  writeFileSync(join(importer, 'occt-import-js/src/importer-xcaf.cpp'), 'pinned XCAF source\n');
  writeFileSync(join(occt, 'native.cpp'), 'pinned OCCT source\n');
  for (const repository of [importer, occt]) {
    git(repository, 'init', '--quiet'); git(repository, 'add', '.');
    git(repository, '-c', 'user.name=CAD test', '-c', 'user.email=cad-test@example.invalid', 'commit', '--quiet', '-m', 'Pinned test source');
  }
  const executable = (name: string, source: string) => { const path = join(bin, name); writeFileSync(path, source); chmodSync(path, 0o755); };
  // Tiny real repositories cannot have the production SHA identities. Only
  // that identity probe is stubbed; every diff uses real Git/index/worktree.
  executable('git', `#!/usr/bin/env bash
if [[ $# = 4 && $1 = -C && $3 = rev-parse && $4 = HEAD ]]; then
  if [[ $2 = "$TEST_IMPORTER" ]]; then echo c2148e54b456b571238d35cac037d304053d64b2; else echo d2abb6d844231cb8f29be6894440874a4700e4a5; fi
else exec "$TEST_REAL_GIT" "$@"; fi
`);
  executable('emcc', '#!/usr/bin/env bash\necho "emcc Emscripten 3.1.69"\n');
  executable('emcmake', '#!/usr/bin/env bash\nprintf "%s\\n" "$@" > "$TEST_BUILD_SENTINEL"\nexit 73\n');
  const target = join(root, 'build'), sentinel = join(root, 'build-reached');
  const run = () => spawnSync('bash', [recipe, target, 'O3', 'off', importer], {
    encoding: 'utf8', env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TEST_IMPORTER: importer, TEST_REAL_GIT: realGit, TEST_BUILD_SENTINEL: sentinel },
  });
  return { importer, occt, target, sentinel, git, run };
}

it('accepts a clean local checkout and reaches source copy/configuration before the build sentinel', () => {
  const fixture = checkout(), result = fixture.run();
  expect(result.error).toBeUndefined(); expect(result.status, result.stderr).toBe(73);
  expect(existsSync(fixture.sentinel)).toBe(true);
  expect(readFileSync(join(fixture.target, 'occt-import-js/src/importer-xcaf.cpp'), 'utf8')).toBe('pinned XCAF source\n');
  expect(readFileSync(join(fixture.target, 'occt-import-js/src/importer.hpp'))).toEqual(readFileSync(offeredSource));
  expect(readFileSync(join(fixture.target, 'CMakeLists.txt'), 'utf8')).toContain('target_link_options (OcctImportJS PUBLIC -O3)');
});
it.each([
  ['importer', 'staged'], ['importer', 'unstaged'], ['occt', 'staged'], ['occt', 'unstaged'],
] as const)('rejects %s %s tracked changes before creating the build target', (kind, state) => {
  const fixture = checkout(), repository = fixture[kind];
  const path = kind === 'importer' ? 'occt-import-js/src/importer-xcaf.cpp' : 'native.cpp';
  writeFileSync(join(repository, path), 'modified source that the offered overrides do not replace\n');
  if (state === 'staged') fixture.git(repository, 'add', path);
  const diff = (...args: string[]) => spawnSync(realGit, ['-C', repository, 'diff', '--quiet', ...args]).status;
  // A staged-only change is invisible to the former worktree-vs-index guard.
  expect(diff()).toBe(state === 'staged' ? 0 : 1); expect(diff('HEAD', '--')).toBe(1);
  const result = fixture.run();
  expect(result.error).toBeUndefined(); expect(result.status).toBe(1);
  expect(existsSync(fixture.target)).toBe(false); expect(existsSync(fixture.sentinel)).toBe(false);
});
