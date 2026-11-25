import type { Locator } from '@playwright/test';

/** coss Select triggers are button-role comboboxes, not native select inputs:
 * open the popup and choose the option carrying the native value. */
export async function selectCossOption(trigger: Locator, value: string): Promise<void> {
  await trigger.click();
  await trigger.page().locator(`[role="option"][data-value="${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"]`).click();
}

/** Reads the values a coss Select offers, opening and dismissing its popup. */
export async function readCossOptions(trigger: Locator): Promise<string[]> {
  await trigger.click();
  const options = trigger.page().locator('[role="option"]');
  await options.first().waitFor();
  const values = await options.evaluateAll(list => list.map(el => el.getAttribute('data-value') ?? ''));
  await trigger.page().keyboard.press('Escape');
  return values;
}
