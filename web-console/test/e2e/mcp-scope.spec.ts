import {test,expect} from '@playwright/test';
import {openResourceModule,openSettingsPage} from './shell-actions';
import {captureEvidence} from './screenshot';
const fixture=`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT??5174}/test/fixtures/settings.html?scenario=mcp-scope`;
test('MCP keeps its resource scope separate from Settings and opens global definitions for editing',async({page})=>{
 const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message));
 await page.goto(fixture);
 const mcp=await openResourceModule(page,'mcp','Global');
 await expect(mcp.getByRole('listitem',{name:'exa',exact:true})).toBeVisible();
 await openResourceModule(page,'mcp','Workspace A');
 const inherited=mcp.getByRole('group',{name:'Available from Global',exact:true});
 await expect(inherited.getByRole('listitem',{name:'exa',exact:true})).toBeVisible();
 await expect(inherited.getByRole('button',{name:'Test connection exa',exact:true})).toBeDisabled();
 const search=mcp.getByRole('searchbox');
 await search.fill('missing');
 await expect(inherited).toHaveCount(0);
 await search.clear();
 await expect(inherited.getByRole('listitem',{name:'exa',exact:true})).toBeVisible();
 await openSettingsPage(page,'General');
 // General is webui-only and carries no scope segment; the resource module
 // must keep its own selection regardless.
 await expect(page.getByRole('region',{name:'Settings',exact:true}).getByRole('heading',{name:'General',exact:true})).toBeVisible();
 await openResourceModule(page,'mcp');
 await expect(mcp.getByRole('tab',{name:'Workspace',exact:true})).toHaveAttribute('aria-selected','true');
 await inherited.getByRole('button',{name:'MCP exa',exact:true}).click();
 await expect(mcp.getByLabel('URL',{exact:true})).toBeEnabled();
 await expect(mcp.getByLabel('URL',{exact:true})).toHaveValue('https://mcp.exa.ai/mcp');
 await expect(mcp.getByRole('button',{name:'Override',exact:true})).toHaveCount(0);
 await expect(mcp.getByRole('button',{name:'Save',exact:true})).toBeDisabled();
 await captureEvidence(mcp,'mcp-inherited-workspace');
 expect(errors).toEqual([]);
});
test('MCP module shares compact typography and fits a narrow page',async({page})=>{
 await page.addInitScript(() => localStorage.setItem('rustx-appearance-v1','dark'));
 await page.goto(fixture);await openSettingsPage(page,'General');
 const mcp=await openResourceModule(page,'mcp','Global');
 await mcp.getByRole('button',{name:'MCP exa',exact:true}).click();
 await expect(mcp.locator('h3')).toHaveCSS('font-size','20px');
 await expect(mcp.getByRole('button',{name:'Form',exact:true})).toHaveAttribute('aria-pressed','true');
 await mcp.getByRole('button',{name:'JSON',exact:true}).click();
 await page.setViewportSize({width:390,height:850});
 await expect(mcp.locator('h3')).toHaveCSS('font-size','20px');
 await captureEvidence(mcp,'mcp-editor-narrow');
 const bounds=await mcp.evaluate(el=>({client:el.clientWidth,scroll:el.scrollWidth}));
 expect(bounds.scroll).toBeLessThanOrEqual(bounds.client);
 // scrollWidth stays quiet when a clipped ancestor hides overflow, so also
 // walk every rendered element: none may escape the page's box sideways
 // (e.g. slide under the primary nav rail).
 const escaped=await mcp.evaluate(root=>{
  const box=root.getBoundingClientRect();
  return [...root.querySelectorAll('*')].filter(el=>{
   const rect=el.getBoundingClientRect();
   if(rect.width===0)return false;
   for(let at:Element|null=el;at&&at!==root;at=at.parentElement){
    const style=getComputedStyle(at);
    if(at!==el&&/auto|scroll/.test(style.overflowX))return false;
    if(style.clipPath!=='none'||(style.clip!=='auto'&&style.clip!==''))return false;
   }
   return rect.left<box.left-1||rect.right>box.right+1;
  }).map(el=>`${el.tagName.toLowerCase()} ${el.getAttribute('aria-label')??el.textContent?.slice(0,40)}`);
 });
 expect(escaped).toEqual([]);
});
