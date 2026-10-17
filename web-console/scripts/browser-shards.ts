/**
 * Complete-coverage gate for sharded browser acceptance.
 *
 * The inventory is Playwright's own `--list` discovery of the checked-out
 * suite, taken in the preparation job. Each shard records which prepared
 * inputs it ran against beside Playwright's JSON report of its own run (its
 * blob report feeds the one merged HTML report). `verify` passes only when
 * every discovered test ran exactly once, in exactly one expected shard,
 * against the same commit and prepared inputs, and either passed or was
 * intentionally skipped by the test itself.
 */
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { JSONReport, JSONReportSuite, JSONReportTest, JSONReportTestResult } from '@playwright/test/reporter';
import { INSTALLED_MANIFEST } from './browser-artifact.ts';

const web = fileURLToPath(new URL('..', import.meta.url));
const repository = resolve(web, '..');

/** What one shard, or the inventory, ran or was taken against. */
export interface RunIdentity { commit: string; prepared: string }
export interface ShardIdentity extends RunIdentity { shard: number; total: number }
export interface Inventory extends RunIdentity { tests: { id: string; title: string }[] }
/** One shard's evidence: its identity and Playwright's own report of its run. */
export interface ShardRun { identity: ShardIdentity; report?: Pick<JSONReport, 'suites'> }

export interface Case { id: string; title: string; test: JSONReportTest }
/** Every test of a report, keyed by Playwright's spec id and project. */
export function cases(report: Pick<JSONReport, 'suites'>): Case[] {
  const found: Case[] = [];
  const visit = (suite: JSONReportSuite, path: string[]) => {
    const titles = suite.title ? [...path, suite.title] : path;
    for (const spec of suite.specs) for (const test of spec.tests) found.push({ id: `${spec.id}@${test.projectName}`, title: [...titles, spec.title].join(' › '), test });
    for (const child of suite.suites ?? []) visit(child, titles);
  };
  for (const suite of report.suites) visit(suite, []);
  return found;
}

function digest(path: string) { return createHash('sha256').update(readFileSync(path)).digest('hex'); }
function commit() { return execFileSync('git', ['rev-parse', 'HEAD'], { cwd: repository, encoding: 'utf8' }).trim(); }
/**
 * The commit and prepared inputs this checkout runs against: a shard's
 * installed manifest, or the preparation job's staged one, which install
 * copies byte for byte.
 */
export function currentIdentity(manifest = join(repository, INSTALLED_MANIFEST)): RunIdentity {
  if (!existsSync(manifest)) throw new Error(`${manifest} is absent; browser shards run only against prepared inputs`);
  return { commit: commit(), prepared: digest(manifest) };
}

export interface Shard { index: number; tests: number; passed: number; skipped: number; failed: number; wallMs: number; testMs: number }
export interface Coverage { errors: string[]; total: number; passed: number; skipped: string[]; shards: Shard[] }

const end = (result: JSONReportTestResult) => Date.parse(result.startTime) + result.duration;
/**
 * Every discovered test ran exactly once, in exactly one of shards 1..total,
 * and passed or skipped itself; every shard reported, ran something, and ran
 * against the inventory's commit and prepared inputs.
 */
export function verify(inventory: Inventory, runs: ShardRun[], total: number): Coverage {
  const errors: string[] = [];
  if (!Number.isInteger(total) || total < 1) errors.push(`invalid shard total ${total}`);
  if (inventory.tests.length === 0) errors.push('the inventory discovered no browser tests');
  for (let index = 1; index <= total; index++) {
    const reported = runs.filter(run => run.identity.shard === index);
    if (reported.length > 1) errors.push(`shard ${index}/${total}: reported more than once`);
    else if (!reported.length || !reported[0].report) errors.push(`shard ${index}/${total}: missing its report`);
  }
  for (const { identity } of runs) {
    if (identity.total !== total || identity.shard < 1 || identity.shard > total) errors.push(`shard ${identity.shard}/${identity.total}: not one of ${total} expected shards`);
    if (identity.commit !== inventory.commit) errors.push(`shard ${identity.shard}: ran commit ${identity.commit}, inventory is ${inventory.commit}`);
    if (identity.prepared !== inventory.prepared) errors.push(`shard ${identity.shard}: ran prepared inputs ${identity.prepared}, inventory is ${inventory.prepared}`);
  }
  const discovered = new Map(inventory.tests.map(test => [test.id, test.title]));
  if (discovered.size !== inventory.tests.length) errors.push('the inventory lists a test identity twice');
  const ran = new Map<string, { shard: number; test: JSONReportTest }>();
  for (const run of runs) for (const found of cases(run.report ?? { suites: [] })) {
    const earlier = ran.get(found.id);
    if (earlier) errors.push(`${found.title}: reported by shard ${earlier.shard} and shard ${run.identity.shard}`);
    else ran.set(found.id, { shard: run.identity.shard, test: found.test });
    if (!discovered.has(found.id)) errors.push(`${found.title}: ran but is not in the inventory`);
  }
  const shards = new Map<number, JSONReportTestResult[]>();
  let passed = 0;
  const skipped: string[] = [];
  for (const [id, title] of discovered) {
    const found = ran.get(id);
    if (!found) { errors.push(`${title}: never executed by any shard`); continue; }
    const { shard, test } = found;
    if (test.results.length !== 1) { errors.push(`${title}: executed ${test.results.length} times`); continue; }
    const [result] = test.results;
    shards.set(shard, [...(shards.get(shard) ?? []), result]);
    const intentional = test.expectedStatus === 'skipped' && test.annotations.some(annotation => annotation.type === 'skip' || annotation.type === 'fixme');
    if (test.status === 'expected' && result.status === 'passed') passed++;
    else if (test.status === 'skipped' && intentional) skipped.push(title);
    else if (test.status === 'skipped') errors.push(`${title}: skipped without the test's own skip, so it never ran`);
    else errors.push(`${title}: ${result.status ?? test.status}`);
  }
  const summary: Shard[] = [];
  for (let index = 1; index <= total; index++) {
    const results = shards.get(index) ?? [];
    if (results.length === 0) errors.push(`shard ${index}/${total}: executed no test`);
    const start = Math.min(...results.map(result => Date.parse(result.startTime)));
    summary.push({
      index, tests: results.length,
      passed: results.filter(result => result.status === 'passed').length,
      skipped: results.filter(result => result.status === 'skipped').length,
      failed: results.filter(result => !['passed', 'skipped'].includes(result.status ?? '')).length,
      wallMs: results.length ? Math.max(...results.map(end)) - start : 0,
      testMs: results.reduce((sum, result) => sum + result.duration, 0),
    });
  }
  return { errors, total: discovered.size, passed, skipped, shards: summary };
}

