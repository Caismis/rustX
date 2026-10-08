// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, symlinkSync, realpathSync, renameSync, openSync, closeSync, constants, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { workspaceFile, workspaceDescriptors, WorkspaceTerminals } from '../host/workbench';
import { LocalWorkspaceHost } from '../host/workspaces';
const directories: string[] = [];
const services: WorkspaceTerminals[] = [];
afterEach(async () => { vi.unstubAllEnvs(); await Promise.all(services.splice(0).map(service => service.close())); directories.splice(0).forEach(path => rmSync(path, { recursive: true, force: true })); });
function directory() { const path = realpathSync(mkdtempSync(join(tmpdir(), 'rustx-workbench-'))); directories.push(path); return path; }
it('lists and reads relative files, rejects traversal, symlinks, binary and oversized content', () => {
  const root = directory(), outside = directory(); mkdirSync(join(root, 'nested')); writeFileSync(join(root, 'nested/a.txt'), 'hello'); writeFileSync(join(outside, 'secret'), 'outside'); symlinkSync(outside, join(root, 'escape'));
  expect(workspaceFile(root, '', false).entries?.map(row => row.name)).toContain('nested');
  expect(workspaceFile(root, 'nested/a.txt', true)).toEqual({ cwd: root, text: 'hello' });
  expect(() => workspaceFile(root, '../secret', true)).toThrow();
  expect(() => workspaceFile(root, 'escape/secret', true)).toThrow();
  expect(() => workspaceFile(root, 'escape', false)).toThrow();
  writeFileSync(join(root, 'binary'), Buffer.from([0])); expect(() => workspaceFile(root, 'binary', true)).toThrow();
  writeFileSync(join(root, 'large'), Buffer.alloc(1024 * 1024 + 1)); expect(() => workspaceFile(root, 'large', true)).toThrow();
});
it('a real PTY uses the requested cwd, resizes, reattaches output and rejects foreign ownership', async () => {
  vi.stubEnv('RUSTX_WORKBENCH_SENTINEL', 'private-host-value');
  const service = new WorkspaceTerminals(fileURLToPath(new URL('../../target/debug/interactive-supervisor', import.meta.url))); services.push(service); const root = directory(), id = randomUUID();
  const create = { kind: 'create' as const, id, shell: '/bin/sh' };
  expect((await service.request('A', root, create)).terminals).toHaveLength(1);
  expect((await service.request('A', root, create)).terminals).toHaveLength(1);
  await expect(service.request('B', root, { kind: 'input', id, data: 'exit\r' })).rejects.toThrow('belong');
  await service.request('A', root, { kind: 'resize', id, cols: 93, rows: 31 });
  await service.request('A', root, { kind: 'input', id, data: "pwd; stty size; echo HOST_ENV_${RUSTX_WORKBENCH_SENTINEL-unset}; printf 'WORKBENCH_%s\\n' READY; exit\r" });
  let cursor = 0, output = '';
  for (;;) { const result = await service.request('A', root, { kind: 'poll', id, cursor }); cursor = result.cursor!; output += result.output; if (result.exited) break; }
  expect(output).toContain('HOST_ENV_unset'); expect(output).not.toContain('private-host-value'); expect(output).toContain(root); expect(output).toContain('31 93'); expect(output).toContain('WORKBENCH_READY');
  expect((await service.request('A', root, { kind: 'poll', id, cursor: 0 })).output).toBe(output);
  await service.request('A', root, { kind: 'close', id }); expect((await service.request('A', root, { kind: 'terminals' })).terminals).toEqual([]);
});
it('Host rereads the exact native target and rejects a replaced authority before browsing', async () => {
  const root = directory(); writeFileSync(join(root, 'a.txt'), 'A'); let calls = 0;
  const host = new LocalWorkspaceHost({ roots: [{ id: 'root', cwd: root, displayName: 'Root' }], picker: false, metadataFile: join(root, 'metadata.json'), nativeFilesystem: 'shared', transportToken: 'test', endpoint: 'ws://localhost:8080' }, undefined, async (_endpoint, _token, target) => { expect(target).toEqual({ session_id: 'A', active_node: 'node-A' }); calls++; return root; });
  try {
    const scope = await host.listWorkspaces(); const call = { target: { session_id: 'A', active_node: 'node-A' }, request: { kind: 'read' as const, path: 'a.txt' } };
    expect(await host.workbench(scope, call)).toEqual({ cwd: root, text: 'A' }); expect(calls).toBe(1);
    await expect(host.workbench({ ...scope, authorityId: 'retired' }, call)).rejects.toThrow(); expect(calls).toBe(1);
  } finally { await host.close(); }
});

