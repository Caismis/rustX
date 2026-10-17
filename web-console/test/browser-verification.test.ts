// @vitest-environment node
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import type { JSONReport, JSONReportSuite, JSONReportTest } from '@playwright/test/reporter';
import { afterEach, describe, expect, it } from 'vitest';
import { EXECUTABLES, INSTALLED_MANIFEST, check, fileErrors, identityErrors, install, record, type Identity, type Manifest } from '../scripts/browser-artifact.ts';
import { assign, verify, weightsOf, type Inventory, type ShardIdentity, type ShardRun } from '../scripts/browser-shards.ts';

const identity: Identity = {
  commit: 'a'.repeat(40), platform: 'linux', arch: 'x64', rust: '1.98.1',
  cargo: { profile: 'dev', command: 'cargo build --bins --example document_artifact_fixture --locked', features: 'default', debuginfo: 'stripped' },
  node: { version: 'v24.19.0', abi: '137' }, pnpm: '11.13.1', playwright: '1.63.0', sources: { 'Cargo.lock': 'c'.repeat(64) },
};

describe('sharded browser coverage', () => {
  const prepared = 'p'.repeat(64);
  const inventory = (...ids: string[]): Inventory => ({ commit: identity.commit, prepared, tests: ids.map(id => ({ id: `${id}@`, title: `title ${id}`, file: 'spec.ts' })), shards: [['spec.ts'], []] });
  type Ran = { id: string; status?: JSONReportTest['status']; result?: 'passed' | 'failed' | 'skipped'; expected?: JSONReportTest['expectedStatus']; skip?: boolean; repeats?: number };
  /** Playwright's JSON report of one shard that ran `ran`, each spec's id being `id`. */
  function report(...ran: Ran[]): Pick<JSONReport, 'suites'> {
    const suite: JSONReportSuite = { title: 'spec.ts', file: 'spec.ts', line: 0, column: 0, specs: ran.map(({ id, status = 'expected', result = 'passed', expected = 'passed', skip = false, repeats = 1 }) => ({
      id, title: `title ${id}`, tags: [], ok: status !== 'unexpected', file: 'spec.ts', line: 1, column: 1,
      tests: [{ timeout: 1, annotations: skip ? [{ type: 'skip' }] : [], expectedStatus: expected, projectName: '', projectId: '', status, results: Array.from({ length: repeats }, () => ({ workerIndex: 0, parallelIndex: 0, status: result, duration: 1000, error: undefined, errors: [], stdout: [], stderr: [], retry: 0, startTime: new Date(0).toISOString(), attachments: [], annotations: [] })) }],
    })) };
    return { suites: [suite] };
  }
  const shard = (index: number, ran?: Pick<JSONReport, 'suites'>, overrides: Partial<ShardIdentity> = {}): ShardRun => ({ identity: { commit: identity.commit, prepared, shard: index, total: 2, ...overrides }, report: ran });
  const skippedC: Ran = { id: 'c', status: 'skipped', result: 'skipped', expected: 'skipped', skip: true };
  const passing = [shard(1, report({ id: 'a' })), shard(2, report({ id: 'b' }, skippedC))];
  const complete = inventory('a', 'b', 'c');

  it('accepts every discovered test run once across both shards, keeping intentional skips', () => {
    const coverage = verify(complete, passing, 2);
    expect(coverage.errors).toEqual([]);
    expect(coverage).toMatchObject({ total: 3, passed: 2, skipped: ['title c'] });
    expect(coverage.shards.map(({ index, tests }) => [index, tests])).toEqual([[1, 1], [2, 2]]);
  });
  it('fails when a shard or its report is missing', () => {
    expect(verify(complete, [passing[0]], 2).errors).toEqual(['shard 2/2: missing its report', 'title b: never executed by any shard', 'title c: never executed by any shard', 'shard 2/2: executed no test']);
    expect(verify(complete, [passing[0], shard(2)], 2).errors).toEqual(expect.arrayContaining(['shard 2/2: missing its report']));
  });
  it('fails when a shard ran nothing although it reported', () => {
    expect(verify(inventory('a'), [shard(1, report({ id: 'a' })), shard(2, report())], 2).errors).toEqual(['shard 2/2: executed no test']);
  });
  it('fails a test assigned to two shards or executed twice, and a shard reported twice', () => {
    const errors = verify(complete, [shard(1, report({ id: 'a' })), shard(2, report({ id: 'a' }, { id: 'b' }, { ...skippedC, repeats: 2 }))], 2).errors;
    expect(errors).toEqual(['spec.ts › title a: reported by shard 1 and shard 2', 'title c: executed 2 times']);
    expect(verify(complete, [...passing, passing[1]], 2).errors).toEqual(expect.arrayContaining(['shard 2/2: reported more than once']));
  });
  it('fails a discovered mandatory test that no shard executed, and an undiscovered one that ran', () => {
    const errors = verify(inventory('a', 'b', 'd'), [shard(1, report({ id: 'a' })), shard(2, report({ id: 'b' }, { id: 'e' }))], 2).errors;
    expect(errors).toEqual(['spec.ts › title e: ran but is not in the inventory', 'title d: never executed by any shard']);
  });
  it('fails a test skipped without its own skip, and any failure', () => {
    const errors = verify(complete, [shard(1, report({ id: 'a', status: 'skipped', result: 'skipped' })), shard(2, report({ id: 'b', status: 'unexpected', result: 'failed' }, { id: 'c' }))], 2).errors;
    expect(errors).toEqual(["title a: skipped without the test's own skip, so it never ran", 'title b: failed']);
  });
  it('fails shards that ran another commit or other prepared inputs, or belong to another split', () => {
    const errors = verify(complete, [shard(1, report({ id: 'a' }), { commit: 'b'.repeat(40) }), shard(2, report({ id: 'b' }, skippedC), { prepared: 'q'.repeat(64) })], 2).errors;
    expect(errors).toEqual([`shard 1: ran commit ${'b'.repeat(40)}, inventory is ${identity.commit}`, `shard 2: ran prepared inputs ${'q'.repeat(64)}, inventory is ${prepared}`]);
    expect(verify(complete, [shard(1, report({ id: 'a' }, { id: 'b' }, skippedC), { total: 1 })], 1).errors).toEqual([]);
    expect(verify(complete, [shard(1, report({ id: 'a' }, { id: 'b' }, skippedC), { total: 3 })], 1).errors).toEqual(['shard 1/3: not one of 1 expected shards']);
  });
  it('fails an empty inventory instead of treating zero tests as coverage', () => {
    expect(verify(inventory(), [shard(1, report()), shard(2, report())], 2).errors).toEqual(expect.arrayContaining(['the inventory discovered no browser tests']));
  });
});