function markdown(coverage: Coverage, inventory: Inventory) {
  const minutes = (ms: number) => `${(ms / 60_000).toFixed(1)} min`;
  return [
    `### Browser acceptance: ${coverage.errors.length ? 'FAILED' : 'complete'}`,
    '', `Commit \`${inventory.commit}\`, prepared inputs \`${inventory.prepared.slice(0, 16)}\`.`,
    `${coverage.total} discovered; ${coverage.passed} passed; ${coverage.skipped.length} intentionally skipped.`,
    '', '| Shard | Tests | Passed | Skipped | Failed | Wall | Test time |', '| --- | --- | --- | --- | --- | --- | --- |',
    ...coverage.shards.map(shard => `| ${shard.index} | ${shard.tests} | ${shard.passed} | ${shard.skipped} | ${shard.failed} | ${minutes(shard.wallMs)} | ${minutes(shard.testMs)} |`),
    ...(coverage.skipped.length ? ['', 'Intentionally skipped:', ...coverage.skipped.map(title => `- ${title}`)] : []),
    ...(coverage.errors.length ? ['', 'Coverage failures:', ...coverage.errors.map(error => `- ${error}`)] : []),
    '',
  ].join('\n');
}

function playwrightList(): JSONReport {
  const output = execFileSync(process.execPath, [join(web, 'node_modules/@playwright/test/cli.js'), 'test', '--list', '--reporter=json'], { cwd: web, encoding: 'utf8', maxBuffer: 64 << 20 });
  return JSON.parse(output) as JSONReport;
}

if (import.meta.main) {
  const [command, ...rest] = process.argv.slice(2);
  if (command === 'inventory' && rest.length === 2) {
    const inventory: Inventory = { ...currentIdentity(resolve(rest[0])), tests: cases(playwrightList()).map(({ id, title }) => ({ id, title })) };
    if (inventory.tests.length === 0) throw new Error('Playwright discovered no browser tests');
    writeFileSync(rest[1], `${JSON.stringify(inventory, null, 2)}\n`);
    console.log(`Inventory: ${inventory.tests.length} browser tests at ${inventory.commit}`);
  } else if (command === 'identity' && rest.length === 2) {
    const [shard, total] = rest[0].split('/').map(Number);
    const identity: ShardIdentity = { ...currentIdentity(), shard, total };
    writeFileSync(rest[1], `${JSON.stringify(identity)}\n`);
  } else if (command === 'verify' && rest.length === 3) {
    // Each shard directory holds identity.json and Playwright's report.json.
    const [inventoryFile, shardDirectory, total] = rest;
    const inventory = JSON.parse(readFileSync(inventoryFile, 'utf8')) as Inventory;
    const runs: ShardRun[] = (existsSync(shardDirectory) ? readdirSync(shardDirectory, { withFileTypes: true }).filter(entry => entry.isDirectory()) : [])
      .filter(entry => existsSync(join(shardDirectory, entry.name, 'identity.json')))
      .map(entry => {
        const directory = join(shardDirectory, entry.name);
        const report = join(directory, 'report.json');
        return { identity: JSON.parse(readFileSync(join(directory, 'identity.json'), 'utf8')) as ShardIdentity, report: existsSync(report) ? JSON.parse(readFileSync(report, 'utf8')) as JSONReport : undefined };
      });
    const coverage = verify(inventory, runs, Number(total));
    const text = markdown(coverage, inventory);
    console.log(text);
    if (process.env.GITHUB_STEP_SUMMARY) writeFileSync(process.env.GITHUB_STEP_SUMMARY, text, { flag: 'a' });
    if (coverage.errors.length) process.exit(1);
  } else {
    throw new Error('usage: browser-shards.ts inventory <staged-manifest> <out> | identity <i/n> <out> | verify <inventory> <shard-dir> <total>');
  }
}
