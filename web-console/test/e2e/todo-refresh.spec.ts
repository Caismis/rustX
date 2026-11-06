import { expect, test } from '@playwright/test';

const origin = 'http://127.0.0.1:' + (process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174);
for (const viewport of [{ width: 1280, height: 900 }, { width: 390, height: 844 }]) {
  for (const expanded of [false, true]) {
    test(`Todo refresh keeps its presentation stable (${viewport.width}px, expanded: ${expanded})`, async ({ page }) => {
      await page.setViewportSize(viewport);
      const errors: string[] = [];
      page.on('pageerror', error => errors.push(error.message));
      page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
      await page.goto(origin + '/test/fixtures/ui-continuity.html?scene=todo');
      await expect(page).toHaveTitle('UI continuity');
      await expect(page).toHaveURL(/ui-continuity\.html\?scene=todo$/);
      const todo = page.getByRole('region', { name: 'To-dos' });
      const header = todo.getByRole('button');
      await expect(header).toContainText('3 pending');
      if (expanded) await header.click();
      await header.focus();
      const label = await header.textContent();
      const geometry = await todo.boundingBox();
      await todo.locator('li').evaluateAll(rows => rows.forEach(row => row.setAttribute('data-retained-todo', 'true')));
      await todo.evaluate(element => {
        element.setAttribute('data-loading-flash', 'false');
        const loading = /Loading|加载中/;
        new MutationObserver(records => {
          if (loading.test(element.textContent ?? '') || records.some(record =>
            loading.test(record.oldValue ?? '') || [...record.addedNodes, ...record.removedNodes].some(node => loading.test(node.textContent ?? ''))
          )) element.setAttribute('data-loading-flash', 'true');
        }).observe(element, { childList: true, subtree: true, characterData: true, characterDataOldValue: true });
      });
      await page.evaluate(() => window.continuity.changeTodos());
      await expect.poll(() => page.evaluate(() => window.continuity.reads())).toBe(2);
      await expect(header).toHaveText(label!);
      await expect(header).toHaveAttribute('aria-expanded', String(expanded));
      await expect(header).toBeFocused();
      expect(await todo.boundingBox()).toEqual(geometry);
      await expect(todo.locator('[data-retained-todo]')).toHaveCount(expanded ? 3 : 0);
      await expect(todo.getByRole('status')).toHaveCount(0);
      await page.evaluate(() => window.continuity.finishTodos());
      await expect(header).toContainText('1 completed');
      await expect(header).toContainText('3 pending');
      await expect(header).toHaveAttribute('aria-expanded', String(expanded));
      await expect(header).toBeFocused();
      await expect(todo.locator('[data-retained-todo]')).toHaveCount(expanded ? 3 : 0);
      if (expanded) {
        await expect(todo.locator('li')).toHaveCount(4);
        await expect(todo.locator('li').first()).toHaveAttribute('data-status', 'completed');
        await expect(todo.getByText('New task after refresh')).toBeVisible();
      }
      await expect(todo).toHaveAttribute('data-loading-flash', 'false');
      await expect(page.locator('vite-error-overlay')).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      expect(errors).toEqual([]);
      if (expanded) await page.screenshot({ path: `/tmp/rustx-todo-refresh-${viewport.width}.png` });
    });
  }
}
