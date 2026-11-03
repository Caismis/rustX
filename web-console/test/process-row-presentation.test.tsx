import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createRef } from 'react';
import { Reasoning } from '../src/presentation/agent/Reasoning';
import { TurnProcess } from '../src/presentation/agent/TurnProcess';
import { DisclosureRow } from '../src/presentation/primitives/DisclosureRow';
import { SessionNodeItem } from '../src/presentation/workspace/Rows';
import { translator } from '../src/locale/translation';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { Server, snapshot } from './fixture';
import { chatFlowFixture } from './fixtures/chat-flow';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it('a native running process keeps its Tool work and gains a settled duration without hiding the answer', async () => {
  const server = new Server(), state = snapshot();
  chatFlowFixture(server, state); server.snapshots.set('A', state);
  try {
    await server.attached('A');
    const view = render(<AgentTranscript snapshot={state}/>);
    expect(view.queryByRole('button', { name: /Took/ })).toBeNull();
    expect(view.container.querySelector('[data-tool-call-id="flow-read"]')).not.toBeNull();
    await act(() => window.chatFlowFixture.settle());
    view.rerender(<AgentTranscript snapshot={server.snapshots.get('A')!}/>);
    expect(view.getByRole('heading', { name: '对话流已整理' })).toBeTruthy();
    expect(view.container.querySelector('[data-turn-process]')?.textContent).toContain('1m 5s');
  } finally { await server.client.disconnect(); }
});

it('only settled work has a duration header and missing end times never run a browser clock', () => {
  const props = { id: 'turn', tools: 2, messages: 1, open: true, start: '2026-11-03T01:00:00Z' };
  const view = render(<TurnProcess {...props} outcome="running"/>);
  expect(view.queryByRole('button')).toBeNull();
  view.rerender(<TurnProcess {...props} outcome="completed"/>);
  expect(view.getByRole('button', { name: 'Worked' })).toBeTruthy();
  view.rerender(<TurnProcess {...props} outcome="completed" end="2026-11-03T01:01:05Z"/>);
  expect(view.getByRole('button').textContent).toBe(translator('en')('agent:copy.took-value', { p0: translator('en')('agent:duration.minutes-seconds', { m: 1, s: 5 }) }));
  view.rerender(<TurnProcess {...props} outcome="cancelled" end="2026-11-03T01:01:05Z"/>);
  expect(view.getByRole('button', { name: 'Stopped' })).toBeTruthy();
});

it('a running disclosure retains one accessible title and its controlled body', () => {
  const toggle = vi.fn();
  const view = render(<DisclosureRow icon={null} title="Read report" running open={false} expandable expandOnRowClick onToggle={toggle}>Original body</DisclosureRow>);
  const button = view.getByRole('button', { name: 'Read report' });
  expect(view.getAllByText('Read report')).toHaveLength(1);
  expect(view.container.querySelector('[aria-hidden][inert]')).not.toBeNull();
  expect(document.getElementById(button.getAttribute('aria-controls')!)?.hidden).toBe(true);
  fireEvent.keyDown(button, { key: 'Enter' });
  expect(toggle).toHaveBeenCalledOnce();
  view.rerender(<DisclosureRow icon={null} title="Read report" running={false} open expandable expandOnRowClick onToggle={toggle}>Original body</DisclosureRow>);
  expect(view.getByText('Original body')).toBeTruthy();
  expect(view.container.querySelector('[aria-hidden][inert]')).toBeNull();
});

it('streaming reasoning previews only completed paragraph first lines and retains its full body', () => {
  const view = render(<Reasoning text="First paragraph" running/>);
  const row = () => view.container.querySelector('[data-variant="think"]')!;
  expect(row().hasAttribute('data-preview')).toBe(false);
  view.rerender(<Reasoning text={'**First paragraph**\n\nSecond paragraph'} running/>);
  expect(view.getByText('First paragraph')).toBeTruthy();
  expect(view.queryByText('Second paragraph')).toBeNull();
  view.rerender(<Reasoning text={'**First paragraph**\n\nSecond paragraph\nMore detail'} running/>);
  expect(view.getByText('Second paragraph')).toBeTruthy();
  view.rerender(<Reasoning text={'**First paragraph**\n\nSecond paragraph\nMore detail'}/>);
  expect(view.getByText('First paragraph')).toBeTruthy();
  fireEvent.click(view.getByRole('button', { name: /Reasoning/ }));
  expect(view.getByText(/More detail/)).toBeTruthy();
  expect(row().hasAttribute('data-preview')).toBe(false);
});

