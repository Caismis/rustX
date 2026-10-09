import { test, expect } from '@playwright/test';
import { openSettingsPage } from './shell-actions';

for (const width of [1440, 390]) test(`MCP compact creation form, reference JSON and save at ${width}px`, async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({width,height:1000});
  await page.addInitScript(() => localStorage.setItem('rustx-locale-v1','en'));
  await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/settings.html?scenario=mcp`);
  await expect(page).toHaveTitle('rustX native Settings reference');
  await page.getByRole('button',{name:'Settings',exact:true}).click();
  await page.getByRole('button',{name:'Dark',exact:true}).click();
  await openSettingsPage(page,'MCP servers');
  await page.getByRole('button',{name:'＋ New MCP server',exact:true}).click();
  const form=page.getByRole('form',{name:'New MCP server'});
  await expect(form.getByLabel('Command',{exact:true})).toBeVisible();
  await expect(form.getByLabel('Arguments (space-separated)',{exact:true})).toBeVisible();
  await expect(form.getByLabel('Command',{exact:true})).toHaveCSS('height','32px');
  await expect(form.getByLabel('Command',{exact:true})).toHaveCSS('font-size','14px');
  await expect(form.getByRole('button',{name:'Add Arguments'})).toHaveCount(0);
  await expect(form.getByText(/New User definition|Empty list/)).toHaveCount(0);
  const type=await form.getByRole('button',{name:'stdio (local command) Type',exact:true}).boundingBox();
  expect(type!.width).toBeLessThanOrEqual(192);
  if (width===1440) {
    expect((await form.getByLabel('Name',{exact:true}).boundingBox())!.width).toBe(192);
    expect((await form.boundingBox())!.height).toBeLessThan(450);
  }
  await expect(form.getByLabel('Environment variables (JSON)')).toBeHidden();
  await expect(form.getByLabel('Working directory')).toHaveCount(0);
  await page.screenshot({path:`/tmp/rustx-mcp-compact-${width}.png`});
  await form.getByLabel('Command',{exact:true}).fill('npx');
  await form.getByLabel('Name',{exact:true}).fill('browser-compact');
  await expect(form.getByLabel('Command',{exact:true})).toHaveValue('npx');
  await form.getByLabel('Arguments (space-separated)',{exact:true}).fill('-y server --directory "/a b" ""');
  await form.getByText('Environment references (optional)',{exact:true}).click();
  const references=form.getByLabel('Environment variables (JSON)');
  await references.fill('{broken');
  await expect(form.getByRole('button',{name:'Save',exact:true})).toBeDisabled();
  await references.fill('{"TOKEN":"$TOKEN"}');
  await form.getByRole('button',{name:'Save',exact:true}).click();
  await expect(page.getByRole('listitem',{name:'browser-compact'})).toBeVisible();
  await page.getByRole('button',{name:'MCP browser-compact',exact:true}).click();
  await page.getByRole('button',{name:'JSON',exact:true}).click();
  const json=page.getByLabel('Complete configuration');
  expect(JSON.parse(await json.inputValue())['browser-compact']).toMatchObject({command:'npx',args:['-y','server','--directory','/a b',''],sensitive_env:{TOKEN:'$TOKEN'}});
  await json.fill('{"type":"http","url":"https://example.com/mcp","sensitive_headers":{"Authorization":"$AUTH"}}');
  await page.getByRole('button',{name:'Save',exact:true}).click();
  await expect(page.getByRole('listitem',{name:'browser-compact'})).toContainText('https://example.com/mcp');
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('Chinese MCP reference states contain only the intended controls', async ({page}) => {
  await page.setViewportSize({width:1440,height:1000});
  await page.addInitScript(() => localStorage.setItem('rustx-locale-v1','zh'));
  await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/settings.html?scenario=mcp`);
  await page.getByRole('button',{name:'设置',exact:true}).click();
  await page.getByRole('button',{name:'深色',exact:true}).click();
  await page.getByRole('tab',{name:'MCP 服务器',exact:true}).click();
  const surface=page.locator('[data-mcp-page]');
  await expect(surface.getByText('尚未安装 MCP 服务器',{exact:true})).toBeVisible();
  await surface.screenshot({path:'/tmp/rustx-mcp-list-zh.png'});
  await page.getByRole('button',{name:'＋ 新建 MCP 服务器',exact:true}).click();
  const form=page.getByRole('form',{name:'新建 MCP 服务器'});
  await expect(form.getByLabel('名称',{exact:true})).toBeVisible();
  await expect(form.getByText(/高级选项|工作目录|保留现有|空列表|新建 用户 定义/)).toHaveCount(0);
  await surface.screenshot({path:'/tmp/rustx-mcp-form-zh.png'});
  // The native reference-only credential contract is intentionally unchanged.
  await form.getByText('环境变量引用（可选）',{exact:true}).click();
  await expect(form.getByLabel('环境变量（JSON）')).toBeVisible();
  await surface.screenshot({path:'/tmp/rustx-mcp-env-zh.png'});
  await page.getByRole('button',{name:'JSON',exact:true}).click();
  await expect(form.getByLabel('名称',{exact:true})).toHaveCount(0);
  await expect(form.getByLabel('完整配置')).toHaveValue(JSON.stringify({'my-mcp-server':{type:'stdio',command:'',args:[]}},null,2));
  await surface.screenshot({path:'/tmp/rustx-mcp-json-zh.png'});
  await form.getByRole('button',{name:'取消',exact:true}).click();
  await expect(surface.getByText('尚未安装 MCP 服务器',{exact:true})).toBeVisible();
});
