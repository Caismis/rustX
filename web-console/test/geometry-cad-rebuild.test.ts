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
const pins = { importer: 'c2148e54b456b571238d35cac037d304053d64b2', occt: 'd2abb6d844231cb8f29be6894440874a4700e4a5' };
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });
function checkout() {
  const root = mkdtempSync(join(tmpdir(), 'rustx-cad-rebuild-')); directories.push(root);
  const importer = join(root, 'upstream'), occt = join(importer, 'occt'), bin = join(root, 'bin');
  mkdirSync(join(importer, 'occt-import-js/src'), { recursive: true }); mkdirSync(occt); mkdirSync(bin);
  const git = (repository: string, ...args: string[]) => execFileSync(realGit, ['-C', repository, ...args], { encoding: 'utf8' });
  writeFileSync(join(importer, '.gitignore'), 'occt/\nignored.cpp\n');
  writeFileSync(join(importer, 'CMakeLists.txt'), '\ttarget_compile_options (OcctImportJS PUBLIC "$<$<CONFIG:Release>:-Oz>")\n');
  writeFileSync(join(importer, 'occt-import-js/src/importer-xcaf.cpp'), 'pinned XCAF source\n');
  writeFileSync(join(occt, 'native.cpp'), 'pinned OCCT source\n');
  mkdirSync(join(occt, 'src/StepFile'), { recursive: true });
  const lexer = '# undef YY_DECL\n#endif\n#define YY_DECL int step::scanner::lex (step::parser::semantic_type* /*yylval*/)\n\ntypedef step::parser::token token;\n\n';
  for (const name of ['step.lex', 'lex.step.cxx']) writeFileSync(join(occt, 'src/StepFile', name), lexer);
  for (const repository of [importer, occt]) {
    git(repository, 'init', '--quiet'); git(repository, 'add', '.');
    git(repository, '-c', 'user.name=CAD test', '-c', 'user.email=cad-test@example.invalid', 'commit', '--quiet', '-m', 'Pinned test source');
  }
  const commits = { importer: git(importer, 'rev-parse', 'HEAD').trim(), occt: git(occt, 'rev-parse', 'HEAD').trim() };
  const archives = join(root, 'archive-revisions');
  const executable = (name: string, source: string) => { const path = join(bin, name); writeFileSync(path, source); chmodSync(path, 0o755); };
  // Translate the two production identities to real fixture commits. Git
  // still performs each diff and chooses the tree exported by each archive.
  executable('git', `#!/usr/bin/env bash
set -euo pipefail
if [[ $1 = -C && ( $2 = "$TEST_IMPORTER" || $2 = "$TEST_IMPORTER/occt" ) ]]; then
  repository=$2
  if [[ $repository = "$TEST_IMPORTER" ]]; then
    kind=importer; pin="$TEST_IMPORTER_PIN"; commit="$TEST_IMPORTER_COMMIT"
  else
    kind=occt; pin="$TEST_OCCT_PIN"; commit="$TEST_OCCT_COMMIT"
  fi
  if [[ $3 = rev-parse && $4 = HEAD && $("$TEST_REAL_GIT" -C "$repository" rev-parse HEAD) = "$commit" ]]; then
    printf '%s\\n' "$pin"
    exit
  fi
  if [[ $3 = archive ]]; then
    printf '%s\\t%s\\n' "$kind" "$4" >> "$TEST_ARCHIVE_REVISIONS"
    if [[ $kind = importer && -n $TEST_IMPORTER_AFTER ]]; then
      "$TEST_REAL_GIT" -C "$TEST_IMPORTER" update-ref HEAD "$TEST_IMPORTER_AFTER"
      "$TEST_REAL_GIT" -C "$TEST_IMPORTER/occt" update-ref HEAD "$TEST_OCCT_AFTER"
    fi
  fi
  if [[ $3 = archive && $4 = "$pin" ]]; then
    shift 4
    exec "$TEST_REAL_GIT" -C "$repository" archive "$commit" "$@"
  fi
  if [[ $3 = diff && $4 = --quiet && $5 = "$pin" ]]; then
    shift 5
    exec "$TEST_REAL_GIT" -C "$repository" diff --quiet "$commit" "$@"
  fi
fi
exec "$TEST_REAL_GIT" "$@"
`);
  executable('emcc', '#!/usr/bin/env bash\necho "emcc Emscripten 3.1.69"\n');
  executable('emcmake', '#!/usr/bin/env bash\nprintf "%s\\n" "$@" > "$TEST_BUILD_SENTINEL"\nexit 73\n');
  const target = join(root, 'build'), sentinel = join(root, 'build-reached');
  const run = (after?: typeof commits) => spawnSync('bash', [recipe, target, 'O3', 'off', importer], {
    encoding: 'utf8', env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TEST_IMPORTER: importer, TEST_REAL_GIT: realGit,
      TEST_BUILD_SENTINEL: sentinel, TEST_ARCHIVE_REVISIONS: archives, TEST_IMPORTER_PIN: pins.importer, TEST_OCCT_PIN: pins.occt,
      TEST_IMPORTER_COMMIT: commits.importer, TEST_OCCT_COMMIT: commits.occt, TEST_IMPORTER_AFTER: after?.importer ?? '', TEST_OCCT_AFTER: after?.occt ?? '' },
  });
  return { importer, occt, target, sentinel, commits, archives, git, run };
}

