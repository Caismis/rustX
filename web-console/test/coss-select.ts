import { fireEvent, screen } from '@testing-library/react';

/** Commit one coss/Base UI select choice using the pointer sequence its item
 * contract recognizes. */
export async function chooseCossOption(trigger: HTMLElement, option: string) {
  fireEvent.click(trigger);
  const item = await screen.findByRole('option', { name: option });
  fireEvent.pointerDown(item);
  fireEvent.click(item);
}
