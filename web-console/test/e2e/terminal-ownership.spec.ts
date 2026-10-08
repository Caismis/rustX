import { test, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startDogfood } from './dogfood-server';
import { AppServerRequestError } from '../../../tui/src/app-server/client';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { LocalWorkspaceHost } from '../../host/workspaces';
import { WorkspaceTerminals } from '../../host/workbench';
import { observeTerminalOwnership } from '../../host/terminal-ownership';

function gate() { let resolve!:()=>void,reject!:(error:Error)=>void; const promise=new Promise<void>((yes,no)=>{resolve=yes;reject=no;});return {promise,resolve,reject}; }

test('real native ABA commits and delayed wire retirements preserve successor PTYs', async () => {
  const fixture=await startDogfood('web_terminal_ownership');
  const native=await AppServerHost.connectRemote({endpoint:fixture.endpoint,token:fixture.token});
  const held:{session:string;through:string;deliver:()=>void}[]=[], observed=[gate(),gate()];
  let settlement:Promise<void>|undefined;
  const original=WorkspaceTerminals.prototype.retireOwnership;
  WorkspaceTerminals.prototype.retireOwnership=function(...args){return settlement=original.apply(this,args);};
  const host=new LocalWorkspaceHost({...JSON.parse(readFileSync(fixture.hostConfigFile,'utf8')),metadataFile:join(fixture.directory,'ownership-host.json'),nativeFilesystem:'shared',terminalSupervisor:fileURLToPath(new URL('../../../target/debug/interactive-supervisor',import.meta.url))},undefined,undefined,
    (endpoint,token,retired,lost)=>observeTerminalOwnership(endpoint,token,(session,through)=>{
      held.push({session,through,deliver:()=>retired(session,through)});observed[held.length-1]?.resolve();
    },lost));
  let passed=false;
  try {
    const created=await native.createSession({cwd:fixture.workspaceA});
    const session=created.session.id,root=created.session.active_node;
    const attached=await native.client.call('session/attach',{session_id:session},'attached');
    await native.client.call('session/subscribe',{target:attached.target,after_cursor:attached.cursor},'subscribed');
    const finished=gate();
    const unlisten=native.client.onNotification(note=>{if(note.method==='session/event'&&note.params.target.session_id===session&&note.params.event.type==='attempt_settled'){if(note.params.event.outcome.type==='completed')finished.resolve();else finished.reject(Error(JSON.stringify(note.params.event.outcome)));}});
    await native.client.call('turn/start',{target:attached.target,content:[{type:'text',text:'Ownership branch seed'}]},'inbound_accepted');
    await finished.promise;unlisten();
    const boundaries=await native.client.call('session/boundaries',{target:attached.target,offset:0,limit:32},'boundaries');
    const scope=await host.listWorkspaces();
    const summary=async()=>(await native.client.call('session/summary',{session_id:session},'session_summary')).summary;
    const start=async(node:string)=>{
      const target={session_id:session,active_node:node},id=randomUUID();
      await host.workbench(scope,{target,request:{kind:'create',id,shell:'/bin/sh'}});
      await host.workbench(scope,{target,request:{kind:'input',id,data:"printf 'PTY_%s_READY\\n' \"$$\"\r"}});
      let cursor=0,output='';
      for(;;){const result=await host.workbench(scope,{target,request:{kind:'poll',id,cursor}});if(!('cursor' in result))throw Error('Missing poll');cursor=result.cursor!;output+=result.output;const match=/PTY_(\d+)_READY/.exec(output);if(match)return {target,id,cursor,pid:Number(match[1])};}
    };
    const a1=await summary(),t1=await start(root);
    const branch=await native.branchSession(session,root,boundaries.surface_revision,boundaries.boundaries[0].message.id);
    await observed[0].promise;
    const b=await summary(),t2=await start(branch.session.active_node);
    expect(BigInt(b.ownership_generation)).toBeGreaterThan(BigInt(a1.ownership_generation));
    // Branch publication changes catalog ownership; switching retires the old resident runtime.
    await native.client.call('session/switchNode',{target:attached.target,node_id:branch.session.active_node},'session');
    expect((await summary()).ownership_generation).toBe(b.ownership_generation);
    const current=await native.client.call('session/attach',{session_id:session},'attached');
    await native.client.call('session/switchNode',{target:current.target,node_id:root},'session');
    const a2=await summary();
    expect(a2.active_node).toBe(a1.active_node);
    expect(BigInt(a2.ownership_generation)).toBeGreaterThan(BigInt(b.ownership_generation));
    await observed[1].promise;
    const t3=await start(root);
    expect(held.map(({session,through})=>({session,through}))).toEqual([{session,through:a1.ownership_generation},{session,through:b.ownership_generation}]);
    await expect(host.workbench(scope,{target:t3.target,request:{kind:'input',id:t1.id,data:'stale before delivery\r'}})).rejects.toThrow('belong');
    held[0].deliver();expect(settlement).toBeDefined();await settlement;
    expect(()=>process.kill(t1.pid,0)).toThrow();
    expect(()=>process.kill(t2.pid,0)).not.toThrow();
    expect(()=>process.kill(t3.pid,0)).not.toThrow();
    held[1].deliver();await settlement;
    expect(()=>process.kill(t2.pid,0)).toThrow();
    expect(()=>process.kill(t3.pid,0)).not.toThrow();
    await host.workbench(scope,{target:t3.target,request:{kind:'input',id:t3.id,data:"printf 'A2_%s\\n' FUNCTIONAL\r"}});
    let cursor=t3.cursor,output='';
    while(!output.includes('A2_FUNCTIONAL')){const result=await host.workbench(scope,{target:t3.target,request:{kind:'poll',id:t3.id,cursor}});if(!('cursor' in result)||result.exited)throw Error('Successor ended');cursor=result.cursor!;output+=result.output;}
    await expect(host.workbench(scope,{target:t3.target,request:{kind:'input',id:t1.id,data:'stale\r'}})).rejects.toThrow('belong');
    expect((await fixture.control('requests')).requests).toHaveLength(1);
    await host.close();expect(()=>process.kill(t3.pid,0)).toThrow();passed=true;
  } catch(error) {
    if(error instanceof AppServerRequestError)throw new Error(`${error.method}: ${JSON.stringify(error.error)}`,{cause:error});
    throw error;
  } finally {
    await host.close();WorkspaceTerminals.prototype.retireOwnership=original;
    await native.shutdown();await fixture.stop(passed);
  }
});
