import AxeBuilder from '@axe-core/playwright';
import {expect,test,type Locator,type Page} from '@playwright/test';
import {existsSync,mkdirSync,readFileSync,symlinkSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {startDogfood} from './dogfood-server';
import {routeWorkspaceHost} from './workspace-host';
import {confirmSettingsAction,connectRemote,openSettingsPage,returnHome,settledSave} from './shell-actions';
import {captureEvidence} from './screenshot';
import {wireProbe} from './wire-probe';

async function workspace(page:Page,scope:Locator) {
 await scope.getByRole('tab',{name:'Workspace',exact:true}).click();
 await scope.getByRole('button',{name:'Choose Workspace',exact:true}).click();
 await page.getByRole('menuitem',{name:'Workspace A',exact:true}).click();
}
async function checkCatalogView(scope: Locator, item: Locator) {
  const list = scope.getByRole('button', { name: 'List view', exact: true });
  await list.click();
  await expect(list).toHaveAttribute('aria-pressed', 'true');
  await expect(item).toBeVisible();
  const cards = scope.getByRole('button', { name: 'Card view', exact: true });
  await cards.click();
  await expect(cards).toHaveAttribute('aria-pressed', 'true');
  await expect(item).toBeVisible();
  expect(await scope.locator(':scope > div').evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
}

for(const width of [1440,390]) test(`independent resources have exact global and workspace toggles, and read-only skills at ${width}px`,async({page})=>{
 const f=await startDogfood();
 const wire=await wireProbe(page);
 const errors:string[]=[];
 page.on('pageerror',error=>errors.push(error.message));
 page.on('console',event=>{if(['error','warning'].includes(event.type()))errors.push(event.text());});
 try {
  const userRoot=join(f.directory,'home/rustx/.agents');
  const workspaceRoot=join(f.workspaceA,'.agents');
  const mcpScript=fileURLToPath(new URL('./image-mcp.py',import.meta.url));
  for(const [root,prefix] of [[userRoot,'global'],[workspaceRoot,'local']]) {
   const skill = join(root,'skills','team',`${prefix}-guide`);
   mkdirSync(join(root,'agents'),{recursive:true});mkdirSync(skill,{recursive:true});
   writeFileSync(join(root,'agents',`${prefix}-reviewer.toml`),`description = "${prefix} reviewer"\ninstructions = "Review carefully."\n`);
   const skillFile=join(skill,'SKILL.md');
   const instructionFile=prefix==='global'?join(f.directory,'shared-global-guide.md'):skillFile;
   writeFileSync(instructionFile,`---\nname: ${prefix}-guide\ndescription: A ${prefix} skill for reviewing changes.\n---\n# ${prefix} guide\n${prefix} guide instructions.\n`);
   if(instructionFile!==skillFile)symlinkSync(instructionFile,skillFile);
   writeFileSync(join(root,'mcp.toml'),`[mcp_servers.${prefix}-server]\ntype = "stdio"\ncommand = "python3"\nargs = [${JSON.stringify(mcpScript)}]\n`);
  }
  const projectSkills=join(workspaceRoot,'skills','manual');
  mkdirSync(projectSkills,{recursive:true});
  writeFileSync(join(projectSkills,'manual.md'),'---\nname: manual-guide\ndescription: Manual skill.\ndisable-model-invocation: true\n---\n# Manual\nInstructions in a standalone file.');
  // Descriptions beyond the native 32-item page must remain discoverable.
  for(let index=0;index<34;index++) {
   const name=`catalog-${String(index).padStart(2,'0')}`,directory=join(userRoot,'skills',name);
   mkdirSync(directory,{recursive:true});
   writeFileSync(join(directory,'SKILL.md'),`---\nname: ${name}\ndescription: Catalog entry ${index}.\n---\n# ${name}\nInstructions.\n`);
  }
  writeFileSync(f.settings,readFileSync(f.settings,'utf8')+'\n[agent]\nskills = ["global-guide"]\nagents = ["global-reviewer"]\n');
  await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme:'dark',reducedMotion:'reduce'});
  await routeWorkspaceHost(page,f);await page.goto('/');await connectRemote(page,f.endpoint,f.token);
  const draft=page.getByRole('textbox',{name:'Message',exact:true});await draft.fill('Keep my conversation draft');
  await page.getByRole('button',{name:'Skills',exact:true}).click();
  const skills=page.getByRole('region',{name:'Skills',exact:true});
  await skills.getByRole('tab',{name:'Global',exact:true}).click();
  await expect(skills.getByRole('heading',{name:'Skills',exact:true})).toBeVisible();
  await skills.getByRole('searchbox').fill('catalog-33');
  await expect(skills.getByRole('listitem',{name:'catalog-33',exact:true})).toContainText('Catalog entry 33.');
  await skills.getByRole('searchbox').fill('guide');
  const globalSkill=skills.getByRole('listitem',{name:'global-guide',exact:true});
  await expect(globalSkill).toContainText('A global skill for reviewing changes.');
  await expect(globalSkill.getByRole('switch')).toBeChecked();
  await globalSkill.getByRole('button',{name:'global-guide',exact:true}).click();
  const detail=page.getByRole('dialog',{name:'global-guide',exact:true});
  await expect(detail).toContainText('global guide instructions.');
  await expect(detail.getByRole('textbox')).toHaveCount(0);await expect(detail.locator('.cm-editor')).toHaveCount(0);
  await expect(detail.getByRole('button',{name:/Save|Delete|Edit/})).toHaveCount(0);
  await captureEvidence(detail,`skill-read-only-${width}`);
  await detail.getByRole('button',{name:'Close',exact:true}).click();
  await workspace(page,skills);
  const manual=skills.getByRole('listitem',{name:'manual-guide',exact:true});
  await expect(manual).toContainText('Manual skill.');
  await expect(manual.getByRole('switch')).toHaveCount(0);
  await manual.getByRole('button',{name:'manual-guide',exact:true}).click();
  const manualDetail=page.getByRole('dialog',{name:'manual-guide',exact:true});
  await expect(manualDetail).toContainText('Instructions in a standalone file.');
  await manualDetail.getByRole('button',{name:'Close',exact:true}).click();
  const inheritedSkill=skills.getByRole('listitem',{name:'global-guide',exact:true});
  await expect(inheritedSkill.getByRole('switch')).toBeChecked();
  await inheritedSkill.getByRole('switch').click();
  const workspaceSettings=join(f.workspaceA,'rustx.toml');
  const workspaceDocument=()=>existsSync(workspaceSettings)?readFileSync(workspaceSettings,'utf8'):'';
  await expect.poll(workspaceDocument).toContain('skills = []');
  await expect(inheritedSkill.getByRole('switch')).not.toBeChecked();
  const localSkill=skills.getByRole('listitem',{name:'local-guide',exact:true});
  await expect(localSkill.getByRole('switch')).toBeEnabled();await localSkill.getByRole('switch').click();
  await expect.poll(workspaceDocument).toContain('skills = ["local-guide"]');
  expect(readFileSync(f.settings,'utf8')).toContain('skills = ["global-guide"]');
  await expect(localSkill.getByRole('switch')).toBeEnabled();
  await checkCatalogView(skills, localSkill);
  await captureEvidence(skills,`workspace-skills-${width}`);
  await skills.getByRole('tab',{name:'Global',exact:true}).click();
  await expect(globalSkill.getByRole('switch')).toBeChecked();

  await page.getByRole('button',{name:'Manage subagents',exact:true}).click();
  const agents=page.getByRole('region',{name:'Subagents',exact:true});
  await agents.getByRole('tab',{name:'Global',exact:true}).click();
  const globalAgent=agents.getByRole('listitem',{name:'global-reviewer',exact:true});
  await expect(globalAgent.getByRole('switch')).toBeChecked();await globalAgent.getByRole('switch').click();
  await expect.poll(()=>readFileSync(f.settings,'utf8')).toContain('agents = []');
  await agents.getByRole('button',{name:'New subagent',exact:true}).click();
  const naming=page.getByRole('dialog',{name:'New subagent',exact:true});
  await naming.getByLabel('Name',{exact:true}).fill('Invalid Name');
  await expect(naming.getByRole('button',{name:'Continue',exact:true})).toBeDisabled();
  await naming.getByLabel('Name',{exact:true}).fill('auditor');
  await naming.getByRole('button',{name:'Continue',exact:true}).click();
  await expect(agents.getByRole('switch')).toHaveCount(0);
  await agents.getByLabel('Description',{exact:true}).fill('Reviews code');
  await agents.getByLabel('Instructions',{exact:true}).fill('Check the changes carefully.');
  await agents.getByRole('button',{name:'Save Agent auditor',exact:true}).click();
  await expect(settledSave(agents, 'Agent auditor')).toBeVisible();
  expect(readFileSync(join(userRoot,'agents/auditor.toml'),'utf8')).toContain('Check the changes carefully.');
  await agents.getByRole('button',{name:'Back to list',exact:true}).click();
  await agents.getByRole('listitem',{name:'auditor',exact:true}).getByRole('switch').click();
  await expect.poll(()=>readFileSync(f.settings,'utf8')).toContain('agents = ["auditor"]');
  await workspace(page,agents);
  await agents.getByRole('listitem',{name:'global-reviewer',exact:true}).getByRole('switch').click();
  await expect.poll(workspaceDocument).toContain('global-reviewer');
  expect(readFileSync(f.settings,'utf8')).toContain('agents = ["auditor"]');
  await expect(agents.getByRole('listitem',{name:'global-reviewer',exact:true}).getByRole('switch')).toBeEnabled();
  await checkCatalogView(agents, agents.getByRole('listitem',{name:'global-reviewer',exact:true}));
  await captureEvidence(agents,`workspace-subagents-${width}`);

  await page.getByRole('button',{name:'MCP servers',exact:true}).click();
  const mcp=page.getByRole('region',{name:'MCP servers',exact:true});
  await mcp.getByRole('tab',{name:'Global',exact:true}).click();
  const server=mcp.getByRole('listitem',{name:'global-server',exact:true});
  await expect(server.getByRole('switch')).not.toBeChecked();await server.getByRole('switch').click();
  await expect.poll(()=>readFileSync(f.settings,'utf8')).toContain('global-server = "all"');
  await workspace(page,mcp);
  const inheritedServer=mcp.getByRole('listitem',{name:'global-server',exact:true});
  await expect(inheritedServer.getByRole('switch')).toBeChecked();await inheritedServer.getByRole('switch').click();
  await expect.poll(workspaceDocument).toContain('global-server = []');
  expect(readFileSync(f.settings,'utf8')).toContain('global-server = "all"');
  await expect(inheritedServer.getByRole('switch')).toBeEnabled();
  await checkCatalogView(mcp, inheritedServer);
  await captureEvidence(mcp,`workspace-mcp-${width}`);
  const globalMcp=readFileSync(join(userRoot,'mcp.toml'),'utf8');
  await inheritedServer.getByRole('button',{name:'MCP global-server',exact:true}).click();
  await expect(mcp.getByLabel('Command',{exact:true})).toBeDisabled();
  await expect(mcp.getByRole('button',{name:'Save',exact:true})).toBeDisabled();
  await expect(mcp.getByRole('button',{name:'JSON',exact:true})).toBeDisabled();
  await mcp.getByRole('button',{name:'Override',exact:true}).click();
  await expect(mcp.getByLabel('Command',{exact:true})).toBeEnabled();
  await mcp.getByRole('button',{name:'Save',exact:true}).click();
  await expect(mcp.getByRole('listitem',{name:'global-server',exact:true})).toBeVisible();
  expect(readFileSync(join(workspaceRoot,'mcp.toml'),'utf8')).toContain('[mcp_servers.global-server]');
  expect(readFileSync(join(userRoot,'mcp.toml'),'utf8')).toBe(globalMcp);
  await mcp.getByRole('button',{name:'MCP global-server',exact:true}).click();
  await confirmSettingsAction(page,'Use global default MCP global-server');
  await expect(mcp.getByRole('group',{name:'Available from Global',exact:true}).getByRole('listitem',{name:'global-server',exact:true})).toBeVisible();
  expect(readFileSync(join(workspaceRoot,'mcp.toml'),'utf8')).not.toContain('[mcp_servers.global-server]');
  expect(readFileSync(join(userRoot,'mcp.toml'),'utf8')).toBe(globalMcp);
  const accessibility=await new AxeBuilder({page}).include('[data-resource-module="mcp"]').withTags(['wcag2a','wcag2aa','wcag21aa']).analyze();
  expect(accessibility.violations.filter(row=>['serious','critical'].includes(row.impact??'')).map(row=>row.id)).toEqual([]);
  // Returning to a module resumes its own scope, independently of Settings.
  await page.getByRole('button',{name:'Skills',exact:true}).click();
  await expect(skills.getByRole('tab',{name:'Global',exact:true})).toHaveAttribute('aria-selected','true');
  await page.getByRole('button',{name:'Settings',exact:true}).click();
  const settings=page.getByRole('region',{name:'Settings',exact:true});
  for(const name of ['MCP servers','Skills','Subagents'])await expect(settings.getByRole('tab',{name,exact:true})).toHaveCount(0);
  await openSettingsPage(page,'Tools & Permissions');
  await expect(settings.getByRole('form',{name:'Available skills',exact:true})).toHaveCount(0);
  await expect(settings.getByRole('form',{name:'Delegated agents',exact:true})).toHaveCount(0);
  await returnHome(page);await expect(draft).toHaveValue('Keep my conversation draft');
  expect(wire.requests.filter(request=>request.method==='thread/start')).toHaveLength(0);
  expect(errors).toEqual([]);
 } catch(error) {
  await test.info().attach('native-diagnostics',{body:JSON.stringify(f.diagnostics()),contentType:'application/json'});throw error;
 } finally {await f.stop(false);}
});
