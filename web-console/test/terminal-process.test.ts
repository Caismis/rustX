// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { terminalProcess } from '../host/terminal-process';

it('carries a large terminal environment in a private snapshot retained through physical settlement', async () => {
  const root = mkdtempSync(join(tmpdir(), 'rx-env-'));
  vi.stubEnv('TMPDIR', root);
  vi.stubEnv('HOME', root);
  vi.stubEnv('USER', 'a'.repeat(75_000));
  vi.stubEnv('LOGNAME', 'b'.repeat(75_000));
  let output = '';
  let observed!: () => void;
  const received = new Promise<void>(resolve => { observed = resolve; });
  const terminal = terminalProcess(fileURLToPath(new URL('../../target/debug/interactive-supervisor', import.meta.url)), '/bin/bash', root, data => { output += data; if (output.includes('lengths=75000/75000 transport=unset')) observed(); });
  try {
    await terminal.ready;
    const directories = readdirSync(root).filter(name => name.startsWith('rustx-pty-'));
    expect(directories).toHaveLength(1);
    const directory = join(root, directories[0]!);
    const snapshot = join(directory, 'environment.json');
    expect(statSync(snapshot).mode & 0o777).toBe(0o600);
    const environment = new Map<string, string>(JSON.parse(readFileSync(snapshot, 'utf8')));
    expect(environment.get('USER')).toHaveLength(75_000);
    expect(environment.get('LOGNAME')).toHaveLength(75_000);
    expect(environment.has('RUSTX_COMMAND_ENVIRONMENT_FILE')).toBe(false);
    terminal.write('printf "lengths=%s/%s transport=%s\\n" "${#USER}" "${#LOGNAME}" "${RUSTX_COMMAND_ENVIRONMENT_FILE-unset}"\r');
    await received;
    terminal.write('exit\r');
    await terminal.done;
    expect(output).toContain('lengths=75000/75000 transport=unset');
    expect(existsSync(directory)).toBe(false);
  } finally {
    try { await terminal.stop(); }
    finally { vi.unstubAllEnvs(); rmSync(root, { recursive: true, force: true }); }
  }
}, 15_000);
