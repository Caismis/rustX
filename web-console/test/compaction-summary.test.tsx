import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CompactionCheckpoint } from '../src/app/agent/CompactionCheckpoint';
import { CompactionMessage } from '../src/presentation/agent/Message';
import { projectionHead, Server, checkpoint, snapshot } from './fixture';
import type { ProjectionDetail } from '../../protocol/app-server/v57';
afterEach(cleanup);

it.each([false,true])('shows only the DSH marker until expanded, without model-projection metadata (inherited=%s)', inherited => {
  const server = new Server(), head = projectionHead('## Earlier work\n\nKeep  the English words.\n\n- Continue here', inherited);
  const ui = render(<CompactionCheckpoint head={head} client={server.client} sessionId="A"/>);
  const trigger = ui.getByRole('button', { name: 'Context compacted · View compaction summary' });
  expect(trigger.getAttribute('aria-expanded')).toBe('false');
  expect(trigger.textContent).not.toContain('Earlier work');
  expect(ui.container.textContent).not.toMatch(/projection|frontier|conv_/i);
  fireEvent.click(trigger);
  expect(ui.getByRole('heading', { name: 'Earlier work' })).toBeTruthy();
  expect(ui.getByRole('listitem').textContent).toBe('Continue here');
  expect(ui.container.textContent).not.toMatch(/Historical model|Inspect projected|Origin:|canonical frontier/);
  expect(ui.container.querySelector('[data-chat-anchor-key]')).toBeNull();
  fireEvent.click(trigger);
  expect(ui.queryByRole('heading', { name: 'Earlier work' })).toBeNull();
});
it('a running command has the DSH compact title/shimmer and no empty disclosure', () => {
  const ui = render(<CompactionMessage title="compact" summary="Compacting context…" running/>);
  const trigger = ui.getByRole('button', { name: 'compact · Compacting context…' });
  expect((trigger as HTMLButtonElement).disabled).toBe(true);
  expect(trigger.hasAttribute('aria-expanded')).toBe(false);
  expect(ui.container.querySelector('[data-shimmer]')).toBeTruthy();
  expect(ui.container.querySelector('[data-compaction-body]')).toBeNull();
});
it('missing summary is explicitly unavailable and never opens an empty body', () => {
  const ui = render(<CompactionCheckpoint head={{ ...projectionHead(), summary_preview: null }} client={new Server().client} sessionId="A"/>);
  expect((ui.getByRole('button', { name: 'Context compacted · Compaction summary unavailable' }) as HTMLButtonElement).disabled).toBe(true);
});
it('a bounded summary loads on expansion, retains complete Markdown and reopens without another read', async () => {
  const server = new Server(); await server.attached('A');
  try {
    const head = { ...projectionHead('Bounded prefix'), summary_preview: { text: 'Bounded prefix', truncated: true } };
    const detail: ProjectionDetail = { read_cut: checkpoint(snapshot()).read_cut, checkpoint: { ...head, summary_preview: { text: '## Full summary\n\nOriginal continuation facts.', truncated: false } }, items: [], next_item: null };
    const read = vi.spyOn(server.client, 'projectionDetail').mockResolvedValue(detail);
    const ui = render(<CompactionCheckpoint head={head} client={server.client} sessionId="A"/>);
    expect(read).not.toHaveBeenCalled();
    fireEvent.click(ui.getByRole('button'));
    expect(await ui.findByRole('heading', { name: 'Full summary' })).toBeTruthy();
    expect(read).toHaveBeenCalledExactlyOnceWith('A', head.checkpoint_id);
    fireEvent.click(ui.getByRole('button')); fireEvent.click(ui.getByRole('button'));
    expect(ui.getByRole('heading', { name: 'Full summary' })).toBeTruthy();
    expect(read).toHaveBeenCalledOnce();
  } finally { server.client.disconnect(); }
});
it('late summary content cannot replace another checkpoint', async () => {
  const server = new Server(); await server.attached('A');
  try {
    const head = { ...projectionHead('Original prefix'), summary_preview: { text: 'Original prefix', truncated: true } };
    let finish!: (detail: ProjectionDetail) => void;
    const read = vi.spyOn(server.client, 'projectionDetail').mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const ui = render(<CompactionCheckpoint key="first" head={head} client={server.client} sessionId="A"/>);
    fireEvent.click(ui.getByRole('button')); await waitFor(() => expect(read).toHaveBeenCalledOnce());
    ui.rerender(<CompactionCheckpoint key="second" head={{ ...projectionHead('Second original'), checkpoint_id: 'second', activation_id: '2' }} client={server.client} sessionId="A"/>);
    await act(async () => finish({ read_cut: checkpoint(snapshot()).read_cut, checkpoint: { ...head, summary_preview: { text: 'Stale full text', truncated: false } }, items: [], next_item: null }));
    fireEvent.click(ui.getByRole('button'));
    expect(ui.getByText('Second original')).toBeTruthy(); expect(ui.queryByText('Stale full text')).toBeNull();
  } finally { server.client.disconnect(); }
});
