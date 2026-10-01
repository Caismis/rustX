import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { TurnNavigator } from '../src/app/agent/TurnNavigator';
import { Server } from './fixture';
let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); });
it('bounded native marks retain focus preview when the pointer leaves and keyboard focus never navigates alone', async () => {
  server = new Server(); await server.attached('A');
  server.handlers.set('session/turns', () => ({type:'conversation_turns',page:{cut:{conversation_id:'conversation-A',journal:'1',transcript:'1000',surface_revision: '1', pending_count: '0', pending_revision: '0'},offset:0,total:1000,
    turns:Array.from({length:64},(_,i)=>({id:{conversation_id:'conversation-A',attempt_id:`native-${i}`},ordinal:i+1,cursor:String(i+1),preview:`Bounded native preview ${i}`}))}}));
  const navigate=vi.fn();let ui: ReturnType<typeof render>;
  await act(async()=>{ui=render(<TurnNavigator client={server.client} sessionId="A" onNavigate={navigate}/>);});
  const marks=ui!.container.querySelectorAll<HTMLButtonElement>('[data-turn-id]');expect(marks.length).toBe(64);
  act(()=>marks[0].focus());expect(ui!.getByRole('tooltip').textContent).toContain('Bounded native preview 0');
  fireEvent.pointerLeave(marks[0].parentElement!);expect(ui!.getByRole('tooltip').textContent).toContain('Bounded native preview 0');
  fireEvent.keyDown(marks[0],{key:'ArrowDown'});expect(document.activeElement).toBe(marks[1]);expect(navigate).not.toHaveBeenCalled();
  fireEvent.click(marks[1]);expect(navigate).toHaveBeenCalledWith(expect.objectContaining({id:{conversation_id:'conversation-A',attempt_id:'native-1'}}));
  expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(1);
});
