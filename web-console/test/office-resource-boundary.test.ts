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
