import { test, expect } from '@playwright/test';
import { writeFileSync, statSync, createReadStream } from 'node:fs';
import { createHash } from 'node:crypto';
import { instrument, resources, observeGeometryLatency } from './geometry-observation';

const source = process.env.RUSTX_GEOMETRY_LARGE_STEP_PATH;
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
test.use({ connectOptions: { wsEndpoint: process.env.RUSTX_BROWSER_WS_ENDPOINT ?? '', exposeNetwork: '<loopback>' } });
test('approximately 200 MB STEP first model, original geometry and interaction budget', async ({ page }, info) => {
  test.skip(!source, 'Generate or provide a genuine approximately 200 MB STEP using RUSTX_GEOMETRY_LARGE_STEP_PATH.');
  const size = statSync(source!).size; expect(size).toBeGreaterThanOrEqual(190_000_000); expect(size).toBeLessThanOrEqual(200 * 1024 * 1024);
  const digest = createHash('sha256'); for await (const chunk of createReadStream(source!)) digest.update(chunk);
  await instrument(page); await page.goto(`${origin}/test/fixtures/geometry.html`);
  expect((await page.evaluate(() => (window as any).geometryContract.loadLargeStep())).bytes).toBe(size);
  await observeGeometryLatency(page);
  await page.getByRole('button', { name: 'Preview cube.step in sidebar', exact: true }).click();
  const body = page.locator('[data-geometry-format="step"]');
  await expect.poll(()=>page.evaluate(()=>(window as any).geometryLatency.coarseFirst || (window as any).geometryLatency.first),{timeout:70_000}).toBeGreaterThan(0);
  const coarseFirst=await page.evaluate(()=>(window as any).geometryLatency.coarseFirst as number),coarseInteractions:number[]=[];
  if(coarseFirst){const rough=page.locator('svg[data-geometry-rough]'),box=(await rough.boundingBox())!;await rough.screenshot({path:info.outputPath('step-200mb-rough.png')});
    for(const button of ['left','right'] as const)for(let action=0;action<5;action++){const x=box.x+box.width/2,y=box.y+box.height/2;await page.mouse.move(x,y);await page.mouse.down({button});await page.mouse.move(x+32,y+12,{steps:10});await page.mouse.up({button});}
    await expect.poll(()=>page.evaluate(()=>(window as any).geometryLatency.coarsePending)).toBe(0);coarseInteractions.push(...await page.evaluate(()=>(window as any).geometryLatency.coarseSamples as number[]));
  }
  await expect.poll(async()=>await page.evaluate(()=>(window as any).geometryLatency.first>0)?'ready':await body.getByRole('alert').count()?'error':'loading',{timeout:70_000}).not.toBe('loading');
  const fineFirst=await page.evaluate(()=>(window as any).geometryLatency.first as number),first=coarseFirst||fineFirst,outcome=fineFirst>0?'ready':'error';
  const interactions: number[] = [];
  if (outcome === 'ready') {
    const canvas = body.locator('canvas'), box = (await canvas.boundingBox())!;
    for (const button of ['left', 'right'] as const) {
      for (let action=0;action<5;action++) {
        const x=box.x+box.width/2,y=box.y+box.height/2;
        await page.mouse.move(x,y);await page.mouse.down({button});await page.mouse.move(x+32,y+12,{steps:10});await page.mouse.up({button});
      }
    }
    await expect.poll(()=>page.evaluate(()=>(window as any).geometryLatency.pending)).toBe(0);
    interactions.push(...await page.evaluate(()=>(window as any).geometryLatency.samples as number[]));
    await page.getByRole('button',{name:'View angle',exact:true}).click();
    await page.getByRole('menuitem',{name:'Reset view',exact:true}).click();
    await expect(canvas).toHaveAttribute('data-geometry-quality','complete');
    await expect.poll(async()=>(await resources(page)).workers).toBe(0);
    const triangles=await page.evaluate(()=>(window as any).geometryImports.at(-1).reduce((sum:number,mesh:{triangles:number})=>sum+mesh.triangles,0));
    expect(await page.evaluate(()=>(window as any).geometryDrawCalls.lastModelTriangles)).toBe(triangles);
    await body.screenshot({path:info.outputPath('step-200mb-complete.png')});
  }
  const result={workload:'step-200mb',source:{bytes:size,sha256:digest.digest('hex')},outcome,firstModelOpportunityMs:first||null,firstFineModelOpportunityMs:fineFirst||null,coarseFirstModelOpportunityMs:coarseFirst||null,coarseInteractions,maximumCoarseInteractionMs:coarseInteractions.length?Math.max(...coarseInteractions):null,
    error:await body.getByRole('alert').count()?await body.getByRole('alert').textContent():null,interactions,maximumInteractionMs:interactions.length?Math.max(...interactions):null,
    imports:await page.evaluate(()=>(window as any).geometryImports),phases:await page.evaluate(()=>performance.getEntriesByType('measure').filter(entry=>entry.name.startsWith('geometry:')).map(entry=>({name:entry.name,ms:entry.duration}))),
    beforeClose:await resources(page),method:'Original STEP; source preloaded before demand, authorized range sampling and complete assembly occur inside demand. Coarse coordinate envelope: DOM update then two rAF turns. Fine mesh: actual model submission -> GPU fence -> next rAF. Both clocks retained; neither certifies physical presentation or native source transport.'};
  await page.getByRole('button',{name:'Close preview cube.step',exact:true}).click();
  await expect.poll(()=>resources(page)).toEqual({workers:0,contexts:0,buffers:0,textures:0,urls:0});
  writeFileSync(info.outputPath('step-200mb-latency.json'),JSON.stringify({...result,afterClose:await resources(page)},null,2));
  await info.attach('step-200mb-latency',{body:JSON.stringify(result,null,2),contentType:'application/json'});
  expect.soft(outcome).toBe('ready');
  if(outcome==='ready')expect.soft(interactions.length).toBeGreaterThanOrEqual(80);
  if(process.env.RUSTX_GEOMETRY_BUDGET==='1') {expect.soft(first||Infinity).toBeLessThanOrEqual(200);if(interactions.length)expect.soft(Math.max(...interactions)).toBeLessThan(50);if(coarseInteractions.length)expect.soft(Math.max(...coarseInteractions)).toBeLessThan(50);}
});
