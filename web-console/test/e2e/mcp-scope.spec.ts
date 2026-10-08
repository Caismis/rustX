import { test, expect } from '@playwright/test';
test('MCP scope shows only definitions owned by the selected scope', async ({page})=>{
 const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message));
 await page.addInitScript(()=>localStorage.setItem('rustx-locale-v1','en'));
 await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT??5174}/test/fixtures/settings.html?scenario=mcp-scope`);
 await page.getByRole('button',{name:'Settings',exact:true}).click();
 await page.getByRole('tab',{name:'MCP servers',exact:true}).click();
 await expect(page.getByRole('listitem',{name:'exa',exact:true})).toBeVisible();
 await page.getByRole('button',{name:'Configuration scope'}).click();
 await page.getByRole('menuitem',{name:'Workspace A',exact:true}).click();
 await expect(page.getByText('No MCP servers installed',{exact:true})).toBeVisible();
 await expect(page.getByRole('listitem',{name:'exa',exact:true})).toHaveCount(0);
 await expect(page.getByText('Inherited from user',{exact:true})).toHaveCount(0);
 await expect(page.locator('[data-mcp-page] [class*="total"]')).toHaveText('MCP 0');
 await page.screenshot({path:'/tmp/rustx-mcp-workspace-scope.png'});
 await page.getByRole('button',{name:'Configuration scope'}).click();
 await page.getByRole('menuitem',{name:'User (global)',exact:true}).click();
 await expect(page.getByRole('listitem',{name:'exa',exact:true})).toBeVisible();
 expect(errors).toEqual([]);
});

test('MCP settings share compact settings typography and fit narrow panels', async ({page})=>{
 await page.addInitScript(()=>localStorage.setItem('rustx-locale-v1','en'));
 await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT??5174}/test/fixtures/settings.html?scenario=mcp-scope`);
 await page.getByRole('button',{name:'Settings',exact:true}).click();
 await page.getByRole('tab',{name:'MCP servers',exact:true}).click();
 await expect(page.locator('[data-mcp-page] h3')).toHaveCSS('font-size','20px');
 await page.screenshot({path:'/tmp/rustx-mcp-list-light.png'});
 await page.emulateMedia({colorScheme:'dark'});
 // MCP inherits the dialog material, like every other settings page.
 const surface=page.locator('[data-mcp-page]').locator('..');
 await expect(surface).toHaveCSS('background-color','rgba(0, 0, 0, 0)');
 await expect(page.getByRole('listitem',{name:'exa',exact:true})).toHaveCSS('background-color','rgb(44, 44, 46)');
 await page.screenshot({path:'/tmp/rustx-mcp-list-dark.png'});
 await page.getByRole('listitem',{name:'exa',exact:true}).getByRole('button').first().click();
 await expect(page.getByRole('button',{name:'Form',exact:true})).toHaveAttribute('aria-pressed','true');
 await page.screenshot({path:'/tmp/rustx-mcp-editor-dark.png'});
 await page.getByRole('button',{name:'JSON',exact:true}).click();
 await expect(page.getByRole('button',{name:'JSON',exact:true})).toHaveAttribute('aria-pressed','true');
 await page.setViewportSize({width:600,height:850});
 await page.screenshot({path:'/tmp/rustx-mcp-editor-narrow.png'});
 const bounds=await page.locator('[data-mcp-page]').evaluate(el=>({client:el.clientWidth,scroll:el.scrollWidth}));
 expect(bounds.scroll).toBeLessThanOrEqual(bounds.client);
});