it('accepts a clean local checkout and reaches source copy/configuration before the build sentinel', () => {
  const fixture = checkout(), result = fixture.run();
  expect(result.error).toBeUndefined(); expect(result.status, result.stderr).toBe(73);
  expect(existsSync(fixture.sentinel)).toBe(true);
  expect(readFileSync(join(fixture.target, 'occt-import-js/src/importer-xcaf.cpp'), 'utf8')).toBe('pinned XCAF source\n');
  expect(readFileSync(join(fixture.target, 'occt-import-js/src/importer.hpp'))).toEqual(readFileSync(offeredSource));
  for (const name of ['step.lex', 'lex.step.cxx']) {
    expect(readFileSync(join(fixture.target, 'occt/src/StepFile', name), 'utf8')).toContain('#undef YY_INTERACTIVE');
    expect(readFileSync(join(fixture.occt, 'src/StepFile', name), 'utf8')).not.toContain('#undef YY_INTERACTIVE');
  }
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

it.each(['untracked.cpp', 'ignored.cpp'])('excludes local-only compiler input %s from the pinned tree', name => {
  const fixture = checkout(), source = join('occt-import-js/src', name);
  writeFileSync(join(fixture.importer, source), 'local-only compiler input\n');
  const status = fixture.git(fixture.importer, 'status', '--porcelain', '--untracked-files=all');
  expect(status.includes(name)).toBe(name === 'untracked.cpp');
  const result = fixture.run();
  expect(result.status, result.stderr).toBe(73);
  expect(existsSync(join(fixture.target, source))).toBe(false);
});

it('materializes both pinned source trees when HEAD moves after validation', () => {
  const fixture = checkout(), after = { ...fixture.commits };
  const files = { importer: 'occt-import-js/src/importer-xcaf.cpp', occt: 'native.cpp' };
  for (const kind of ['importer', 'occt'] as const) {
    const repository = fixture[kind], path = files[kind];
    writeFileSync(join(repository, path), 'different revision\n');
    fixture.git(repository, 'add', path);
    fixture.git(repository, '-c', 'user.name=CAD test', '-c', 'user.email=cad-test@example.invalid', 'commit', '--quiet', '-m', 'Different source');
    after[kind] = fixture.git(repository, 'rev-parse', 'HEAD').trim();
    fixture.git(repository, 'checkout', '--quiet', '--detach', fixture.commits[kind]);
  }
  const result = fixture.run(after);
  expect(result.status, result.stderr).toBe(73);
  for (const kind of ['importer', 'occt'] as const) {
    const path = files[kind];
    expect(fixture.git(fixture[kind], 'rev-parse', 'HEAD').trim()).toBe(after[kind]);
    const exported = join(fixture.target, kind === 'occt' ? 'occt' : '', path);
    expect(readFileSync(exported, 'utf8')).toBe(fixture.git(fixture[kind], 'show', `${fixture.commits[kind]}:${path}`));
  }
  expect(readFileSync(fixture.archives, 'utf8').trim().split('\n')).toEqual([
    `importer\t${pins.importer}`, `occt\t${pins.occt}`,
  ]);
});
