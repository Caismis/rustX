// @vitest-environment node
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { it, expect, vi } from 'vitest';
const launch=vi.hoisted(()=>({spawn:vi.fn()}));
vi.mock('node:child_process',()=>({spawn:launch.spawn}));
import { runMacOfficeSandbox } from '../host/documents/office-macos';
it('does not signal a retired process group again when cancellation settles',async()=>{
  const directory=await mkdtemp(join(tmpdir(),'rustx-mac-lifecycle-'));
  const child=Object.assign(new EventEmitter(),{pid:987654,stdout:new PassThrough(),stderr:new PassThrough()});
  let spawned!:()=>void;
  const ready=new Promise<void>(resolve=>{spawned=resolve});
  launch.spawn.mockImplementation(()=>{spawned();return child});
  const kill=vi.spyOn(process,'kill').mockReturnValueOnce(true).mockImplementation(()=>{throw Object.assign(new Error('retired group'),{code:'EPERM'})});
  try {
    const controller=new AbortController();
    const work=runMacOfficeSandbox(directory,['node'],[],controller.signal);
    const result=expect(work).rejects.toThrow('obsolete');
    await ready;controller.abort();
    expect(kill).toHaveBeenCalledWith(-987654,'SIGKILL');
    child.emit('close',null,'SIGKILL');
    await result;
    expect(kill).toHaveBeenCalledTimes(1);
  } finally {kill.mockRestore();await rm(directory,{recursive:true,force:true})}
});