describe('shard assignment', () => {
  const tests = (counts: Record<string, number>) => Object.entries(counts).flatMap(([file, count]) => Array.from({ length: count }, () => ({ file })));
  it('places every file exactly once, heaviest first onto the lightest shard', () => {
    const shards = assign(tests({ 'a.spec.ts': 1, 'b.spec.ts': 1, 'c.spec.ts': 1, 'd.spec.ts': 1 }), { 'a.spec.ts': 60, 'b.spec.ts': 50, 'c.spec.ts': 40, 'd.spec.ts': 30 }, 2);
    expect(shards).toEqual([['a.spec.ts', 'd.spec.ts'], ['b.spec.ts', 'c.spec.ts']]);
    expect(shards.flat().sort()).toEqual(['a.spec.ts', 'b.spec.ts', 'c.spec.ts', 'd.spec.ts']);
  });
  it('weighs an unmeasured file by its test count at the measured mean, deterministically', () => {
    const input = tests({ 'measured.spec.ts': 2, 'new.spec.ts': 10, 'small.spec.ts': 1 });
    const shards = assign(input, { 'measured.spec.ts': 20, 'small.spec.ts': 10 }, 2);
    // 30 s over 3 measured tests: the new file weighs 100 s and takes a shard alone.
    expect(shards).toEqual([['new.spec.ts'], ['measured.spec.ts', 'small.spec.ts']]);
    expect(assign([...input].reverse(), { 'measured.spec.ts': 20, 'small.spec.ts': 10 }, 2)).toEqual(shards);
  });
  it('weighs each test by its wall span until the next test of its run starts', () => {
    const result = (startTime: string, duration: number) => ({ workerIndex: 0, parallelIndex: 0, status: 'passed' as const, duration, error: undefined, errors: [], stdout: [], stderr: [], retry: 0, startTime, attachments: [], annotations: [] });
    const spec = (file: string, id: string, start: string, duration: number) => ({ id, title: 't', tags: [], ok: true, file, line: 1, column: 1, tests: [{ timeout: 1, annotations: [], expectedStatus: 'passed' as const, projectName: '', projectId: '', status: 'expected' as const, results: [result(start, duration)] }] });
    const suite = (file: string, specs: ReturnType<typeof spec>[]) => ({ title: file, file, line: 0, column: 0, specs });
    // a: 1.5 s test, then 2.5 s until b starts; b: last test, its own 3 s.
    const report = { suites: [suite('b.spec.ts', [spec('b.spec.ts', '2', '2026-01-01T00:00:04.000Z', 3000)]), suite('a.spec.ts', [spec('a.spec.ts', '1', '2026-01-01T00:00:00.000Z', 1500)])] };
    expect(weightsOf([report])).toEqual({ 'a.spec.ts': 4, 'b.spec.ts': 3 });
  });
});

