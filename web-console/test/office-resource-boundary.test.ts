// @vitest-environment node
import { mkdtemp, rm, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, expect, it, vi } from 'vitest';
const observation = vi.hoisted(() => ({ unit: '', line: (_: string) => {}, launcher: undefined as import('node:child_process').ChildProcess | undefined }));
vi.mock('node:child_process', async original => {
  const actual = await original<typeof import('node:child_process')>();
  return { ...actual, spawn: (...args: Parameters<typeof actual.spawn>) => {
    const child = actual.spawn(...args);
    if (args[0] === '/usr/bin/systemd-run') {
      observation.unit = (args[1] as string[]).find(x => x.startsWith('--unit='))!.slice(7);
      observation.launcher = child;
      let pending = '';
      child.stdout!.on('data', chunk => { pending += String(chunk); while (pending.includes('\n')) { const at = pending.indexOf('\n'); observation.line(pending.slice(0, at)); pending = pending.slice(at + 1); } });
    }
    return child;
  } };
});
import { runOfficeSandbox } from '../host/documents/office-sandbox.ts';
const directories: string[] = [];
afterEach(async () => { vi.useRealTimers(); observation.line = () => {}; await Promise.all(directories.splice(0).map(path => rm(path, { recursive: true, force: true }))); });
async function run(script: string, signal = new AbortController().signal) {
  const directory = await mkdtemp(join(tmpdir(), 'office-boundary-')); directories.push(directory);
  return runOfficeSandbox(directory, ['/usr/bin/python3', '-c', script], signal);
}
function ready() { let resolve!: () => void; const promise = new Promise<void>(r => { resolve = r; }); observation.line = line => { if (line === 'PROBE-READY') resolve(); }; return promise; }
async function group() {
  const { execFile } = await import('node:child_process');
  return new Promise<string>((resolve, reject) => execFile('/usr/bin/systemctl', ['--user', 'show', observation.unit, '--property=ControlGroup', '--value'], (error, stdout) => error ? reject(error) : resolve('/sys/fs/cgroup' + stdout.trim())));
}
async function empty(path: string) {
  try { expect(await readFile(join(path, 'cgroup.events'), 'utf8')).toContain('populated 0'); }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
}
it('records the upstream clearenv kernel-memory behavior using only a synthetic environment', async () => {
  if (process.platform !== 'linux') return;
  const { execFile } = await import('node:child_process');
  const { promisify } = await import('node:util');
  // Deliberately bypass the clean exec only in this harmless upstream probe.
  // No manager or Host environment enters this control process.
  const { stdout } = await promisify(execFile)('/usr/bin/bwrap', ['--unshare-all', '--unshare-user', '--disable-userns',
    '--die-with-parent', '--new-session', '--cap-drop', 'ALL', '--ro-bind', '/usr', '/usr',
    '--symlink', 'usr/lib', '/lib', '--symlink', 'usr/lib64', '/lib64', '--proc', '/proc', '--clearenv',
    '--', '/usr/bin/python3', '-c', `import os
secret=b'RUSTX_OFFICE_SECRET_SENTINEL=synthetic-upstream-probe'
assert 'RUSTX_OFFICE_SECRET_SENTINEL' not in os.environ
assert secret not in open('/proc/self/environ','rb').read()
print('retained' if secret in open('/proc/1/environ','rb').read() else 'erased')`],
  { env: { RUSTX_OFFICE_SECRET_SENTINEL: 'synthetic-upstream-probe' } });
  // Either upstream behavior is acceptable; the production regression below
  // must pass independently of whether the installed bwrap fixes issue #725.
  expect(['retained', 'erased']).toContain(stdout.trim());
  process.stdout.write(`Bubblewrap #725 probe: initial sentinel ${stdout.trim()}\n`);
});
it('manager secrets cannot survive the pre-bwrap exec in any kernel-visible sandbox environment', async () => {
  if (process.platform !== 'linux') return;
  const { execFile } = await import('node:child_process');
  const { promisify } = await import('node:util');
  const { randomUUID } = await import('node:crypto');
  const exec = promisify(execFile);
  // Unique synthetic keys avoid changing or printing any existing manager secret.
  const key = `RUSTX_OFFICE_SECRET_SENTINEL_${randomUUID().replaceAll('-', '_')}`;
  const secret = 'must-not-enter-office-sandbox';
  const connection = Object.fromEntries(['XDG_RUNTIME_DIR', 'DBUS_SESSION_BUS_ADDRESS'].flatMap(name => process.env[name] ? [[name, process.env[name]!]] : []));
  const service = async () => exec('/usr/bin/systemd-run', ['--user', '--quiet', '--wait', '--pipe', '--collect',
    '--', '/usr/bin/python3', '-c', `import os;print(int(os.environ.get('${key}')=='${secret}'))`], { env: connection });
  try {
    await exec('/usr/bin/systemctl', ['--user', 'set-environment', `${key}=${secret}`], { env: connection });
    // Positive control: the secret reaches a real user service, although the
    // systemd-run client's environment contains only connection inputs.
    expect((await service()).stdout.trim()).toBe('1');
    const output = await run(`import os,glob,json
expected={'PATH':'/usr/bin','HOME':'/tmp/home','LANG':'C.UTF-8','SAL_USE_VCLPLUGIN':'svp','TMPDIR':'/tmp','PWD':'/tmp'}
def environment(path):
 raw=open(path,'rb').read()
 assert b'${secret}' not in raw,'manager sentinel leaked'
 entries=dict(entry.split(b'=',1) for entry in raw.split(b'\\0') if entry)
 assert b'DBUS_SESSION_BUS_ADDRESS' not in entries,'manager bus leaked'
 assert b'XDG_RUNTIME_DIR' not in entries,'manager runtime leaked'
 return {key.decode():value.decode() for key,value in entries.items()}
assert environment('/proc/1/environ')=={},'bwrap initial environment was not empty'
assert environment('/proc/self/environ')==expected,'payload initial environment differs'
assert dict(os.environ)==expected,'payload effective environment differs'
# No trusted admission shell/env intermediary survives exec in this namespace.
for path in glob.glob('/proc/[0-9]*/environ'):
 try:
  actual=environment(path)
  assert all(key in expected and value==expected[key] for key,value in actual.items()),'unexpected inherited environment'
 except FileNotFoundError: pass
print('kernel environments clean')`);
    expect(output.toString().trim()).toBe('kernel environments clean');
  } finally {
    await exec('/usr/bin/systemctl', ['--user', 'unset-environment', key], { env: connection });
    // Await the manager operation and prove the next service no longer inherits it.
    expect((await service()).stdout.trim()).toBe('0');
  }
});
it('all writable paths share one 64 MiB tmpfs; root, dev and input cannot supply another filesystem', async () => {
  if (process.platform !== 'linux') return;
  const result = await run(`import os,errno,json
for directory in ['/','/dev','/etc','/input','/proc']:
 assert os.statvfs(directory).f_flag & os.ST_RDONLY,directory
for path in ['/escape','/dev/escape','/etc/escape','/input/escape']:
 try:
  open(path,'wb').write(b'x')
  raise AssertionError(path)
 except OSError as e:
  assert e.errno in [errno.EROFS,errno.EACCES],(path,e)
assert os.stat('/tmp').st_dev == os.stat('/dev/shm').st_dev
assert os.statvfs('/tmp').f_blocks * os.statvfs('/tmp').f_frsize == 67108864
written=0
try:
 for i in range(20):
  with open(('/tmp/' if i%2 else '/dev/shm/')+str(i),'wb',buffering=0) as f:
   for j in range(1024):
    written += f.write(b'x'*4096)
 raise AssertionError('unbounded writable storage')
except OSError as e:
 assert e.errno == errno.ENOSPC,e
assert written <= 67108864
print('storage bounded')`);
  expect(result.toString()).toContain('storage bounded');
});
it('fork proliferation hits the operation task controller, independently of UID limits', async () => {
  if (process.platform !== 'linux') return;
  const result = await run(`import os,errno,signal
r,w=os.pipe(); children=[]
try:
 for i in range(100):
  pid=os.fork()
  if pid==0:
   os.close(w);os.read(r,1);os._exit(0)
  children.append(pid)
 raise AssertionError('task limit absent')
except OSError as e:
 assert e.errno==errno.EAGAIN,e
 assert 1 < len(children) < 64,len(children)
finally:
 os.close(w)
 for pid in children: os.waitpid(pid,0)
print('tasks bounded')`);
  expect(result.toString()).toContain('tasks bounded');
});
it('aggregate memory charges multiple descendants and OOM kills the whole service', async () => {
  if (process.platform !== 'linux') return;
  const allocate = (count: number) => run(`import os
ready_r,ready_w=os.pipe();release_r,release_w=os.pipe()
for i in range(${count}):
 if os.fork()==0:
  os.close(ready_r);os.close(release_w)
  memory=bytearray(200*1024*1024)
  os.write(ready_w,b'x');os.read(release_r,1);os._exit(0)
os.close(ready_w)
ready=b''
while len(ready)<${count}:
 part=os.read(ready_r,${count}-len(ready))
 assert part,'allocation child failed'
 ready+=part
os.write(release_w,b'x'*${count})
for i in range(${count}):
 pid,status=os.wait();assert status==0
print('all allocations resident')`);
  expect((await allocate(1)).toString()).toContain('all allocations resident');
  // Each child requests the same legal 200 MiB allocation. Three concurrent
  // descendants exceed the operation's 512 MiB, rather than a per-child limit.
  await expect(allocate(3)).rejects.toThrow('converter_failure');
});
it.each(['cancel', 'external timeout', 'launcher failure'] as const)('%s settles only after the complete descendant cgroup is empty', async mode => {
  if (process.platform !== 'linux') return;
  if (mode === 'external timeout') vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  const entered = ready(), controller = new AbortController();
  const work = run(`import os
r,w=os.pipe();ready_r,ready_w=os.pipe()
if os.fork()==0:
 os.close(1);os.close(2)
 if os.fork()==0:
  os.write(ready_w,b'x');os.read(r,1)
 else:
  os.write(ready_w,b'x');os.wait()
else:
 ready=b''
 while len(ready)<2: ready+=os.read(ready_r,2-len(ready))
 print('PROBE-READY',flush=True);os.close(1);os.close(2)
 os.wait()`, controller.signal);
  const rejected = expect(work).rejects.toThrow(mode === 'cancel' ? 'obsolete' : 'converter_failure');
  await entered; const path = await group();
  expect((await readFile(join(path, 'cgroup.procs'), 'utf8')).trim().split('\n').length).toBeGreaterThan(1);
  if (mode === 'cancel') controller.abort();
  if (mode === 'launcher failure') observation.launcher!.kill('SIGKILL');
  await rejected; await empty(path);
  expect((await run("print('next operation')")).toString()).toContain('next operation');
}, 25000);
it('missing user manager never admits an unsandboxed payload', async () => {
  if (process.platform !== 'linux') return;
  vi.stubEnv('DBUS_SESSION_BUS_ADDRESS', 'unix:path=/nonexistent/rustx-document-bus');
  try { await expect(run("raise AssertionError('must never run')")).rejects.toThrow('converter_unavailable'); }
  finally { vi.unstubAllEnvs(); }
});