it('held directory descriptors survive path replacement without following its replacement symlink', () => {
  const root = directory(), outside = directory();
  mkdirSync(join(root, 'nested')); writeFileSync(join(root, 'nested/a.txt'), 'inside'); writeFileSync(join(outside, 'a.txt'), 'outside');
  const parent = openSync(root, constants.O_RDONLY | constants.O_DIRECTORY);
  const held = workspaceDescriptors.openChild(parent, 'nested', true);
  try {
    renameSync(join(root, 'nested'), join(root, 'retained')); symlinkSync(outside, join(root, 'nested'));
    expect(() => workspaceDescriptors.openChild(parent, 'nested', true)).toThrow();
    expect(workspaceDescriptors.entries(held).map(entry => entry.name.toString())).toEqual(['a.txt']);
    const file = workspaceDescriptors.openChild(held, 'a.txt', false);
    try { expect(readFileSync(file, 'utf8')).toBe('inside'); } finally { closeSync(file); }
    for (const name of ['', '..', '.', 'a/b', 'a\0b']) expect(() => workspaceDescriptors.openChild(held, name, false)).toThrow();
  } finally { closeSync(held); closeSync(parent); }
});
it('bounds native directory iteration and preserves UTF-8 filename identity', () => {
  const root = directory(), name = '\ufeff文件.txt'; writeFileSync(join(root, name), 'unicode');
  expect(workspaceFile(root, '', false).entries?.[0].name).toBe(name);
  expect(workspaceFile(root, name, true).text).toBe('unicode');
  for (let i = 0; i < 2000; i++) writeFileSync(join(root, String(i)), '');
  expect(() => workspaceFile(root, '', false)).toThrow('2000');
});