it('disabled previews are absent while the reasoning body keeps its disclosure state', () => {
  const view = render(<Reasoning text="Original reasoning"/>);
  expect(view.getByText('Original reasoning')).toBeTruthy();
  view.rerender(<Reasoning text="Original reasoning" preview={false}/>);
  expect(view.queryByText('Original reasoning')).toBeNull();
  fireEvent.click(view.getByRole('button', {name:'Reasoning'}));
  expect(view.getByText('Original reasoning')).toBeTruthy();
  view.rerender(<Reasoning text="Original reasoning"/>);
  expect(view.getByRole('button', {name:'Reasoning'}).getAttribute('aria-expanded')).toBe('true');
  view.rerender(<Reasoning text="Original reasoning" preview={false}/>);
  expect(view.getByText('Original reasoning')).toBeTruthy();
});

function marqueeFixture(reduced: boolean) {
  const frames = new Map<number, FrameRequestCallback>();
  let id = 0;
  vi.stubGlobal('requestAnimationFrame', vi.fn((callback: FrameRequestCallback) => { frames.set(++id, callback); return id; }));
  vi.stubGlobal('cancelAnimationFrame', vi.fn((id: number) => { frames.delete(id); }));
  vi.stubGlobal('matchMedia', () => ({ matches: reduced }));
  const open = vi.fn();
  const view = render(<SessionNodeItem node={{ id: 'session', title: 'A long title', running: false, runningSubagentCount: 0, updatedAt: 0 }} currentId={undefined} now={0} onOpen={open} onRename={vi.fn()} onFork={vi.fn()} onDelete={vi.fn()} menuFocusOwner={createRef()} t={translator('en')}/>);
  const title = view.getByRole('button', { name: 'Open A long title' }).querySelector('span')!;
  Object.defineProperties(title, { scrollWidth: { value: 160 }, clientWidth: { value: 100 } });
  const tick = (time: number) => act(() => { const pending = [...frames]; frames.clear(); for (const [, callback] of pending) callback(time); });
  return { view, title, tick, frames, open, row: view.getByRole('treeitem') };
}

it('a title marquee advances while hovered, resets on leave and stops after unmount', () => {
  const f = marqueeFixture(false);
  fireEvent.pointerEnter(f.row); f.tick(0); f.tick(1000);
  expect(f.title.scrollLeft).toBe(30);
  expect(f.title.hasAttribute('data-scrolled')).toBe(true);
  expect(f.title.hasAttribute('data-clipped')).toBe(true);
  fireEvent.pointerLeave(f.row);
  expect(f.frames.size).toBe(0);
  expect(f.title.scrollLeft).toBe(0);
  expect(f.title.hasAttribute('data-scrolled')).toBe(false);
  fireEvent.pointerEnter(f.row); f.tick(0); f.tick(2000);
  expect(f.title.scrollLeft).toBe(60);
  expect(f.title.hasAttribute('data-clipped')).toBe(false);
  fireEvent.pointerLeave(f.row); fireEvent.pointerEnter(f.row);
  f.view.unmount();
  expect(f.frames.size).toBe(0);
  expect(f.open).not.toHaveBeenCalled();
});

it('reduced motion reveals the title without scheduling an animation', () => {
  const f = marqueeFixture(true);
  fireEvent.pointerEnter(f.row);
  expect(f.title.scrollLeft).toBe(60);
  expect(f.frames.size).toBe(0);
  fireEvent.pointerLeave(f.row);
  expect(f.title.scrollLeft).toBe(0);
});
