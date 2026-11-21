import { test, expect } from '@playwright/test';
import { openResourceModule } from './shell-actions';

test('MCP Back guards unsaved JSON and footer Cancel resets the JSON and form together', async ({ page }) => {
  await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/settings.html?scenario=mcp`);
  const surface = await openResourceModule(page, 'mcp', 'Global');
  await surface.getByRole('button', { name: '＋ New MCP server', exact: true }).click();
  await surface.getByLabel('Name', { exact: true }).fill('draft-mcp');
  await surface.getByLabel('Command', { exact: true }).fill('initial');
  await surface.getByRole('button', { name: 'Save', exact: true }).click();
  const server = surface.getByRole('button', { name: 'MCP draft-mcp', exact: true });
  await server.click();
  await surface.getByLabel('Command', { exact: true }).fill('form-draft');
  await surface.getByRole('button', { name: 'JSON', exact: true }).click();
  const json = surface.getByLabel('Complete configuration', { exact: true });
  await json.fill('{unfinished');
  // Staged edits guard the detail's back gesture; keeping them stays put.
  await surface.getByRole('button', { name: 'MCP servers', exact: true }).click();
  const modal = page.getByRole('dialog');
  await expect(modal).toBeVisible();
  await modal.getByRole('button', { name: 'Keep editing', exact: true }).click();
  await expect(json).toHaveText('{unfinished', { useInnerText: true });
  await expect(surface.getByRole('button', { name: 'Cancel', exact: true })).toBeEnabled();
  // The footer Cancel discards the staged draft back to the committed config.
  await surface.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect.poll(async () => JSON.parse(await json.innerText())['draft-mcp'].command).toBe('initial');
  await expect(surface.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
  await surface.getByRole('button', { name: 'Form', exact: true }).click();
  await expect(surface.getByLabel('Command', { exact: true })).toHaveValue('initial');
  const references = surface.getByLabel('Environment variables (JSON)', { exact: true });
  await surface.getByText('Environment references (optional)', { exact: true }).click();
  // CodeMirror owns its selection: select-all + insertText lands the staged value.
  await references.press('ControlOrMeta+a');
  await page.keyboard.insertText('{unfinished');
  await expect(references).toHaveText('{unfinished', { useInnerText: true });
  await expect(surface.getByRole('button', { name: 'Cancel', exact: true })).toBeEnabled();
  await surface.getByRole('button', { name: 'Cancel', exact: true }).click();
  await surface.getByText('Environment references (optional)', { exact: true }).click();
  const referenceEditor = surface.locator('[data-json-editor]').filter({ has: page.getByRole('textbox', { name: 'Environment variables (JSON)', exact: true }) });
  await expect(referenceEditor).toHaveAttribute('data-empty', 'true');
  await expect(references).toHaveAttribute('aria-invalid', 'false');
});

for (const width of [1440, 390]) test(`MCP compact creation form, reference JSON and save at ${width}px`, async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({width,height:1000});
  await page.addInitScript(() => { localStorage.setItem('rustx-locale-v1','en'); localStorage.setItem('rustx-appearance-v1','dark'); });
  await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/settings.html?scenario=mcp`);
  await expect(page).toHaveTitle('Session A — rustX');
  await openResourceModule(page,'mcp','Global');
  await page.getByRole('button',{name:'＋ New MCP server',exact:true}).click();
  const form=page.getByRole('form',{name:'New MCP server'});
  await expect(page.getByRole('heading',{name:'New MCP server',exact:true,level:3})).toBeFocused();
  await expect(page.getByRole('tab',{name:'Global',exact:true})).toHaveCount(0);
  await expect(form.getByLabel('Command',{exact:true})).toBeVisible();
  await expect(form.getByLabel('Arguments (space-separated)',{exact:true})).toBeVisible();
  await expect(form.getByLabel('Command',{exact:true})).toHaveCSS('height','36px');
  await expect(form.getByLabel('Command',{exact:true})).toHaveCSS('font-size','13px');
  await expect(form.getByRole('button',{name:'Add Arguments'})).toHaveCount(0);
  await expect(form.getByText(/New User definition|Empty list/)).toHaveCount(0);
  const type=await form.getByRole('button',{name:'stdio (local command) Type',exact:true}).boundingBox();
  expect(type!.width).toBeLessThanOrEqual(240);
  if (width===1440) {
    expect((await form.getByLabel('Name',{exact:true}).boundingBox())!.width).toBe(240);
    expect((await form.boundingBox())!.height).toBeLessThan(480);
  }
  await expect(form.getByLabel('Environment variables (JSON)',{exact:true})).toBeHidden();
  await expect(form.getByLabel('Working directory')).toHaveCount(0);
  await page.screenshot({path:`/tmp/rustx-mcp-compact-${width}.png`});
  await form.getByLabel('Command',{exact:true}).fill('npx');
  await form.getByLabel('Name',{exact:true}).fill('browser-compact');
  await expect(form.getByLabel('Command',{exact:true})).toHaveValue('npx');
  await form.getByLabel('Arguments (space-separated)',{exact:true}).fill('-y server --directory "/a b" ""');
  await form.getByText('Environment references (optional)',{exact:true}).click();
  const references=form.getByLabel('Environment variables (JSON)',{exact:true});
  const referenceEditor=form.locator('[data-json-editor]').filter({has:page.getByRole('textbox',{name:'Environment variables (JSON)',exact:true})});
  await expect(referenceEditor.locator('.cm-gutters')).toHaveCount(0);
  await expect(referenceEditor.getByRole('button',{name:'Format JSON: Environment variables (JSON)',exact:true})).toBeHidden();
  await references.fill('{broken');
  const saveButton=page.getByRole('button',{name:'Save',exact:true});
  await expect(saveButton).toBeEnabled();
  await saveButton.scrollIntoViewIfNeeded();
  await expect(saveButton).toBeInViewport({ratio:1});
  await expect(references).toHaveAttribute('aria-invalid','true');
  await expect(form.locator('.cm-lintRange-error')).not.toHaveCount(0);
  await references.fill('{"TOKEN":"$TOKEN"}');
  await form.getByLabel('Command',{exact:true}).click();
  await expect(references).toHaveText('{\n  "TOKEN": "$TOKEN"\n}',{useInnerText:true});
  await page.getByRole('button',{name:'Save',exact:true}).click();
  await expect(page.getByRole('listitem',{name:'browser-compact'})).toBeVisible();
  await page.getByRole('button',{name:'MCP browser-compact',exact:true}).click();
  await expect(page.getByRole('region',{name:'MCP servers',exact:true}).getByText('Global',{exact:true}).first()).toBeVisible();
  await page.getByRole('button',{name:'JSON',exact:true}).click();
  const json=page.getByLabel('Complete configuration',{exact:true});
  const completeEditor=page.locator('[data-json-editor]').filter({has:json});
  expect(JSON.parse(await json.innerText())['browser-compact']).toMatchObject({command:'npx',args:['-y','server','--directory','/a b',''],sensitive_env:{TOKEN:'$TOKEN'}});
  await json.fill('');
  await expect(page.getByRole('button',{name:'Save',exact:true})).toBeEnabled();
  await expect(json).toHaveAttribute('aria-invalid','true');
  await expect(completeEditor).toHaveAttribute('data-empty','true');
  await expect(completeEditor.locator('.cm-gutters')).toHaveCount(0);
  await json.fill('{"type":');
  await expect(page.getByRole('button',{name:'Save',exact:true})).toBeEnabled();
  await json.fill('{"type":"http","url":"https://example.com/mcp","sensitive_headers":{"Authorization":"$AUTH"}}');
  await page.getByRole('button',{name:'Format JSON: Complete configuration',exact:true}).click();
  await expect(json).toHaveText(JSON.stringify({type:'http',url:'https://example.com/mcp',sensitive_headers:{Authorization:'$AUTH'}},null,2),{useInnerText:true});
  await page.getByRole('button',{name:'Save',exact:true}).click();
  await expect(page.getByRole('listitem',{name:'browser-compact'})).toContainText('https://example.com/mcp');
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('Chinese MCP reference states contain only the intended controls', async ({page}) => {
  await page.setViewportSize({width:1440,height:1000});
  await page.addInitScript(() => { localStorage.setItem('rustx-locale-v1','zh'); localStorage.setItem('rustx-appearance-v1','dark'); });
  await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/settings.html?scenario=mcp`);
  await page.getByRole('button',{name:'MCP 服务器',exact:true}).click();
  const surface=page.getByRole('region',{name:'MCP 服务器',exact:true});
  const listBack=surface.getByRole('button',{name:'MCP 服务器',exact:true});
  if (await listBack.isVisible()) await listBack.click();
  await page.getByRole('tab',{name:'全局',exact:true}).click();
  await expect(surface.getByText('尚未安装 MCP 服务器',{exact:true})).toBeVisible();
  await surface.screenshot({path:'/tmp/rustx-mcp-list-zh.png'});
  await page.getByRole('button',{name:'＋ 新建 MCP 服务器',exact:true}).click();
  const form=page.getByRole('form',{name:'新建 MCP 服务器'});
  await expect(form.getByLabel('名称',{exact:true})).toBeVisible();
  await expect(form.getByText(/高级选项|工作目录|保留现有|空列表|新建 用户 定义/)).toHaveCount(0);
  await surface.screenshot({path:'/tmp/rustx-mcp-form-zh.png'});
  // The native reference-only credential contract is intentionally unchanged.
  await form.getByText('环境变量引用（可选）',{exact:true}).click();
  await expect(form.getByLabel('环境变量（JSON）',{exact:true})).toBeVisible();
  await surface.screenshot({path:'/tmp/rustx-mcp-env-zh.png'});
  await page.getByRole('button',{name:'JSON',exact:true}).click();
  await expect(form.getByLabel('名称',{exact:true})).toHaveCount(0);
  await expect(form.getByLabel('完整配置',{exact:true})).toHaveText(JSON.stringify({'my-mcp-server':{type:'stdio',command:'',args:[]}},null,2), {useInnerText:true});
  await surface.screenshot({path:'/tmp/rustx-mcp-json-zh.png'});
  await surface.getByRole('button',{name:'MCP 服务器',exact:true}).click();
  const guardDialog=page.getByRole('dialog');
  if (await guardDialog.isVisible()) await guardDialog.getByRole('button',{name:'放弃并离开',exact:true}).click();
  await expect(surface.getByText('尚未安装 MCP 服务器',{exact:true})).toBeVisible();
});

for (const width of [1440, 390]) test(`MCP connectivity indicators match across scopes at ${width}px`, async ({page}) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({width,height:1000});
  await page.addInitScript(({ width }) => { localStorage.setItem('rustx-locale-v1','en'); localStorage.setItem('rustx-appearance-v1', width === 1440 ? 'dark' : 'light'); }, { width });
  await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/settings.html?scenario=mcp-probe`);
  await openResourceModule(page,'mcp','Global');
  const region=page.getByRole('region',{name:'MCP servers',exact:true});
  const surface=region.getByRole('group',{name:'Installed',exact:true});
  for (const scope of ['User','Workspace A']) {
    if(scope !== 'User') {
      await openResourceModule(page,'mcp','Workspace A');
    }
    const inherited = region.getByRole('group',{name:'Available from Global',exact:true});
    if (scope === 'Workspace A') {
      for (const name of ['exa','broken']) {
        await expect(inherited.getByRole('listitem',{name,exact:true}).locator('[data-status]')).toHaveAttribute('data-status','unknown');
        await expect(inherited.getByRole('button',{name:`Test connection ${name}`,exact:true})).toBeDisabled();
      }
    }
    await expect(surface.getByRole('listitem',{name:'exa'}).locator('[data-status]')).toHaveAttribute('data-status','unknown');
    await surface.getByRole('button',{name:'Test connection exa',exact:true}).click();
    await surface.getByRole('button',{name:'Test connection broken',exact:true}).click();
    await expect(surface.getByRole('listitem',{name:'exa'}).locator('[data-status]')).toHaveAttribute('data-status','reachable');
    await expect(surface.getByRole('listitem',{name:'broken'}).locator('[data-status]')).toHaveAttribute('data-status','connection_failed');
    await expect(surface.getByRole('listitem',{name:'exa'}).locator('[data-status]')).toHaveCSS('background-color',await page.locator('body').evaluate(el=>getComputedStyle(el).getPropertyValue('--dsw-alias-state-success-primary').trim()));
    await page.getByRole('region',{name:'MCP servers',exact:true}).getByRole('button',{name:'Refresh',exact:true}).click();
    await expect(page.getByRole('region',{name:'MCP servers',exact:true}).getByRole('button',{name:'Refresh',exact:true})).toBeEnabled();
    await expect(surface.getByRole('img',{name:'Connection test succeeded.'})).toBeVisible();
    if (scope === 'Workspace A') {
      // Global cards cannot inherit a Workspace-scoped probe result.
      for (const name of ['exa','broken']) await expect(inherited.getByRole('listitem',{name,exact:true}).locator('[data-status]')).toHaveAttribute('data-status','unknown');
    }
  }
  await region.screenshot({path:`/tmp/rustx-mcp-probes-${width}.png`});
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
  expect(errors).toEqual([]);
});