it.each(['linux', 'darwin'] as const)('opens an admitted file on %s with literal argv and the Session directory as cwd', async platform => {
  const { DesktopAdapter } = await import('../host/desktop');
  const root = directory(), outside = directory(), name = '-汉字 "quote";$(echo x).py';
  writeFileSync(join(root, name), 'print(1)'); symlinkSync(outside, join(root, 'escape'));
  const launch = vi.fn(async () => ({ status: 'spawned' as const }));
  const adapter = new DesktopAdapter({ platform, env: { PATH: '/bin', DISPLAY: ':0' }, macOSDesktop: () => true, executable: path => path, launch });
  const host = new LocalWorkspaceHost({ roots: [{ id: 'root', cwd: root, displayName: 'Root' }], picker: false, metadataFile: join(root, 'metadata.json'), nativeFilesystem: 'shared', transportToken: 'test', endpoint: 'ws://localhost:8080' }, adapter, async (_e, _t, target) => { if (target.active_node !== 'node-A') throw new Error('Retired node'); return root; });
  try {
    const scope = await host.listWorkspaces(), target = { session_id: 'A', active_node: 'node-A' };
    await host.workbench(scope, { target, request: { kind: 'open', path: name, directory: false, application: 'code' } });
    expect(launch).toHaveBeenCalledWith(expect.objectContaining({ cwd: root, args: ['--new-window', '--', join(root, name)] }));
    await host.workbench(scope, { target, request: { kind: 'open', path: name, directory: false, application: 'files' } });
    expect(launch).toHaveBeenLastCalledWith(expect.objectContaining({ cwd: root, args: platform === 'darwin' ? ['-R', '--', join(root, name)] : [root] }));
    launch.mockClear();
    for (const path of ['../outside', '/etc/passwd', 'escape/file', 'missing']) await expect(host.workbench(scope, { target, request: { kind: 'open', path, directory: false, application: 'code' } })).rejects.toThrow();
    await expect(host.workbench(scope, { target: { ...target, active_node: 'retired' }, request: { kind: 'open', path: name, directory: false, application: 'code' } })).rejects.toThrow('Retired');
    await expect(host.workbench({ ...scope, authorityId: 'retired' }, { target, request: { kind: 'open', path: name, directory: false, application: 'files' } })).rejects.toThrow();
    expect(launch).not.toHaveBeenCalled();
  } finally { await host.close(); }
});
it('binary previews retain descriptor admission and enforce a bounded byte read', async () => {
  const root=directory(),outside=directory(),bytes=Buffer.from([0,255,1,2]);
  writeFileSync(join(root,'image.png'),bytes);
  expect(workspaceFile(root,'image.png',true,true)).toEqual({cwd:root,base64:bytes.toString('base64')});
  writeFileSync(join(outside,'private'),bytes);symlinkSync(outside,join(root,'escape'));
  expect(()=>workspaceFile(root,'escape/private',true,true)).toThrow();
  expect(()=>workspaceFile(root,'../private',true,true)).toThrow();
  writeFileSync(join(root,'large'),Buffer.alloc(16*1024*1024+1));
  expect(()=>workspaceFile(root,'large',true,true)).toThrow('16 MiB');
  const service=new WorkspaceTerminals(fileURLToPath(new URL('../../target/debug/interactive-supervisor', import.meta.url)));services.push(service);
  expect(await service.request('owner',root,{kind:'bytes',path:'image.png'})).toEqual({cwd:root,base64:bytes.toString('base64')});
});

it('resolves file references within the admitted native workspace without following symlinks', async () => {
  const root = directory(), outside = directory();
  mkdirSync(join(root, 'docs')); writeFileSync(join(root, 'docs/モルガン 解説.md'), '# document');
  writeFileSync(join(outside, 'secret'), 'private'); symlinkSync(outside, join(root, 'escape'));
  const host = new LocalWorkspaceHost({ roots: [{ id: 'root', cwd: root, displayName: 'Root' }], picker: false, metadataFile: join(root, 'metadata.json'), nativeFilesystem: 'shared', transportToken: 'test', endpoint: 'ws://localhost:8080' }, undefined, async () => root);
  try {
    const scope = await host.listWorkspaces(), target = { session_id: 'A', active_node: 'node-A' };
    for (const path of ['docs/モルガン 解説.md', './docs/モルガン 解説.md', join(root, 'docs/モルガン 解説.md')])
      expect(await host.workbench(scope, { target, request: { kind: 'resolve', path } })).toEqual({ path: 'docs/モルガン 解説.md' });
    for (const path of [join(outside, 'secret'), '../secret', 'escape/secret', 'missing.md', 'docs', 'a\0b'])
      await expect(host.workbench(scope, { target, request: { kind: 'resolve', path } })).rejects.toThrow();
    await expect(host.workbench({ ...scope, authorityId: 'retired' }, { target, request: { kind: 'resolve', path: 'docs/モルガン 解説.md' } })).rejects.toThrow();
  } finally { await host.close(); }
});

