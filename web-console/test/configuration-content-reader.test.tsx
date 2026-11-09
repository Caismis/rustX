import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { ConfigurationContent } from '../src/app/trajectory/TrajectoryInspector';
import type { TraceFieldReader } from '../src/client/trace';
import type { CheckpointReadCut, ContentChunk, ContentRef } from '../../protocol/app-server/v76';
afterEach(cleanup);

const text = 'Instructions B';
const reference: ContentRef = { conversation_id: 'conv', source: { type: 'configuration_instructions', journal_sequence: 7 }, representation: 'utf8_text', version: 1, content_revision: 1, readable_end: text.length };
const cut = (through: string) => ({ conversation_id: 'conv', revision: '1', through }) as CheckpointReadCut;
const chunk = (): ContentChunk => ({ content_ref: reference, offset: 0, data_base64: btoa(text), end_offset: text.length, readable_end: text.length, next_offset: null, complete: true });

function heldReader() {
  const reads: { offset: number; cut: CheckpointReadCut; current: () => boolean; release: () => void }[] = [];
  const reader = { content: (cut: CheckpointReadCut, _ref: ContentRef, offset: number, current: () => boolean) => new Promise<ContentChunk>(resolve => { reads.push({ offset, cut, current, release: () => resolve(chunk()) }); }) } as unknown as TraceFieldReader;
  return { reader, reads };
}

it('a parent rerender with the same reader and an equal cut keeps one held read and its text', async () => {
  const { reader, reads } = heldReader();
  const ui = render(<ConfigurationContent reader={reader} cut={cut('9')} reference={reference} />);
  for (let index = 0; index < 3; index++) ui.rerender(<ConfigurationContent reader={reader} cut={cut('9')} reference={{ ...reference }} />);
  expect(reads.map(read => read.offset)).toEqual([0]);
  expect(reads[0]!.current()).toBe(true);
  await act(async () => reads[0]!.release());
  expect(await screen.findByText(text)).toBeTruthy();
  ui.rerender(<ConfigurationContent reader={reader} cut={cut('9')} reference={reference} />);
  expect(screen.getByText(text)).toBeTruthy();
  expect(reads).toHaveLength(1);
});

it('a different read cut or reader owns a new read and retires the held one', async () => {
  const { reader, reads } = heldReader();
  const ui = render(<ConfigurationContent reader={reader} cut={cut('9')} reference={reference} />);
  ui.rerender(<ConfigurationContent reader={reader} cut={cut('10')} reference={reference} />);
  expect(reads).toHaveLength(2);
  expect(reads[0]!.current()).toBe(false);
  expect(reads[1]!.cut.through).toBe('10');
  const other = heldReader();
  ui.rerender(<ConfigurationContent reader={other.reader} cut={cut('10')} reference={reference} />);
  expect(reads[1]!.current()).toBe(false);
  expect(other.reads).toHaveLength(1);
});
