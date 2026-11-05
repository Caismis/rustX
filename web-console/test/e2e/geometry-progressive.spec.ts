import {test,expect,type Page} from '@playwright/test';
import {instrument,observeGeometryLatency,resources} from './geometry-observation';

const origin=`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT??5174}`;
test.use({connectOptions:{wsEndpoint:process.env.RUSTX_BROWSER_WS_ENDPOINT??'',exposeNetwork:'<loopback>'}});
async function heldCAD(page:Page) {
  await instrument(page);
  await page.addInitScript(()=>{
    const records:{worker:Worker;parts:MessageEvent[];final?:MessageEvent;handler?:Worker['onmessage']}[]=[];
    const Native=Worker;
    window.Worker=class extends Native {
      constructor(url:string|URL,options?:WorkerOptions){super(url,options);if(options?.name!=='rustx-cad')return;
        const record:{worker:Worker;parts:MessageEvent[];final?:MessageEvent;handler?:Worker['onmessage']}={worker:this,parts:[]};records.push(record);
        this.addEventListener('message',event=>{
          if(event.data?.kind==='partial'){record.handler=this.onmessage;record.parts.push(event);event.stopImmediatePropagation();}
          else if(event.data?.ok===true&&event.data?.model){record.handler=this.onmessage;record.final=event;event.stopImmediatePropagation();}
        });
      }
    };
    const get=HTMLCanvasElement.prototype.getContext,seen=new WeakSet<object>();let contexts=0;
    Object.defineProperty(HTMLCanvasElement.prototype,'getContext',{value:function(this:HTMLCanvasElement,...args:unknown[]){const result=Reflect.apply(get,this,args);if(args[0]==='webgl2'&&result&&!seen.has(result)){seen.add(result);contexts++;}return result;}});
    Object.assign(window,{cadProgressive:{
      state:()=>({parts:records[0]?.parts.length??0,final:!!records[0]?.final,contexts}),
      parts:(count:number)=>{const record=records[0];for(const event of record.parts.splice(0,count))record.handler?.call(record.worker,event);},
      final:()=>{const record=records[0],event=record.final;if(event){record.final=undefined;record.handler?.call(record.worker,event);}},
    }});
  });
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.evaluate(async()=>{const bytes=[...new Uint8Array(await(await fetch('/test/fixtures/geometry/bracket-array.step')).arrayBuffer())];(window as any).geometryContract.replace('cube.step',bytes);});
  await observeGeometryLatency(page);
  await page.getByRole('button',{name:'Preview cube.step in sidebar',exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).cadProgressive.state().final)).toBe(true);
  expect(await page.evaluate(()=>(window as any).cadProgressive.state().parts)).toBeGreaterThan(2);
}
test('computed CAD fragments improve the picture before full completion and share one renderer through quiet handoff',async({page},info)=>{
  await heldCAD(page);await page.evaluate(()=>(window as any).cadProgressive.parts(1));
  const canvas=page.locator('[data-geometry-completeness="partial"]');await expect(canvas).toBeVisible();await expect.poll(()=>page.evaluate(()=>(window as any).geometryLatency.partialFirst)).toBeGreaterThan(0);
  await expect(page.getByRole('status',{name:''}).filter({hasText:'Refining model'})).toBeVisible();
  await expect(page.getByRole('button',{name:'Parts',exact:true})).toBeDisabled();
  const firstTriangles=await page.evaluate(()=>(window as any).geometryDrawCalls.lastModelTriangles);
  await page.screenshot({path:info.outputPath('cad-partial-first.png')});
  await page.evaluate(()=>(window as any).cadProgressive.parts(2));await expect.poll(()=>page.evaluate(()=>(window as any).geometryDrawCalls.lastModelTriangles)).toBeGreaterThan(firstTriangles);
  await canvas.evaluate(element=>{element.setAttribute('data-progressive-identity','same-context');});
  await page.screenshot({path:info.outputPath('cad-partial-more.png')});
  const box=(await canvas.boundingBox())!;
  for(const button of ['left','right'] as const)for(let action=0;action<4;action++){const sign=action%2?-1:1,x=box.x+box.width/2,y=box.y+box.height/2;await page.mouse.move(x,y);await page.mouse.down({button});await page.mouse.move(x+sign*24,y+sign*10,{steps:8});await page.mouse.up({button});}
  await expect.poll(()=>page.evaluate(()=>(window as any).geometryLatency.pending)).toBe(0);
  await page.mouse.move(box.x+box.width/2,box.y+box.height/2);await page.mouse.down();await page.evaluate(()=>(window as any).cadProgressive.final());
  await expect(canvas).toHaveCount(1);await page.mouse.move(box.x+box.width/2+16,box.y+box.height/2+8,{steps:5});await page.mouse.up();
  const full=page.locator('[data-geometry-completeness="complete"]');await expect(full).toBeVisible();await expect(full).toHaveAttribute('data-progressive-identity','same-context');
  await expect(page.getByRole('button',{name:'Parts',exact:true})).toBeEnabled();
  await expect.poll(async()=>(await resources(page)).workers).toBe(0);await expect(full).toHaveAttribute('data-geometry-quality','complete');
  const triangles=await page.evaluate(()=>(window as any).geometryImports.at(-1).reduce((sum:number,mesh:any)=>sum+mesh.triangles,0));
  await expect.poll(()=>page.evaluate(()=>(window as any).geometryDrawCalls.lastModelTriangles)).toBe(triangles);
  await page.evaluate(()=>(window as any).cadProgressive.parts(100));await expect(full).toHaveCount(1);
  expect(await page.evaluate(()=>(window as any).cadProgressive.state().contexts)).toBe(1);
  const samples=await page.evaluate(()=>(window as any).geometryLatency.partialSamples as number[]);expect(samples.length).toBeGreaterThanOrEqual(40);
  await page.screenshot({path:info.outputPath('cad-partial-complete.png')});
  await info.attach('progressive-CAD',{body:JSON.stringify({firstPartialTriangles:firstTriangles,completeTriangles:triangles,partialInputSamples:samples,maximumPartialInputMs:Math.max(...samples),nativeParts:await page.evaluate(()=>(window as any).geometryPartials),createdContexts:1,method:'Real CAD-generated fragments with controlled message delivery/final hold, not a cold-performance result. Partial and final GPU clocks are distinct.'},null,2),contentType:'application/json'});
  if(process.env.RUSTX_GEOMETRY_BUDGET==='1')expect(Math.max(...samples)).toBeLessThan(50);
  await page.getByRole('button',{name:'Close preview cube.step',exact:true}).click();await expect.poll(()=>resources(page)).toEqual({workers:0,contexts:0,buffers:0,textures:0,urls:0});
});
test('retirement destroys partial graphics and fences native fragments and the final result held in transit',async({page})=>{
  await heldCAD(page);await page.evaluate(()=>(window as any).cadProgressive.parts(1));await expect(page.locator('[data-geometry-completeness="partial"]')).toBeVisible();
  await page.getByRole('button',{name:'Hide delivery',exact:true}).click();await expect.poll(()=>resources(page)).toEqual({workers:0,contexts:0,buffers:0,textures:0,urls:0});
  await page.evaluate(()=>{(window as any).cadProgressive.parts(100);(window as any).cadProgressive.final();});
  await expect(page.locator('[data-geometry-format] canvas')).toHaveCount(0);expect(await resources(page)).toEqual({workers:0,contexts:0,buffers:0,textures:0,urls:0});
});
