/** CI/operator admission probe: same controls and mounts as real conversion. */
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { runOfficeSandbox } from '../host/documents/office-sandbox.ts';
const directory = await mkdtemp(join(tmpdir(), 'rustx-office-probe-'));
try { await runOfficeSandbox(directory, ['/usr/bin/true'], new AbortController().signal); }
finally { await rm(directory, { recursive: true, force: true }); }
console.log('Office cgroup and filesystem admission passed');
