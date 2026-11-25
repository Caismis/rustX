/** Actual installed-Chrome source/read/decode benchmark. No input is preloaded
 * in the browser. All pages use production persistent workspace descriptors.
 * node --experimental-transform-types web-console/scripts/geometry-stream-browser-benchmark.ts --browser-ws <Playwright WS> --output <JSON> [--smoke]
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { createServer, build } from 'vite';
import { createServer as createHttpServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { chromium } from '@playwright/test';
import { openWorkspaceBytePreview, readWorkspacePreview } from '../host/workspace-preview.ts';
import type { FilePreviewOperation } from '../shared/file-preview.ts';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const tracked=['src/client/geometry-source.ts','src/client/session-files.ts','src/app/components/documents/geometry/parse.ts','src/app/components/documents/geometry/stl-stream.ts','src/app/components/documents/geometry/stl-worker.ts','host/workspace-preview.ts','host/workbench.ts','host/workspace-fs/build/Release/workspace_fs.node','scripts/geometry-stream-browser-benchmark.ts'];
const hashes=Object.fromEntries(tracked.map(name=>[name,createHash('sha256').update(fs.readFileSync(path.join(root,name))).digest('hex')]));
const option = (name: string) => process.argv[process.argv.indexOf(name) + 1];
if (!process.argv.includes('--browser-ws') || !process.argv.includes('--output')) throw new Error('--browser-ws and --output are required');
const asciiSmoke=process.argv.includes('--ascii-smoke');
const dev = process.argv.includes('--dev'),defaultPolicy=process.argv.includes('--default-policy');
const smoke = process.argv.includes('--smoke'), competitionOnly = process.argv.includes('--competition-only');
const pairs = process.argv.includes('--pairs') ? Number(option('--pairs')) : smoke||asciiSmoke ? 1 : 5;
if (!Number.isInteger(pairs) || pairs < 1 || pairs > 100) throw new Error('--pairs must be an integer from 1 to 100');
const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'rustx-stream-'));
const prepareOnly=process.argv.includes('--prepare-only'),reuseBundle=process.argv.includes('--reuse-bundle');
if(prepareOnly&&!process.argv.includes('--bundle-dir'))throw new Error('--prepare-only requires --bundle-dir');
const sizes = prepareOnly?[]:asciiSmoke?[32]:smoke ? [8] : competitionOnly ? [32] : [32, 200], files = new Map<number, string>(), fixtures: {sizeMiB:number;bytes:number;triangles:number;sha256:string;format:'ascii'|'binary';names:string[];utf8BoundaryOffset?:number}[] = [];
for (const size of sizes) {
  if(asciiSmoke){
    const names=['front plate 😀','rear plate 😀'],target=size*1024*1024,prefix='solid front plate ',utf8BoundaryOffset=2*1024*1024-1;
    const leading=Buffer.from(' '.repeat(utf8BoundaryOffset-Buffer.byteLength(prefix))+prefix+'😀\n');
    const boundary=Buffer.from('endsolid front plate 😀\nsolid rear plate 😀\n'),ending=Buffer.from('endsolid rear plate 😀\n');
    const triangle=Buffer.from('facet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\n');
    const triangles=Math.floor((target-leading.length-boundary.length-ending.length)/triangle.length),first=Math.floor(triangles/2),name=`${size}-ascii.stl`,fd=fs.openSync(path.join(directory,name),'w'),digest=createHash('sha256');let written=0;
    const write=(chunk:Buffer)=>{fs.writeSync(fd,chunk);digest.update(chunk);written+=chunk.length;};
    const block=Buffer.concat(Array.from({length:1024},()=>triangle));
    const faces=(count:number)=>{for(let left=count;left>0;left-=1024)write(block.subarray(0,Math.min(left,1024)*triangle.length));};
    write(leading);faces(first);write(boundary);faces(triangles-first);write(ending);write(Buffer.from(' '.repeat(target-written)));fs.closeSync(fd);
    fixtures.push({sizeMiB:size,bytes:written,triangles,sha256:digest.digest('hex'),format:'ascii',names,utf8BoundaryOffset});files.set(size,name);continue;
  }
  const triangles = Math.floor((size * 1024 * 1024 - 84) / 50), name = `${size}.stl`, fd = fs.openSync(path.join(directory, name), 'w');
  const digest = createHash('sha256');
  const header = Buffer.alloc(84); header.writeUInt32LE(triangles, 80); fs.writeSync(fd, header); digest.update(header);
  const block = Buffer.alloc(50 * 65536);
  for (let face = 0; face < 65536; face++) {
    const offset = face * 50; block.writeFloatLE(1, offset + 8);
    block.writeFloatLE(face % 256, offset + 12); block.writeFloatLE(Math.floor(face / 256), offset + 16);
    block.writeFloatLE(face % 256 + 1, offset + 24); block.writeFloatLE(Math.floor(face / 256), offset + 28);
    block.writeFloatLE(face % 256, offset + 36); block.writeFloatLE(Math.floor(face / 256) + 1, offset + 40);
  }
  for (let left = triangles; left > 0; left -= 65536) { const chunk=block.subarray(0, Math.min(left,65536)*50); fs.writeSync(fd,chunk);digest.update(chunk); }
  fixtures.push({sizeMiB:size,bytes:84+triangles*50,triangles,sha256:digest.digest('hex'),format:'binary',names:['']});
  fs.closeSync(fd); files.set(size, name);
}
const artifactDirectory=process.argv.includes('--bundle-dir')?path.resolve(option('--bundle-dir')):path.join(directory,'bundle');
const artifacts:Record<string,string>={};
if(!dev){
  if(!reuseBundle){
  const entry='virtual:geometry-stream-benchmark';
  await build({root,configFile:false,logLevel:'warn',plugins:[{name:'geometry-stream-benchmark-entry',resolveId:id=>id===entry?'\0'+entry:undefined,load:id=>id==='\0'+entry?`export {readGeometrySource} from ${JSON.stringify(path.join(root,'src/client/geometry-source.ts'))};export {parseGeometryStream} from ${JSON.stringify(path.join(root,'src/app/components/documents/geometry/parse.ts'))};export {PreviewDemand} from ${JSON.stringify(path.join(root,'src/client/session-files.ts'))};`:undefined}],build:{outDir:artifactDirectory,emptyOutDir:true,minify:true,target:'es2022',rollupOptions:{preserveEntrySignatures:'strict',input:entry,output:{entryFileNames:'benchmark.js'}}}});
  }
  const walk=(directory:string)=>{for(const entry of fs.readdirSync(directory,{withFileTypes:true})){const filename=path.join(directory,entry.name);if(entry.isDirectory())walk(filename);else if(entry.name!=='benchmark-manifest.json')artifacts[path.relative(artifactDirectory,filename)]=createHash('sha256').update(fs.readFileSync(filename)).digest('hex');}};walk(artifactDirectory);
  const manifest=path.join(artifactDirectory,'benchmark-manifest.json');
  if(reuseBundle){const recorded=JSON.parse(fs.readFileSync(manifest,'utf8'));if(JSON.stringify(recorded.hashes)!==JSON.stringify(hashes)||JSON.stringify(recorded.artifacts)!==JSON.stringify(artifacts))throw new Error('Prepared artifact/source fingerprint mismatch');}
  else fs.writeFileSync(manifest,JSON.stringify({hashes,artifacts},null,2)+'\n');
}
if(prepareOnly){process.stdout.write(JSON.stringify({artifactDirectory,hashes,artifacts},null,2)+'\n');fs.rmSync(directory,{recursive:true,force:true});process.exit(0);}
const operations = new Map<number, FilePreviewOperation>(); let sequence = 0;
const route=async (request:IncomingMessage,response:ServerResponse,next:()=>void) => {
  const url = new URL(request.url ?? '/', 'http://localhost');
  if (!url.pathname.startsWith('/__stream/')) { next(); return; }
  try {
    const size = Number(url.searchParams.get('size')), name = files.get(size);
    if (url.pathname === '/__stream/open') {
      if (!name) throw new Error('Invalid fixture');
      const operation = openWorkspaceBytePreview(directory, name, new AbortController().signal), id = ++sequence;
      operations.set(id, operation); response.setHeader('content-type', 'application/json'); response.end(JSON.stringify({ id, bytes: operation.bytes, version: operation.version })); return;
    }
    if (url.pathname === '/__stream/download') {
      if (!name || size > 32) throw new Error('Invalid download fixture');
      const page = await readWorkspacePreview(directory, name, {kind:'bytes'}, new AbortController().signal);
      response.end(Buffer.from(page.data.buffer, page.data.byteOffset, page.data.byteLength)); return;
    }
    const id = Number(url.searchParams.get('id')), operation = operations.get(id); if (!operation) throw new Error('Missing operation');
    if (url.pathname === '/__stream/close') { operations.delete(id); await operation.close(); response.end('{}'); return; }
    if (url.pathname !== '/__stream/read') throw new Error('Invalid command');
    const delay = Number(url.searchParams.get('delay')); if (delay) await new Promise(resolve => setTimeout(resolve, delay));
    const page = await operation.read({ offset: Number(url.searchParams.get('offset')), length: Number(url.searchParams.get('length')) });
    response.setHeader('x-version', page.version); response.setHeader('x-bytes', page.bytes); response.setHeader('x-offset', page.offset); response.setHeader('x-eof', String(page.eof));
    response.end(Buffer.from(page.data.buffer, page.data.byteOffset, page.data.byteLength));
  } catch (error) { response.statusCode = 500; response.end(String(error)); }
};
const viteServer=dev?await createServer({root,configFile:false,appType:'custom',optimizeDeps:{noDiscovery:true,include:[]},server:{host:'127.0.0.1',port:0,fs:{allow:[root]}}}):undefined;
let staticServer:ReturnType<typeof createHttpServer>|undefined;
if(viteServer){viteServer.middlewares.use(route);await viteServer.listen();}
else{staticServer=createHttpServer((request,response)=>{void route(request,response,()=>{
  const url=new URL(request.url??'/','http://localhost'),filename=path.resolve(artifactDirectory,'.'+url.pathname);
  if(url.pathname==='/'){response.setHeader('content-type','text/html');response.end('<!doctype html><title>Geometry source benchmark</title>');return;}
  if(!filename.startsWith(artifactDirectory+path.sep)||!fs.existsSync(filename)||!fs.statSync(filename).isFile()){response.statusCode=404;response.end('Not found');return;}
  response.setHeader('content-type',filename.endsWith('.wasm')?'application/wasm':filename.endsWith('.js')?'application/javascript':'application/octet-stream');fs.createReadStream(filename).pipe(response);
});});await new Promise<void>(resolve=>staticServer!.listen(0,'127.0.0.1',resolve));}
const address = (viteServer?.httpServer??staticServer)!.address(); if (!address || typeof address === 'string') throw new Error('Missing server address');
const browser = await chromium.connect(option('--browser-ws')), context = await browser.newContext(), page = await context.newPage();
try {
  const networkRequests:string[]=[];page.on('request',request=>networkRequests.push(request.url()));
  await page.goto(`http://127.0.0.1:${address.port}/`);
  const run = async (policy: string, size: number, competing: boolean, delay: number) => page.evaluate(async ({policy,size,competing,delay,dev,fixture}) => {
    const geometryPath = '/src/client/geometry-source.ts', parserPath = '/src/app/components/documents/geometry/parse.ts', demandPath = '/src/client/session-files.ts';
    const bundle=dev?undefined:await import('/benchmark.js' as string);
    const geometry: typeof import('../src/client/geometry-source') = bundle??await import(geometryPath), parser: typeof import('../src/app/components/documents/geometry/parse') = bundle??await import(parserPath), { PreviewDemand }: typeof import('../src/client/session-files') = bundle??await import(demandPath);
    const demand = new PreviewDemand(2, 8, 'unavailable', () => false), origin = performance.now(), requests: {source:string;kind:string;offset:number;submitted:number;admitted:number;ended:number;bytes:number}[] = [], chunks: {source:string;offset:number;start:number;end:number;bytes:number;transferred:boolean}[] = [];
    const verification: {source:string;geometry:import('three').BufferGeometry}[]=[];
    const scheduling: (import('../src/client/geometry-source').GeometryReadObservation & {source:string})[]=[];
    let active = 0, maximum = 0, downloadSubmitted = 0, downloadEnded = 0, copiedBySlice = 0;
    const originalSlice = Uint8Array.prototype.slice;
    Uint8Array.prototype.slice = function(start?:number,end?:number) { const result = originalSlice.call(this,start,end); copiedBySlice += result.byteLength; return result; };
    const checkedFetch = async (url:string) => {const response=await fetch(url);if(!response.ok)throw new Error(await response.text());return response;};
    const open = (source:string) => async (signal:AbortSignal) => {
      const opened={source,kind:'open',offset:0,submitted:performance.now()-origin,admitted:0,ended:0,bytes:0};requests.push(opened);
      const metadata = await demand.run(signal,async()=>{opened.admitted=performance.now()-origin;active++;maximum=Math.max(maximum,active);try{return await(await checkedFetch(`/__stream/open?size=${size}`)).json();}finally{active--;opened.ended=performance.now()-origin;}});
      return {...metadata, read: (range:{offset:number;length:number}) => {
        const request={source,kind:'read',offset:range.offset,submitted:performance.now()-origin,admitted:0,ended:0,bytes:0};requests.push(request);
        return demand.run(signal,async()=>{request.admitted=performance.now()-origin;active++;maximum=Math.max(maximum,active);try{
          const response=await checkedFetch(`/__stream/read?id=${metadata.id}&offset=${range.offset}&length=${range.length}&delay=${delay}`),data=new Uint8Array(await response.arrayBuffer());
          request.bytes=data.length;return {data,version:response.headers.get('x-version')!,bytes:Number(response.headers.get('x-bytes')),offset:Number(response.headers.get('x-offset')),eof:response.headers.get('x-eof')==='true'};
        }finally{active--;request.ended=performance.now()-origin;}});
      }, close: async()=>{const closed={source,kind:'close',offset:0,submitted:performance.now()-origin,admitted:0,ended:0,bytes:0};requests.push(closed);await demand.run(new AbortController().signal,async()=>{closed.admitted=performance.now()-origin;active++;maximum=Math.max(maximum,active);try{await checkedFetch(`/__stream/close?id=${metadata.id}`);}finally{active--;closed.ended=performance.now()-origin;}});} };
    };
    const preview = async (source:string) => {
      const controller=new AbortController();
      const prepared=await parser.parseGeometryStream({consume:(sink:import('../shared/geometry-source').GeometrySourceConsumer,signal:AbortSignal)=>geometry.readGeometrySource(open(source),signal,{start:metadata=>sink.start(metadata),chunk:async(data,offset)=>{
        const entry={source,offset,start:performance.now()-origin,end:0,bytes:data.length,transferred:false};chunks.push(entry);await sink.chunk(data,offset);entry.end=performance.now()-origin;entry.transferred=data.byteLength===0;
      }},policy==='default'?undefined:policy as import('../src/client/geometry-source').GeometryReadPolicy,sample=>scheduling.push({source,...sample}))},controller.signal);
      const ended=performance.now()-origin;let vertices=0,normals=0,triangles=0;const names:string[]=[];prepared.scene.traverse(object=>{if('geometry' in object){const mesh=object as import('three').Mesh;verification.push({source,geometry:mesh.geometry});names.push(mesh.name);vertices+=mesh.geometry.getAttribute('position').count;normals+=mesh.geometry.getAttribute('normal')?.count??0;triangles+=(mesh.geometry.index?.count??mesh.geometry.getAttribute('position').count)/3;}});
      const expected=fixture.triangles*3;if(JSON.stringify(names)!==JSON.stringify(fixture.names))throw new Error('Solid name/group mismatch');if(triangles!==expected/3||normals!==vertices||vertices>expected)throw new Error(`Complete-model count mismatch: vertices=${vertices}, normals=${normals}, triangles=${triangles}, expected=${expected/3}`);const pendingPreview=prepared.pendingPreview;prepared.dispose();return {source,ended,vertices,normals,triangles,names,bounds:{min:prepared.bounds.min.toArray(),max:prepared.bounds.max.toArray()},pendingPreview};
    };
    try {
      const first=preview('A'), work=[first];
      if(competing){work.push(preview('B'));while(requests.filter(value=>value.kind==='read').length<2)await new Promise(resolve=>setTimeout(resolve,0));downloadSubmitted=performance.now()-origin;
        const request={source:'download',kind:'download',offset:0,submitted:downloadSubmitted,admitted:0,ended:0,bytes:0};requests.push(request);
        work.push(demand.run(new AbortController().signal,async()=>{request.admitted=performance.now()-origin;active++;maximum=Math.max(maximum,active);try{request.bytes=(await(await checkedFetch(`/__stream/download?size=${size}`)).arrayBuffer()).byteLength;downloadEnded=performance.now()-origin;return {source:'download',ended:downloadEnded,vertices:0,normals:0,triangles:0,names:[] as string[],bounds:{min:[0,0,0] as [number,number,number],max:[0,0,0] as [number,number,number]},pendingPreview:false};}finally{active--;request.ended=performance.now()-origin;}}));}
      const results=await Promise.all(work),elapsed=performance.now()-origin;
      // Independent sampled authored-attribute verification runs after the timing
      // interval. Full triangle and normal counts above also bind EOF admission.
      const decodedDigests: Record<string,string>={};
      for(const {source,geometry:output} of verification){
        const positions=output.getAttribute('position'),normals=output.getAttribute('normal'),count=(output.index?.count??positions.count)/3,sampled=new Float32Array(64*18);
        for(let sample=0;sample<64;sample++){
          const face=Math.floor((count-1)*sample/63),sourceFace=face%65536,x=sourceFace%256,y=Math.floor(sourceFace/256),expected=fixture.format==='ascii'?[[0,0,0],[1,0,0],[0,1,0]]:[[x,y,0],[x+1,y,0],[x,y+1,0]];
          for(let corner=0;corner<3;corner++){
            const vertex=output.index?.getX(face*3+corner)??face*3+corner;
            const actual=[positions.getX(vertex),positions.getY(vertex),positions.getZ(vertex)],normal=[normals.getX(vertex),normals.getY(vertex),normals.getZ(vertex)];
            if(actual.some((value,axis)=>value!==expected[corner][axis])||normal.some((value,axis)=>value!==Number(axis===2)))throw new Error('Authored attribute mismatch');
            sampled.set(actual,sample*18+corner*6);sampled.set(normal,sample*18+corner*6+3);
          }
        }
        decodedDigests[source]=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',sampled.buffer))).map(value=>value.toString(16).padStart(2,'0')).join('');
      }
      if(maximum>2||chunks.some(chunk=>!chunk.transferred||chunk.bytes>2*1024*1024)||copiedBySlice)throw new Error('Bound/transfer regression');
      return {decodedDigests,policy,sizeMiB:size,competing,delayMs:delay,elapsedMs:elapsed,maximumPhysicalReads:maximum,downloadLatencyMs:competing?downloadEnded-downloadSubmitted:null,downloadAdmissionRank:competing?requests.filter(value=>value.kind==='read'||value.kind==='download').sort((a,b)=>a.admitted-b.admitted).findIndex(value=>value.source==='download'):null,copiedBySlice,parentSourceAssemblyBytes:0,workerRetainedSourceBytes:fixture.format==='ascii'?0:chunks.reduce((sum,value)=>sum+value.bytes,0)>16666784?chunks.reduce((sum,value)=>sum+value.bytes,0):0,policyUpgrades:scheduling.filter(value=>value.nextWidth>value.width).length,policyDownshifts:scheduling.filter(value=>value.nextWidth<value.width).length,scheduling,chunkTransferredBytes:chunks.reduce((sum,value)=>sum+value.bytes,0),results,requests,chunks};
    }finally{Uint8Array.prototype.slice=originalSlice;}
  },{policy,size,competing,delay,dev,fixture:fixtures.find(fixture=>fixture.sizeMiB===size)!});
  // Warm imports, Worker code and local filesystem cache outside samples.
  await run('fixed1', sizes[0], false, 0);
  const samples=[];
  const scenarios=asciiSmoke?[{size:32,competing:false,delay:0}]:defaultPolicy?[{size:200,competing:false,delay:0},{size:32,competing:true,delay:0}]:competitionOnly?[{size:sizes[0],competing:true,delay:0}]:smoke?[{size:sizes[0],competing:false,delay:0},{size:sizes[0],competing:true,delay:0}]:[{size:200,competing:false,delay:0},{size:32,competing:true,delay:0},{size:32,competing:false,delay:10},{size:32,competing:false,delay:50}];
  for(let pair=0;pair<pairs;pair++)for(const scenario of scenarios)for(const policy of defaultPolicy||asciiSmoke?['default']:pair%2?['adaptive','fixed2','fixed1']:['fixed1','fixed2','adaptive']){
    try{const result=await run(policy,scenario.size,scenario.competing,scenario.delay);samples.push({pair,...result});process.stderr.write(`${policy} ${scenario.size}MiB competing=${scenario.competing} delay=${scenario.delay}: ${result.elapsedMs.toFixed(1)}ms\n`);}catch(error){const failure={pair,policy,...scenario,error:String(error)};samples.push(failure);process.stderr.write(JSON.stringify(failure)+'\n');}
  }
  for(const name of tracked)if(createHash('sha256').update(fs.readFileSync(path.join(root,name))).digest('hex')!==hashes[name])throw new Error(`Source changed during benchmark: ${name}`);
  if(!dev&&networkRequests.some(url=>/occt|cad-worker|\.wasm(?:\?|$)|\/src\//.test(url)))throw new Error('STL benchmark requested a CAD runtime or development source');
  const output={generatedAt:new Date().toISOString(),browser:browser.version(),fixtures,device:{platform:os.platform(),release:os.release(),cpus:os.cpus().length,cpu:os.cpus()[0]?.model},smoke,asciiSmoke,competitionOnly,pairs,defaultPolicy,reuseBundle,codeDelivery:dev?'Vite development modules':'Minified Vite production bundle and static Worker assets',artifacts,networkRequests,transport:'Production openWorkspaceBytePreview persistent descriptor, HTTP binary pages, production PreviewDemand(2), disposable production STL Worker; zero artificial delay in local scenarios',hashes,samples};
  fs.mkdirSync(path.dirname(path.resolve(option('--output'))),{recursive:true});fs.writeFileSync(option('--output'),JSON.stringify(output,null,2)+'\n');
  if(samples.some(sample=>'error' in sample))throw new Error('One or more retained benchmark samples failed');
}finally{await context.close();await browser.close();await viteServer?.close();if(staticServer)await new Promise<void>((resolve,reject)=>staticServer!.close(error=>error?reject(error):resolve()));for(const operation of operations.values())await operation.close().catch(()=>{});fs.rmSync(directory,{recursive:true,force:true});}
