import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Toast } from '../src/presentation/primitives/Toast';
import { useToastNotice } from '../src/locale/use-toast-notice';

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
it('long actionable notices share one hold duration and keep the latest callback', async () => {
 vi.useFakeTimers(); const done = vi.fn(), inspect = vi.fn();
 const ui = render(<Toast text="Inspect the failed operation" holdMs={6000} actions={[{ label: 'View details', onClick: inspect }]} onDone={done}/>);
 await act(async () => {});
 fireEvent.click(screen.getByRole('button', { name: 'View details' })); expect(inspect).toHaveBeenCalledOnce();
 await act(async () => vi.advanceTimersByTime(4000)); expect(done).not.toHaveBeenCalled();
 ui.rerender(<Toast text="Updated wording" holdMs={6000} onDone={done}/>);
 await act(async () => vi.advanceTimersByTime(3000)); expect(done).toHaveBeenCalledOnce();
});
it('success is a polite confirmation instead of an error alert', async () => {
 render(<Toast text="Copied" tone="success" onDone={() => {}}/>);
 await act(async () => {});
 expect(screen.getByRole('status').textContent).toBe('Copied'); expect(screen.queryByRole('alert')).toBeNull();
});
it('repeating the same operation error starts a new display occurrence without changing its native owner', async () => {
 vi.useFakeTimers();
 function Fixture() {
   const [notice, show] = useToastNotice();
   return <><button onClick={() => show('Same failure')}>Try</button>{notice && <Toast key={notice.sequence} text={notice.text} onDone={() => show('')}/>}</>;
 }
 render(<Fixture/>); fireEvent.click(screen.getByRole('button', { name: 'Try' }));
 await act(async () => vi.advanceTimersByTime(3000)); fireEvent.click(screen.getByRole('button', { name: 'Try' }));
 await act(async () => vi.advanceTimersByTime(3000)); expect(screen.getAllByRole('alert')).toHaveLength(1);
 await act(async () => vi.advanceTimersByTime(1000)); expect(screen.queryByRole('alert')).toBeNull();
});
it('a late failure from another view cannot replace the current view notice', async () => {
 let late!: () => void;
 function Fixture({ owner }: { owner: string }) {
   const [notice, show] = useToastNotice(owner);
   return <><button onClick={() => { late = () => show('Old failure'); }}>Start</button><button onClick={() => show('Current failure')}>Fail</button>{notice && <Toast key={notice.sequence} text={notice.text} onDone={() => show('')}/>}</>;
 }
 const ui = render(<Fixture owner="A"/>); fireEvent.click(screen.getByRole('button', { name: 'Start' }));
 ui.rerender(<Fixture owner="B"/>); fireEvent.click(screen.getByRole('button', { name: 'Fail' }));
 await act(async () => {}); act(() => late());
 expect(screen.getByRole('alert').textContent).toBe('Current failure');
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
