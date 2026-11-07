/** Linux process accounting shared by the App Server measurement scenarios. */
import {readFileSync,readdirSync} from 'node:fs';
import {once} from 'node:events';
import {createServer,connect,type Socket} from 'node:net';
export function counters(pid:number) {
  const stat=readFileSync(`/proc/${pid}/stat`,'utf8');
  const fields=stat.slice(stat.lastIndexOf(')')+2).trim().split(/\s+/);
  const io=Object.fromEntries(readFileSync(`/proc/${pid}/io`,'utf8').trim().split('\n').map(line=>{const[k,v]=line.split(':');return[k,Number(v.trim())];}));
  const status=readFileSync(`/proc/${pid}/status`,'utf8');
  return {cpuTicks:Number(fields[11])+Number(fields[12]),childCpuTicks:Number(fields[13])+Number(fields[14]),writeCalls:io.syscw,writtenBytes:io.write_bytes,writtenCharacters:io.wchar,
    residentKiB:Number(status.match(/^VmRSS:\s+(\d+)/m)![1]),peakResidentKiB:Number(status.match(/^VmHWM:\s+(\d+)/m)![1]),descriptors:readdirSync(`/proc/${pid}/fd`).length};
}
export function processTree(pid:number):number[] {
  const result:number[]=[],pending=[pid],seen=new Set<number>();
  while(pending.length) {
    const current=pending.pop()!;if(seen.has(current))continue;seen.add(current);result.push(current);
    let tasks:string[];try{tasks=readdirSync(`/proc/${current}/task`);}catch{continue;}
    for(const task of tasks)try{pending.push(...readFileSync(`/proc/${current}/task/${task}/children`,'utf8').trim().split(/\s+/).filter(Boolean).map(Number));}catch{}
  }
  return result;
}
export async function slowProxy(endpoint:string) {
  const url=new URL(endpoint),links:Array<{client:Socket;server:Socket}>=[];
  const listener=createServer(client=>{
    const server=connect(Number(url.port),url.hostname);client.pipe(server);server.pipe(client);
    client.on('error',()=>server.destroy());server.on('error',()=>client.destroy());
    client.on('close',()=>server.destroy());server.on('close',()=>client.destroy());links.push({client,server});
  });
  listener.listen(0,'127.0.0.1');await once(listener,'listening');
  const address=listener.address();if(!address||typeof address==='string')throw Error('Missing proxy listener');
  return {endpoint:`ws://127.0.0.1:${address.port}/`,pause:()=>{for(const link of links){link.server.unpipe(link.client);link.server.pause();}},
    resume:()=>{for(const link of links){link.server.pipe(link.client);link.server.resume();}},close:async()=>{for(const link of links){link.client.destroy();link.server.destroy();}await new Promise<void>(resolve=>listener.close(()=>resolve()));}};
}
