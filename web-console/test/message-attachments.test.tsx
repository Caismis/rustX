import { cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { ContentRef, MessageBlock } from '../../protocol/app-server/v57';
import { Message } from '../src/app/agent/Message';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { JournalSourceProvider } from '../src/app/agent/ObservationSource';
import { MessageFile } from '../src/presentation/agent/Message';
import { ArtifactContext } from '../src/app/components/Artifact';
import { PreviewContext } from '../src/app/components/ArtifactPreview';
import { ArtifactResources } from '../src/client/artifacts';
import { Server, snapshot } from './fixture';

afterEach(() => { cleanup(); vi.restoreAllMocks(); });
const uploaded = { type: 'uploaded_file' as const, batch_id: 'native-batch', name: 'compose.yml' };
const user = (content: Extract<MessageBlock, { role: 'user' }>['content']): MessageBlock => ({ role: 'user', source: 'human', id: 'user-attachment', content });

it('file attachments occupy their own row above plain message text and its ordinary actions', () => {
  const message = user([uploaded, { type: 'text', text: '研究一下这个' }]);
  const ui = render(<AgentTranscript snapshot={{ ...snapshot(), transcript: { entries: [{ cursor: '1', item: { type: 'message', message } }] } }}/>);
  const row = ui.container.querySelector('[data-message-attachments]')!, body = ui.container.querySelector('[data-message-body]')!;
  expect(row.textContent).toBe('compose.ymlYML'); expect(row.querySelector('svg')).toBeTruthy();
  expect(body.textContent).toBe('研究一下这个'); expect(body.contains(row)).toBe(false);
  expect(row.compareDocumentPosition(body) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(ui.getByRole('button', { name: 'Copy' })).toBeTruthy();
  expect(row.textContent).not.toMatch(/0B|undefined|NaN|File attachment/);
});

it.each([false, true])('attachment-only messages have no empty text bubble (empty text block=%s)', emptyText => {
  const extra = emptyText ? [{ type: 'text' as const, text: '' }] : [];
  const ui = render(<Message message={user([uploaded, ...extra, { ...uploaded, name: 'report.pdf' }])}/>);
  expect(ui.container.querySelectorAll('[data-message-file]')).toHaveLength(2);
  expect(ui.container.querySelector('[data-message-body]')).toBeNull();
});

it('local pending cards show known bytes while historical references omit an unknown size', () => {
  const ui = render(<MessageFile name="compose.yml" bytes={3277}/>);
  expect(ui.container.textContent).toContain('YML 3.2KB');
  ui.rerender(<MessageFile name="compose.yml"/>);
  expect(ui.container.textContent).toBe('compose.ymlYML');
});

it('hoisting files preserves the original deferred text block index, including an empty inline prefix', async () => {
  const server = new Server(); await server.attached('A');
  try {
    const text = 'Native text after two attachments', bytes = new TextEncoder().encode(text);
    const ref: ContentRef = { conversation_id: 'conversation-A', source: { type: 'canonical_message_block', component: 'payload', locator: { type: 'by_message_id', message_id: 'user-attachment' }, block_index: 2 }, representation: 'utf8_text', version: 1, content_revision: 1, readable_end: bytes.length };
    server.handlers.set('conversation/contentRead', request => {
      if (request.method !== 'conversation/contentRead') throw Error('Expected content read');
      expect(request.params.content_ref).toEqual(ref);
      return { type: 'content_chunk', chunk: { content_ref: ref, offset: 0, end_offset: bytes.length, readable_end: bytes.length, data_base64: btoa(text), next_offset: null, complete: true } };
    });
    const message = user([uploaded, { ...uploaded, name: 'second.txt' }, { type: 'text', text: '' }]);
    const ui = render(<JournalSourceProvider source={{ client: server.client, target: server.target('A'), cut: () => undefined, owned: () => true }} content={[{ path: ['messages', '0', 'content', '2', 'text'], content_ref: ref, deferred: true }]}><Message message={message}/></JournalSourceProvider>);
    await ui.findByText(text);
    expect(ui.container.querySelector('[data-message-body]')?.textContent).toBe(text);
    expect(ui.container.querySelectorAll('[data-message-file]')).toHaveLength(2);
  } finally { cleanup(); server.client.disconnect(); }
});

it('a managed file card opens its original artifact preview without eager reads or extra inline controls', async () => {
  const server = new Server(); await server.attached('A');
  const resources = new ArtifactResources(server.client, 'A'), openPreview = vi.fn();
  try {
    const ui = render(<ArtifactContext value={resources}><PreviewContext value={{ openPreview, download: vi.fn() }}><Message message={user([{ type: 'file', artifact_id: 'native-file', name: 'plan.md', mime_type: 'text/plain' }, { type: 'text', text: 'Read the plan' }])}/></PreviewContext></ArtifactContext>);
    fireEvent.click(ui.getByRole('button', { name: /plan.md/ }));
    expect(openPreview).toHaveBeenCalledExactlyOnceWith({ source: { kind: 'artifact', id: 'native-file' }, name: 'plan.md', image: false, mimeType: 'text/plain' });
    expect(ui.queryByRole('button', { name: 'Load attachment' })).toBeNull();
    expect(server.requests.some(({ request }) => request.method === 'artifact/read')).toBe(false);
  } finally { cleanup(); resources.dispose(); server.client.disconnect(); }
});
