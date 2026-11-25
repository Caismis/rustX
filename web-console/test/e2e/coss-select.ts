import type { Locator } from '@playwright/test';

/** coss Select triggers are button-role comboboxes, not native select inputs:
 * open the popup and choose the option carrying the native value. */
export async function selectCossOption(trigger: Locator, value: string): Promise<void> {
  await trigger.click();
  await trigger.page().locator(`[role="option"][data-value="${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"]`).click();
}
