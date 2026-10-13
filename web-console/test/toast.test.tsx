import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Toast } from '../src/presentation/primitives/Toast';

afterEach(() => { cleanup(); vi.useRealTimers(); });
it('parent updates do not extend an announcement and completion uses the latest callback', async () => {
 vi.useFakeTimers();
 const first = vi.fn(), latest = vi.fn();
 const ui = render(<Toast text="Missing credential" icon={null} anchor={null} onDone={first}/>);
 await act(async () => {});
 expect(screen.getByRole('alert').parentElement).toBe(document.body);
 await act(async () => vi.advanceTimersByTime(3000));
 ui.rerender(<Toast text="Updated translation" icon={null} anchor={null} onDone={latest}/>);
 await act(async () => vi.advanceTimersByTime(999));
 expect(latest).not.toHaveBeenCalled();
 await act(async () => vi.advanceTimersByTime(1));
 expect(first).not.toHaveBeenCalled(); expect(latest).toHaveBeenCalledOnce();
});
it('a repeated error receives a full new lifetime and unmount cancels completion', async () => {
 vi.useFakeTimers(); const done = vi.fn();
 const ui = render(<Toast key="1" text="Missing credential" icon={null} anchor={null} onDone={done}/>);
 await act(async () => vi.advanceTimersByTime(3000));
 ui.rerender(<Toast key="2" text="Missing credential" icon={null} anchor={null} onDone={done}/>);
 await act(async () => vi.advanceTimersByTime(3000));
 expect(done).not.toHaveBeenCalled();
 ui.unmount(); await act(async () => vi.advanceTimersByTime(4000));
 expect(done).not.toHaveBeenCalled();
});
