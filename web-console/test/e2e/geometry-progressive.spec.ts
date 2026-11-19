import {test,expect,type Page} from '@playwright/test';
import {instrument,observeGeometryLatency,resources} from './geometry-observation';
import {GEOMETRY_LIMITS} from '../../src/app/components/documents/geometry/limits';

const origin=`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT??5174}`;
test.use({connectOptions:{wsEndpoint:process.env.RUSTX_BROWSER_WS_ENDPOINT??'',exposeNetwork:'<loopback>'}});
async function heldCAD(page:Page) {
  await instrument(page);
  await page.addInitScript(()=>{
    const records:{worker:Worker;lods:Map<number,MessageEvent>;final?:MessageEvent;handler?:Worker['onmessage']}[]=[];
    const Native=Worker;
    window.Worker=class extends Native {
      constructor(url:string|URL,options?:WorkerOptions){super(url,options);if(options?.name!=='rustx-cad')return;
        const record:{worker:Worker;lods:Map<number,MessageEvent>;final?:MessageEvent;handler?:Worker['onmessage']}={worker:this,lods:new Map()};records.push(record);
        this.addEventListener('message',event=>{
          if(event.data?.kind==='lod'){record.handler=this.onmessage;record.lods.set(event.data.level,event);event.stopImmediatePropagation();}
          else if(event.data?.ok===true&&event.data?.model){record.handler=this.onmessage;record.final=event;event.stopImmediatePropagation();}
        });
      }
    };
    const get=HTMLCanvasElement.prototype.getContext,seen=new WeakSet<object>();let contexts=0;
    Object.defineProperty(HTMLCanvasElement.prototype,'getContext',{value:function(this:HTMLCanvasElement,...args:unknown[]){const result=Reflect.apply(get,this,args);if(args[0]==='webgl2'&&result&&!seen.has(result)){seen.add(result);contexts++;}return result;}});
    Object.assign(window,{cadProgressive:{
      state:()=>({levels:[...(records[0]?.lods.keys()??[])],final:!!records[0]?.final,contexts}),
      finalModel:()=>{const model=records[0]?.final?.data.model;return model?{meshNames:model.meshes.map((mesh:any)=>mesh.name),root:model.root,triangles:model.meshes.reduce((sum:number,mesh:any)=>sum+mesh.indices.length/3,0)}:undefined;},
      lod:(level:number)=>{const record=records[0],event=record.lods.get(level);if(event)record.handler?.call(record.worker,event);},
      final:()=>{const record=records[0],event=record.final;if(event)record.handler?.call(record.worker,event);},
    }});
  });
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.evaluate(async()=>{const bytes=[...new Uint8Array(await(await fetch('/test/fixtures/geometry/lod-assembly.step')).arrayBuffer())];(window as any).geometryContract.replace('cube.step',bytes);});
  await observeGeometryLatency(page);
  await page.getByRole('button',{name:'Preview cube.step in sidebar',exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).cadProgressive.state().final)).toBe(true);
  expect(await page.evaluate(()=>(window as any).cadProgressive.state().levels)).toEqual([0]);
  const tiers=await page.evaluate(()=>(window as any).geometryLods);
  expect(tiers).toHaveLength(1);expect(tiers[0].level).toBe(0);expect(tiers[0].meshNames.length).toBeGreaterThan(1);
  const final=await page.evaluate(()=>(window as any).cadProgressive.finalModel());
  expect(tiers[0].meshNames).toEqual(final.meshNames);expect(tiers[0].root).toEqual(final.root);
  expect(tiers[0].triangles).toBeLessThan(final.triangles);
  return tiers as {level:number;meshNames:string[];root:unknown;triangles:number}[];
}
const bracketNames=['Base plate','Upright','Left gusset','Right gusset','Washer 1','Bolt 1','Washer 2','Bolt 2','Washer 3','Bolt 3','Washer 4','Bolt 4'];
const naturalCases=[
  {
    title:'an ordinary unheld 96-part CAD import preserves the complete model and may skip an unpainted LOD',
    fixture:'bracket-array.step',occurrences:96,coarseTriangles:9088,fullTriangles:17824,
    names:Array.from({length:8},()=>bracketNames).flat(),
    policy:'may-skip-unpainted',attachment:'natural-unheld-CAD-LOD',
  },
  {
    title:'an unheld high-output 100-sphere STEP import presents a complete LOD before the full model',
    fixture:'curved-100.step',occurrences:100,coarseTriangles:23400,fullTriangles:500800,
    names:Array.from({length:100},(_,index)=>`Curved part ${String(index).padStart(3,'0')}`),
    policy:'display-before-full',attachment:'natural-unheld-high-output-CAD-LOD',
  },
] as const;
for(const sample of naturalCases)test(sample.title,async({page},info)=>{
  await instrument(page);
  await page.addInitScript(()=>{
    const get=HTMLCanvasElement.prototype.getContext,seen=new WeakSet<object>();let contexts=0;
    Object.defineProperty(HTMLCanvasElement.prototype,'getContext',{value:function(this:HTMLCanvasElement,...args:unknown[]){const result=Reflect.apply(get,this,args);if(args[0]==='webgl2'&&result&&!seen.has(result)){seen.add(result);contexts++;}return result;}});
    Object.assign(window,{geometryCreatedContexts:()=>contexts});
  });
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.evaluate(async fixture=>{const bytes=[...new Uint8Array(await(await fetch(`/test/fixtures/geometry/${fixture}`)).arrayBuffer())];(window as any).geometryContract.replace('cube.step',bytes);},sample.fixture);
  await observeGeometryLatency(page);
  await page.getByRole('button',{name:'Preview cube.step in sidebar',exact:true}).click();
  const capture=()=>page.evaluate(()=>({nativeTiers:(window as any).geometryLods,latency:(window as any).geometryLatency,frames:(window as any).geometryModelFrames,imports:(window as any).geometryImports,roots:(window as any).geometryImportRoots,importTimings:(window as any).geometryImportTimings,createdContexts:(window as any).geometryCreatedContexts(),resources:{...(window as any).geometryResources}}));
  let captured=false;
  try {
    const full=page.locator('[data-geometry-completeness="complete"][data-geometry-lod="full"]');
    const failure=page.locator('[data-geometry-format="step"]').getByRole('alert');
    // Wait for the product's terminal outcome, including cold native meshing on CI.
    await expect(full.or(failure)).toBeVisible({timeout:GEOMETRY_LIMITS.cadTimeoutMs});
    if(await failure.isVisible())throw new Error(`STEP preview failed with "${await failure.textContent()}"`);
    await expect(full).toHaveAttribute('data-geometry-quality','complete');
    await expect.poll(()=>page.evaluate(()=>(window as any).geometryLatency.first)).toBeGreaterThan(0);await expect.poll(async()=>(await resources(page)).workers).toBe(0);
    // Settle test telemetry for tiers that really submitted triangles. A final
    // frame does not guarantee an earlier fence observer's rAF has run yet.
    // This wait occurs after final readiness and never delays product adoption.
    await expect.poll(()=>page.evaluate(()=>Object.values((window as any).geometryLatency.lodStages).every((stage:any)=>stage.gpu>0&&stage.opportunity>0))).toBe(true);
    const evidence=await capture();
    await info.attach(sample.attachment,{body:JSON.stringify({fixture:sample.fixture,expectedOccurrences:sample.occurrences,policy:sample.policy,...evidence,method:'Natural unheld import with normal observation only: no Worker-message interception, delayed delivery, synthetic stages or triangle sampling. Native arrival, actual submitted LOD frames, GPU fences and frame opportunities are recorded independently. Observer settlement after final readiness does not hold back the final model. Original-tessellation triangle-count proof uses the first full frame and complete-quality frames; interactive frames are not authoritative final-quality proof.'},null,2),contentType:'application/json'});
    captured=true;
    const imported=evidence.imports.at(-1),root=evidence.roots.at(-1),fullFirst=evidence.latency.first;
    expect(evidence.imports).toHaveLength(1);expect(imported).toHaveLength(sample.occurrences);expect(imported.map((mesh:any)=>mesh.name)).toEqual(sample.names);
    const triangles=imported.reduce((sum:number,mesh:any)=>sum+mesh.triangles,0);expect(triangles).toBe(sample.fullTriangles);
    const occurrences=(node:any):number=>node.meshes.length+node.children.reduce((sum:number,child:any)=>sum+occurrences(child),0);
    expect(occurrences(root)).toBe(sample.occurrences);
    // The one native tier is independently required to contain the entire
    // assembly even when the final result supersedes it before presentation.
    expect(evidence.nativeTiers).toHaveLength(1);
    const tier=evidence.nativeTiers[0];expect(tier.level).toBe(0);expect(tier.meshNames).toEqual(sample.names);expect(tier.root).toEqual(root);expect(occurrences(tier.root)).toBe(sample.occurrences);expect(tier.triangles).toBe(sample.coarseTriangles);expect(tier.triangles).toBeLessThan(triangles);
    expect(evidence.frames.length).toBeGreaterThan(0);
    for(const frame of evidence.frames){
      if(frame.completeness==='preview'){
        expect(frame.lod).toBe('coarse');expect(frame.quality).toBe('preview');expect(frame.triangles).toBe(tier.triangles);expect(evidence.latency.lodStages.coarse).toBeDefined();
      }else{
        expect(frame.completeness).toBe('complete');expect(frame.lod).toBe('full');expect(['complete','interactive']).toContain(frame.quality);
        if(frame.quality==='complete')expect(frame.triangles).toBe(triangles);
        else{expect(frame.triangles).toBeGreaterThan(0);expect(frame.triangles).toBeLessThanOrEqual(triangles);}
      }
    }
    const fullFrames=evidence.frames.filter((frame:any)=>frame.completeness==='complete');expect(fullFrames.length).toBeGreaterThan(0);expect(fullFrames[0].triangles).toBe(triangles);
    expect(fullFrames.some((frame:any)=>frame.quality==='complete'&&frame.triangles===triangles)).toBe(true);
    // Interactive full-model frames may use existing admitted one-pixel-error
    // surfaces. They do not prove the authoritative original tessellation.
    for(const [lod,stage] of Object.entries(evidence.latency.lodStages) as [string,{submission:number;gpu:number;opportunity:number}][]){
      expect(lod).toBe('coarse');expect(stage.submission).toBeGreaterThan(0);expect(stage.gpu).toBeGreaterThanOrEqual(stage.submission);expect(stage.opportunity).toBeGreaterThanOrEqual(stage.gpu);
      expect(evidence.frames.some((frame:any)=>frame.completeness==='preview'&&frame.lod===lod&&frame.triangles===tier.triangles&&frame.ms-evidence.latency.start<=stage.gpu)).toBe(true);
    }
    const displayed=Object.entries(evidence.latency.lodStages).filter(([,stage]:[string,any])=>stage.submission>0&&stage.gpu>=stage.submission&&stage.opportunity>=stage.gpu&&stage.opportunity<fullFirst);
    // Small imports may finish before an unpainted tier is chosen. A tied or
    // later observer opportunity is never counted as coarse-before-full proof.
    // The high-output workload retains the original strict natural requirement.
    if(sample.policy==='display-before-full')expect(displayed.length).toBeGreaterThan(0);
    expect(evidence.createdContexts).toBe(1);expect(evidence.resources.contexts).toBe(1);
  } finally {
    try {
      if(!captured){
        const evidence=await capture().catch(error=>({captureError:String(error)}));
        await info.attach(`${sample.attachment}-incomplete`,{body:JSON.stringify({fixture:sample.fixture,expectedOccurrences:sample.occurrences,policy:sample.policy,...evidence,method:'Incomplete observation captured immediately before retirement after a failed readiness check. Only existing globals are recorded; missing arrivals or frame timings are not synthesized.'},null,2),contentType:'application/json'});
      }
    } finally {
      await page.getByRole('button',{name:'Close preview cube.step',exact:true}).click();await expect.poll(()=>resources(page)).toEqual({workers:0,contexts:0,buffers:0,textures:0,urls:0});
      await info.attach(sample.attachment.replace('-LOD','-retirement'),{body:JSON.stringify(await resources(page)),contentType:'application/json'});
    }
  }
});
test('whole-model CAD tessellation refines coarse to full through one focused renderer',async({page},info)=>{
  const tiers=await heldCAD(page);await page.evaluate(()=>(window as any).cadProgressive.lod(0));
  const canvas=page.locator('[data-geometry-completeness="preview"]');await expect(canvas).toBeVisible();await expect(canvas).toHaveAttribute('data-geometry-lod','coarse');
  await expect.poll(()=>page.evaluate(()=>(window as any).geometryLatency.lodStages.coarse?.opportunity??0)).toBeGreaterThan(0);
  await expect(page.getByRole('status',{name:''}).filter({hasText:'Refining model'})).toBeVisible();await expect(page.getByRole('button',{name:'Parts',exact:true})).toBeDisabled();
  await expect.poll(()=>page.evaluate(()=>(window as any).geometryDrawCalls.lastModelTriangles)).toBe(tiers[0].triangles);
  await canvas.evaluate(element=>{element.setAttribute('data-progressive-identity','same-context');});
  await page.screenshot({path:info.outputPath('cad-lod-coarse.png')});
  const box=(await canvas.boundingBox())!;
  for(const button of ['left','right'] as const)for(let action=0;action<4;action++){const sign=action%2?-1:1,x=box.x+box.width/2,y=box.y+box.height/2;await page.mouse.move(x,y);await page.mouse.down({button});await page.mouse.move(x+sign*24,y+sign*10,{steps:8});await page.mouse.up({button});}
  await expect.poll(()=>page.evaluate(()=>(window as any).geometryLatency.pending)).toBe(0);
  await canvas.focus();await canvas.press('Numpad5');await expect(canvas).toBeFocused();
  await page.mouse.move(box.x+box.width/2,box.y+box.height/2);await page.mouse.down();
  await page.evaluate(()=>(window as any).cadProgressive.final());await expect(canvas).toHaveCount(1);
  await expect(canvas).toHaveAttribute('data-geometry-lod','coarse');await expect(canvas).toBeFocused();
  await expect(page.getByRole('button',{name:'Parts',exact:true})).toBeDisabled();
  await expect.poll(()=>page.evaluate(()=>(window as any).geometryDrawCalls.lastModelTriangles)).toBe(tiers[0].triangles);
  await page.mouse.move(box.x+box.width/2+24,box.y+box.height/2+12,{steps:5});
  await expect(canvas).toHaveAttribute('data-geometry-lod','coarse');await expect(page.getByRole('button',{name:'Parts',exact:true})).toBeDisabled();await page.mouse.up();
  const full=page.locator('[data-geometry-completeness="complete"]');await expect(full).toBeVisible();await expect(full).toHaveAttribute('data-geometry-lod','full');await expect(full).toHaveAttribute('data-progressive-identity','same-context');await expect(full).toBeFocused();
  await expect(page.getByRole('button',{name:'Parts',exact:true})).toBeEnabled();await expect.poll(async()=>(await resources(page)).workers).toBe(0);await expect(full).toHaveAttribute('data-geometry-quality','complete');
  await expect.poll(()=>page.evaluate(()=>(window as any).geometryContract.snapshot().find((tab:any)=>tab.name==='cube.step')?.state?.camera?.projection)).toBe('perspective');
  const imported=await page.evaluate(()=>(window as any).geometryImports.at(-1));
  expect(imported.map((mesh:any)=>mesh.name)).toEqual(tiers[0].meshNames);expect(await page.evaluate(()=>(window as any).geometryImportRoots.at(-1))).toEqual(tiers[0].root);
  const triangles=imported.reduce((sum:number,mesh:any)=>sum+mesh.triangles,0);expect(triangles).toBeGreaterThan(tiers[0].triangles);
  expect(triangles).toBe(await page.evaluate(()=>(window as any).cadProgressive.finalModel().triangles));
  await expect.poll(()=>page.evaluate(()=>(window as any).geometryDrawCalls.lastModelTriangles)).toBe(triangles);
  const keyboardDraws=await page.evaluate(()=>(window as any).geometryDrawCalls.modelCalls);
  await page.keyboard.press('ArrowRight');await expect.poll(()=>page.evaluate(()=>(window as any).geometryDrawCalls.modelCalls)).toBeGreaterThan(keyboardDraws);await expect(full).toBeFocused();
  await page.evaluate(()=>{(window as any).cadProgressive.lod(0);(window as any).cadProgressive.final();});await expect(full).toHaveCount(1);await expect(full).toHaveAttribute('data-geometry-lod','full');
  expect(await page.evaluate(()=>(window as any).cadProgressive.state().contexts)).toBe(1);
  const samples=await page.evaluate(()=>(window as any).geometryLatency.lodSamples as number[]);expect(samples.length).toBeGreaterThanOrEqual(40);
  await page.screenshot({path:info.outputPath('cad-lod-full.png')});
  await info.attach('whole-model-CAD-coarse-final',{body:JSON.stringify({nativeTiers:await page.evaluate(()=>(window as any).geometryLods),displayedTiers:await page.evaluate(()=>(window as any).geometryLatency.lodStages),completeTriangles:triangles,lodInputSamples:samples,maximumLodInputMs:Math.max(...samples),createdContexts:1,method:'Actual native whole-model coarse/final tessellations with controlled message delivery. Every stage contains the same complete assembly hierarchy; no sampled triangles or withheld-final proxy tiers.'},null,2),contentType:'application/json'});
  if(process.env.RUSTX_GEOMETRY_BUDGET==='1')expect(Math.max(...samples)).toBeLessThan(50);
  await page.getByRole('button',{name:'Close preview cube.step',exact:true}).click();await expect.poll(()=>resources(page)).toEqual({workers:0,contexts:0,buffers:0,textures:0,urls:0});
});
test('whole-model adoption preserves focus on external controls',async({page})=>{
  await heldCAD(page);await page.evaluate(()=>(window as any).cadProgressive.lod(0));await expect(page.locator('[data-geometry-lod="coarse"]')).toBeVisible();
  const external=page.getByRole('button',{name:'Close preview cube.step',exact:true});await external.focus();await expect(external).toBeFocused();
  await page.evaluate(()=>(window as any).cadProgressive.final());await expect(page.locator('[data-geometry-lod="full"]')).toBeVisible();await expect(external).toBeFocused();
  await external.click();await expect.poll(()=>resources(page)).toEqual({workers:0,contexts:0,buffers:0,textures:0,urls:0});
});
test('retirement destroys LOD graphics and fences native tiers and final results held in transit',async({page})=>{
  await heldCAD(page);await page.evaluate(()=>(window as any).cadProgressive.lod(0));await expect(page.locator('[data-geometry-lod="coarse"]')).toBeVisible();
  await page.getByRole('button',{name:'Hide delivery',exact:true}).click();await expect.poll(()=>resources(page)).toEqual({workers:0,contexts:0,buffers:0,textures:0,urls:0});
  await page.evaluate(()=>{(window as any).cadProgressive.lod(0);(window as any).cadProgressive.final();});
  await expect(page.locator('[data-geometry-format] canvas')).toHaveCount(0);expect(await resources(page)).toEqual({workers:0,contexts:0,buffers:0,textures:0,urls:0});
});