it('successful close proves subprocesses reaped, not just the PTY leader killed', async () => {
  const service = new WorkspaceTerminals(fileURLToPath(new URL('../../target/debug/interactive-supervisor', import.meta.url)));
  services.push(service); const root=directory(),id=randomUUID();
  await service.request('A',root,{kind:'create',id,shell:'/bin/sh'});
  await service.request('A',root,{kind:'input',id,data:"sh -c 'trap \"\" TERM; printf \"CHILD_%s_READY\\n\" \"$$\"; read value'\r"});
  let cursor=0,output='',pid:number|undefined;
  while(pid===undefined){const result=await service.request('A',root,{kind:'poll',id,cursor});cursor=result.cursor!;output+=result.output;const match=/CHILD_(\d+)_READY/.exec(output);if(match)pid=Number(match[1]);if(result.exited)throw new Error('Child did not reach its input barrier');}
  await service.request('A',root,{kind:'close',id});
  expect(()=>process.kill(pid!,0)).toThrow();
});
it('PTY interrupt reaches managed foreground work while the supervisor and shell stay owned', async () => {
  const service = new WorkspaceTerminals(fileURLToPath(new URL('../../target/debug/interactive-supervisor', import.meta.url)));
  services.push(service); const root=directory(),id=randomUUID();
  await service.request('A',root,{kind:'create',id,shell:'/bin/sh'});
  await service.request('A',root,{kind:'input',id,data:"trap 'printf \"SHELL_%s\\n\" INTERRUPTED' INT; sh -c 'printf \"INTERRUPT_%s\\n\" READY; read value'\r"});
  let cursor=0,output='';
  while(!output.includes('INTERRUPT_READY')){const result=await service.request('A',root,{kind:'poll',id,cursor});cursor=result.cursor!;output+=result.output;if(result.exited)throw new Error('Terminal ended before interrupt');}
  await service.request('A',root,{kind:'input',id,data:'\x03'});
  while(!output.includes('SHELL_INTERRUPTED')){const result=await service.request('A',root,{kind:'poll',id,cursor});cursor=result.cursor!;output+=result.output;if(result.exited)throw new Error('Interrupt destroyed terminal ownership');}
  await service.request('A',root,{kind:'input',id,data:"printf 'AFTER_%s\\n' INTERRUPT\r"});
  while(!output.includes('AFTER_INTERRUPT')){const result=await service.request('A',root,{kind:'poll',id,cursor});cursor=result.cursor!;output+=result.output;if(result.exited)throw new Error('Interrupt destroyed terminal ownership');}
  await service.request('A',root,{kind:'close',id});
});

it.each(['Session deletion', 'active-node retirement'])('%s settles the owned real PTY through the native ownership event', async reason => {
  const root=directory(),id=randomUUID(),target={session_id:'A',active_node:'node-A'};
  let retire!: (session:string)=>void, current=true;
  const retired=vi.spyOn(WorkspaceTerminals.prototype,'retireSession');
  const host=new LocalWorkspaceHost({roots:[{id:'root',cwd:root,displayName:'Root'}],picker:false,metadataFile:join(root,'metadata.json'),nativeFilesystem:'shared',transportToken:'test',endpoint:'ws://localhost:8080',terminalSupervisor:fileURLToPath(new URL('../../target/debug/interactive-supervisor',import.meta.url))},undefined,async()=>{if(!current)throw new Error(reason);return root;},async(_endpoint,_token,onRetired)=>{retire=onRetired;return async()=>{};});
  try {
    const scope=await host.listWorkspaces();
    await host.workbench(scope,{target,request:{kind:'create',id,shell:'/bin/sh'}});
    await host.workbench(scope,{target,request:{kind:'input',id,data:"printf 'OWNED_%s_READY\\n' \"$$\"\r"}});
    let cursor=0,output='',pid:number|undefined;
    while(pid===undefined){const result=await host.workbench(scope,{target,request:{kind:'poll',id,cursor}});if(!('cursor' in result))throw new Error('Missing terminal poll result');cursor=result.cursor!;output+=result.output;const match=/OWNED_(\d+)_READY/.exec(output);if(match)pid=Number(match[1]);}
    current=false;retire('A');expect(retired).toHaveBeenCalledWith('A');
    await retired.mock.results.at(-1)!.value;
    expect(()=>process.kill(pid!,0)).toThrow();
    await expect(host.workbench(scope,{target,request:{kind:'input',id,data:'echo stale\r'}})).rejects.toThrow(reason);
    await host.workbench(scope,{target,request:{kind:'close',id}});
  } finally {retired.mockRestore();await host.close();}
});
