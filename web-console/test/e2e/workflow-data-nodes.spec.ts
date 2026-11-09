import { dataNodesDocument } from '../workflow-data-document';
import { test, expect, type Locator, type Page } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parse } from 'yaml';
import { viewId } from '../../src/app/workflows/ui/editor/selection';
import { startDogfood } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';

const canvas=(page:Page)=>page.getByRole('region',{name:'Workflow canvas'});
const panel=(page:Page)=>page.getByRole('complementary',{name:'Inspector'});
const card=(page:Page,id:string)=>canvas(page).locator(`.react-flow__node[data-id="${viewId([],'node',id)}"]`);
async function activate(node:Locator) {await node.focus();await node.press('Enter');}
async function drag(page:Page,source:Locator,target:Locator) {
  const a=(await source.boundingBox())!,b=(await target.boundingBox())!;
  await page.mouse.move(a.x+a.width/2,a.y+a.height/2);await page.mouse.down();
  await page.mouse.move(b.x+b.width/2,b.y+b.height/2,{steps:16});
  await expect(target).toHaveClass(/connectingto/);await expect(target).toHaveClass(/\bvalid\b/);await page.mouse.up();
}


for (const width of [2000,390]) test(`data nodes edit, connect and persist with workspace Bash at ${width}px`,async({page})=>{
  page.setDefaultTimeout(15_000);await page.setViewportSize({width,height:width===390 ? 844:1100});
  const fixture=await startDogfood('web_console_dogfood'),errors:string[]=[];
  page.on('pageerror',error=>errors.push(error.message));page.on('console',message=>{if(message.type()==='error') errors.push(message.text());});
  try {
    const directory=join(fixture.workspaceA,'.agents/workflows');mkdirSync(directory,{recursive:true});
    const file=join(directory,'data_nodes.yaml');writeFileSync(file,JSON.stringify(dataNodesDocument()));
    await routeWorkspaceHost(page,fixture);await page.goto('/');await connectRemote(page,fixture.endpoint,fixture.token);await openEmptySession(page,fixture,'Workspace A');
    await page.getByRole('navigation',{name:'Primary navigation'}).getByRole('button',{name:'Workflows',exact:true}).click();
    await page.getByRole('button',{name:'data_nodes',exact:true}).click();await expect(page).toHaveTitle(/rustX$/);
    await expect(canvas(page).locator('.react-flow__node')).toHaveCount(9);

    // Template input keys use the existing versioned rename/draft transaction.
    await activate(card(page,'left'));
    await panel(page).getByRole('textbox',{name:'Variable name',exact:true}).fill('question');
    await panel(page).getByRole('textbox',{name:'Variable name',exact:true}).press('Enter');
    const template=panel(page).getByRole('textbox',{name:'Template code',exact:true});
    await template.fill('{{ question | upper }}!');await template.blur();
    await page.screenshot({path:`/tmp/rustx-data-template-${width}.png`});

    // Primitive list filtering needs no field path; operations have fixed order.
    await activate(card(page,'list'));
    await panel(page).getByRole('switch',{name:'Filter',exact:true}).click();
    await panel(page).getByRole('combobox',{name:'Operator',exact:true}).selectOption('greater_equal');
    const comparison=panel(page).getByRole('textbox',{name:'Comparison value',exact:true});await comparison.fill('2');await comparison.blur();
    await panel(page).getByRole('switch',{name:'Sort',exact:true}).click();
    await panel(page).getByRole('radio',{name:'Descending',exact:true}).click();
    await panel(page).getByRole('switch',{name:'Limit',exact:true}).click();
    const limit=panel(page).getByRole('textbox',{name:'Take first N items',exact:true});await limit.fill('2');await limit.press('Enter');
    await page.screenshot({path:`/tmp/rustx-data-list-${width}.png`});

    // A renamed grouped output rewrites the Return's structured selector.
    await activate(card(page,'merge'));
    await panel(page).getByRole('switch',{name:'Aggregation groups',exact:true}).click();
    const output=panel(page).getByRole('textbox',{name:'Output name',exact:true});await output.fill('answer');await output.press('Enter');
    await panel(page).getByRole('button',{name:'Choose a variable',exact:true}).first().click();
    const variables=page.getByRole('dialog',{name:'Choose a variable',exact:true});
    await expect(variables.getByRole('button',{name:/^left \/ output/})).toBeEnabled();
    await expect(variables.getByRole('button',{name:/^right \/ output/})).toBeEnabled();await page.keyboard.press('Escape');
    await page.screenshot({path:`/tmp/rustx-data-aggregate-${width}.png`});

    await activate(card(page,'classify'));
    const rename = panel(page).getByRole('button',{name:'Rename CLASS 2',exact:true});
    await rename.dblclick();
    const classLabel=panel(page).getByRole('textbox',{name:'Class label',exact:true});
    await classLabel.fill('General questions');await classLabel.press('Enter');
    await expect(card(page,'classify').locator('[data-handleid="class_2"]')).toHaveCount(1);
    if(width===2000) {
      await panel(page).getByRole('button',{name:'Disconnect right',exact:true}).click();
      await drag(page,card(page,'classify').locator('[data-handleid="class_2"]'),card(page,'right').locator('[data-handleid="in"]'));
    }
    await page.screenshot({path:`/tmp/rustx-data-classifier-${width}.png`});

    // Code has its own configuration and lowers to the native Bash Tool contract.
    await activate(card(page,'repeat'));
    await panel(page).getByRole('button',{name:'Add parallel node on Next step',exact:true}).click();
    const chooser=page.getByRole('dialog',{name:'Choose a workflow node',exact:true});
    for(const name of ['Template','List Operator','Question Classifier','Variable Aggregator','Code Execution']) await expect(chooser.getByRole('option',{name,exact:true})).toBeVisible();
    await expect(chooser.getByRole('option',{name:'Variable Assigner',exact:true})).toHaveCount(0);
    await chooser.getByRole('option',{name:'Code Execution',exact:true}).click();
    await expect(panel(page)).toContainText('Code runs in the Session workspace');
    const command=panel(page).getByRole('textbox',{name:'Code',exact:true});
    await expect(panel(page).getByRole('combobox',{name:'Language',exact:true})).toHaveValue('python3');
    await expect(command).toContainText('def main(arg1: str, arg2: str)');
    await expect(panel(page).getByRole('textbox',{name:'Variable name',exact:true})).toHaveCount(2);
    for(const index of [0,1]){await panel(page).getByRole('button',{name:'Choose a variable',exact:true}).nth(index).click();await page.getByRole('dialog',{name:'Choose a variable',exact:true}).getByRole('button',{name:/^args \/ query /}).click();}
    const script='def main(arg1, arg2):\n    return {"result":"node check"}\n';await command.fill(script);await command.blur();
    await panel(page).getByRole('button',{name:'Choose next node for Next step',exact:true}).click();
    await chooser.getByRole('tab',{name:'Existing',exact:true}).click();await chooser.locator('[data-node-id="done"]').click();
    await command.scrollIntoViewIfNeeded();
    await page.screenshot({path:`/tmp/rustx-data-code-${width}.png`});

    // Assignment is available in its real scope: a Loop body with variables.
    const inner=canvas(page).locator(`.react-flow__node[data-id="${viewId([{node:'repeat'}],'start')}"]`);
    const add=inner.locator('[data-handleid="next"] button');await add.focus();await add.press('Enter');
    const palette=(await page.getByRole('dialog').boundingBox())!;expect(palette.y+palette.height).toBeLessThanOrEqual(page.viewportSize()!.height);
    await page.getByRole('dialog').getByRole('option',{name:'Variable Assigner',exact:true}).click();
    await panel(page).getByRole('button',{name:'Add variable',exact:true}).click();
    await panel(page).getByRole('combobox',{name:'Operation',exact:true}).selectOption('increment');
    const save=page.getByRole('button',{name:'Save File',exact:true});await save.click();await expect(save).toBeDisabled({timeout:15_000});
    const saved=parse(readFileSync(file,'utf8'));
    expect(saved.block.nodes.left.variables.question.path).toEqual(['args','query']);
    expect(saved.block.nodes.left.template).toBe('{{ question | upper }}!');
    expect(saved.block.nodes.list).toMatchObject({filter:{operator:'greater_equal',value:{type:'literal',value:2}},sort:{order:'desc'},limit:2});
    expect(saved.block.nodes.done.outputs[0].value_selector).toEqual(['merge','answer','output']);
    expect(saved.block.edges).toContainEqual({from:'classify',to:'right',port:'class_2'});
    const code=Object.values(saved.block.nodes).find((node:any)=>node.type==='code') as any;
    expect(code.code_language).toBe('python3');expect(code.code).toBe(script);expect(code.outputs).toEqual({result:'string'});
    const assigner=Object.values(saved.block.nodes.repeat.body.nodes).find((node:any)=>node.type==='assigner') as any;
    expect(assigner.items[0]).toMatchObject({variable:['repeat','count'],operation:'increment'});
    await page.getByRole('button',{name:'Back to Workflows',exact:true}).first().click();await page.getByRole('button',{name:'data_nodes',exact:true}).click();
    await activate(card(page,'left'));await expect(panel(page).getByRole('textbox',{name:'Template code',exact:true}).locator('.cm-line')).toHaveText(['{{ question | upper }}!']);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);expect(errors).toEqual([]);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally {await fixture.stop(false);}
});
