import { fireEvent, screen, within } from '@testing-library/react';

/** Explicitly choose the advanced representation in the redesigned panel. */
export function showSchemaJson(label: string): HTMLTextAreaElement {
  const panel = screen.getByRole('complementary', { name: 'Inspector' });
  const section = [...panel.querySelectorAll<HTMLElement>('[data-schema-label]')].find(section => section.dataset.schemaLabel === label);
  if (!section) throw new Error(`No schema editor for ${label}`);
  const current = within(section).queryByLabelText(label);
  if (current instanceof HTMLTextAreaElement) return current;
  fireEvent.click(within(section).getAllByRole('button', { name: 'Advanced JSON' })[0]);
  return within(section).getByLabelText(label) as HTMLTextAreaElement;
}