describe('prepared browser inputs', () => {
  const directories: string[] = [];
  afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });
  function directory() { const path = mkdtempSync(join(tmpdir(), 'rustx-prepared-')); directories.push(path); return path; }
  /** A staged archive holding every executable and a one-file bundle. */
  function staged(): { stage: string; manifest: Manifest } {
    const stage = directory();
    const write = (file: string, content: string, executable = false) => {
      mkdirSync(dirname(join(stage, file)), { recursive: true });
      writeFileSync(join(stage, file), content);
      if (executable) chmodSync(join(stage, file), 0o755);
    };
    for (const file of EXECUTABLES) write(file, file, true);
    write('web-console/dist/index.html', '<html>');
    const manifest: Manifest = { schema: 1, identity, provenance: { rustc: '', environment: {} }, files: {} };
    manifest.files = Object.fromEntries([...EXECUTABLES, 'web-console/dist/index.html'].map(file => [file, record(join(stage, file))]));
    writeFileSync(join(stage, 'browser-prepared.json'), JSON.stringify(manifest));
    return { stage, manifest };
  }

  it('installs into a clean checkout and re-verifies the installed copy', () => {
    const { stage } = staged();
    const root = directory();
    install(stage, root, identity);
    expect(check(root, identity).identity).toEqual(identity);
    writeFileSync(join(root, 'target/debug/rustx'), 'tampered');
    expect(() => check(root, identity)).toThrow('target/debug/rustx: content differs from the prepared checksum');
  });
  it('refuses inputs prepared for another commit', () => {
    const { stage } = staged();
    expect(() => install(stage, directory(), { ...identity, commit: 'b'.repeat(40) })).toThrow(`identity.commit: prepared "${identity.commit}", required "${'b'.repeat(40)}"`);
  });
  it('refuses inputs prepared on another platform or under another build identity', () => {
    expect(identityErrors({ ...identity, platform: 'darwin', arch: 'arm64' }, identity)).toEqual(['identity.platform: prepared "darwin", required "linux"', 'identity.arch: prepared "arm64", required "x64"']);
    expect(identityErrors({ ...identity, cargo: { ...identity.cargo, features: 'all' as 'default' }, node: { ...identity.node, abi: '127' } }, identity)).toEqual(['identity.cargo.features: prepared "all", required "default"', 'identity.node.abi: prepared "127", required "137"']);
    const { stage } = staged();
    expect(() => install(stage, directory(), { ...identity, rust: '1.99.0' })).toThrow('identity.rust: prepared "1.98.1", required "1.99.0"');
  });
  it('refuses a missing manifest, a missing or altered file, a lost executable mode and an extra bundle file', () => {
    expect(() => install(directory(), directory(), identity)).toThrow('No prepared browser manifest');
    const missing = staged();
    rmSync(join(missing.stage, 'target/debug/bash-supervisor'));
    expect(() => install(missing.stage, directory(), identity)).toThrow('target/debug/bash-supervisor: missing');
    const altered = staged();
    writeFileSync(join(altered.stage, 'web-console/dist/index.html'), '<html>stale');
    chmodSync(join(altered.stage, 'target/debug/rustx'), 0o644);
    writeFileSync(join(altered.stage, 'web-console/dist/extra.js'), '');
    const errors = fileErrors(altered.manifest, altered.stage);
    expect(errors).toEqual(expect.arrayContaining(['web-console/dist/index.html: content differs from the prepared checksum', 'target/debug/rustx: executable mode differs', 'web-console/dist/extra.js: not part of the prepared bundle']));
    const incomplete = staged();
    delete incomplete.manifest.files['target/debug/interactive-supervisor'];
    expect(fileErrors(incomplete.manifest, incomplete.stage)).toEqual(['target/debug/interactive-supervisor: not a prepared executable']);
  });
  it('never overwrites an existing build output, and checks nothing without an installed manifest', () => {
    const { stage } = staged();
    const root = directory();
    mkdirSync(join(root, 'target/debug'), { recursive: true });
    writeFileSync(join(root, 'target/debug/rustx'), 'local build');
    expect(() => install(stage, root, identity)).toThrow('target/debug/rustx: a build output already exists');
    expect(() => check(directory(), identity)).toThrow(`${INSTALLED_MANIFEST} is absent`);
  });
});

