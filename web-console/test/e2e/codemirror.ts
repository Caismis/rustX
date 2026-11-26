import type { Locator } from '@playwright/test';

export async function replaceCodeMirrorText(editor: Locator, text: string) {
  // CodeMirror can redraw over the DOM range that Playwright's fill() selects.
  // Select in CodeMirror's own state before sending replacement text.
  await editor.press('ControlOrMeta+a');
  if (text) await editor.page().keyboard.insertText(text);
  else await editor.press('Backspace');
}
