import { expect, type Locator, type Page } from '@playwright/test';

export const structuredDialog = (page: Page) => page.getByRole('dialog', { name: 'Structured output Schema', exact: true });
export async function schemaJson(dialog: Locator) {
  await dialog.getByRole('tab', { name: 'JSON Schema', exact: true }).click();
  return dialog.getByRole('textbox', { name: 'JSON', exact: true });
}
export async function replaceJson(editor: Locator, text: string) {
  await editor.press('ControlOrMeta+a');
  await editor.page().keyboard.insertText(text);
  await expect(editor).toHaveText(text, { useInnerText: true });
}
export async function addOutputField(page: Page, name: string, type = 'string', parent?: string) {
  const dialog = structuredDialog(page);
  await dialog.getByRole('button', { name: parent ? `Add field to ${parent}` : 'Add field', exact: true }).click();
  const field = dialog.getByRole('form', { name: 'Field editor', exact: true });
  await field.getByRole('textbox', { name: 'Field name', exact: true }).fill(name);
  if (type !== 'string') {
    await field.getByRole('button', { name: 'Type', exact: true }).click();
    await page.getByRole('menuitem', { name: type, exact: true }).click();
  }
  return field;
}
