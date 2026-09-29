import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';

test('native Node loads real Workspace Host and HTTP handler with shared authority errors', async () => {
  const env = { ...process.env };
  delete env.NODE_OPTIONS;
  delete env.NODE_PATH;
  // execFile does not inherit process.execArgv. In particular, no transform or
  // loader used by a parent runner can conceal a broken native import graph.
  const { stdout } = await promisify(execFile)(process.execPath, [
    fileURLToPath(new URL('./fixtures/workspace-host-native.ts', import.meta.url)),
  ], { env, timeout: 20_000 });
  assert.equal(stdout.trim(), 'native Workspace boundary passed');
});
