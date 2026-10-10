import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { ConversationPreferenceStore, conversationPreferences, CONVERSATION_PREFERENCE_KEY } from '../src/app/conversation-preferences';
import { GeneralPage } from '../src/app/settings/general/GeneralPage';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { snapshot } from './fixture';
import type { RuntimeClientTranscriptEntry, TurnProcessView } from '../../protocol/app-server/v41';

afterEach(() => { cleanup(); conversationPreferences().update({fontSize:14, transcriptMode:'detailed', codingView:true}); });
it('validates stored preferences and applies settings even when browser storage fails', () => {
  const storage = new Map<string,string>();
  const adapter = { getItem:(key:string) => storage.get(key) ?? null, setItem:(key:string,value:string) => { storage.set(key,value); } };
  const store = new ConversationPreferenceStore(adapter);
  store.update({fontSize:22, transcriptMode:'verbose', codingView:false});
  expect(new ConversationPreferenceStore(adapter).getSnapshot()).toEqual(store.getSnapshot());
  store.update({fontSize:23}); expect(store.getSnapshot().fontSize).toBe(22);
  storage.set(CONVERSATION_PREFERENCE_KEY, '{broken');
  expect(new ConversationPreferenceStore(adapter).getSnapshot().fontSize).toBe(14);
  const denied = new ConversationPreferenceStore({getItem:()=>{throw Error();},setItem:()=>{throw Error();}});
  denied.update({fontSize:10});expect(denied.getSnapshot().fontSize).toBe(10);
});
it('General edits the three preferences, bounds the font stepper and persists the selection', async () => {
  render(<GeneralPage theme="dark"/>);
  fireEvent.click(screen.getByRole('button', {name:'Increase font size'}));
  fireEvent.click(screen.getByRole('button', {name:'Work details Detailed'}));
  fireEvent.click(await screen.findByRole('menuitem', {name:'Fully expanded'}));
  fireEvent.click(screen.getByRole('switch', {name:'Show coding view'}));
  expect(new ConversationPreferenceStore().getSnapshot()).toEqual({fontSize:15, transcriptMode:'verbose', codingView:false});
  act(()=>conversationPreferences().update({fontSize:22}));
  expect((screen.getByRole('button',{name:'Increase font size'}) as HTMLButtonElement).disabled).toBe(true);
});
const process: TurnProcessView = {conversation_id:'c',attempt_id:'a',outcome:'completed',control_cursor:'1',final_message_id:'final',message_count:2,tool_call_count:0};
const entries: RuntimeClientTranscriptEntry[] = [
  {cursor:'1',turn_process:process,item:{type:'message',message:{role:'assistant',id:'first',content:[{type:'reasoning',text:'A reasoning preview'},{type:'text',text:'An intermediate reply'}]}}},
  {cursor:'2',turn_process:process,item:{type:'message',message:{role:'assistant',id:'final',content:[{type:'text',text:'Final reply'}]}}},
];
it('verbose reveals completed work, preserves manual disclosures across modes, and compact hides reasoning previews', () => {
  render(<AgentTranscript snapshot={{...snapshot(),transcript:{entries}}}/>);
  expect(screen.getByText('An intermediate reply').closest('[hidden]')).not.toBeNull();
  act(()=>conversationPreferences().update({transcriptMode:'verbose'}));
  expect(screen.getByText('An intermediate reply').closest('[hidden]')).toBeNull();
  expect((screen.getByRole('button',{name:'Worked'}) as HTMLButtonElement).disabled).toBe(true);
  const reasoning = screen.getByRole('button',{name:/Reasoning/});
  fireEvent.click(reasoning);
  expect(reasoning.getAttribute('aria-expanded')).toBe('true');
  act(()=>conversationPreferences().update({transcriptMode:'standard'}));
  fireEvent.click(screen.getByRole('button',{name:'Worked'}));
  fireEvent.click(screen.getByRole('button',{name:'Analysis completed'}));
  expect(reasoning.getAttribute('aria-expanded')).toBe('true');
  fireEvent.click(reasoning);
  act(()=>conversationPreferences().update({transcriptMode:'compact'}));
  expect(screen.queryByText('A reasoning preview')).toBeNull();
});
it('compact and standard group running work, while detailed exposes its rows', () => {
  const liveEntries = entries.map(entry=>({...entry,turn_process:{...process,outcome:'running' as const}}));
  act(()=>conversationPreferences().update({transcriptMode:'compact'}));
  render(<AgentTranscript snapshot={{...snapshot(),transcript:{entries:liveEntries}}}/>);
  expect(screen.getByRole('button',{name:'Analyzing request'}).getAttribute('aria-expanded')).toBe('false');
  act(()=>conversationPreferences().update({transcriptMode:'standard'}));
  expect(screen.getByRole('button',{name:'Analyzing request · A reasoning preview'})).toBeTruthy();
  act(()=>conversationPreferences().update({transcriptMode:'detailed'}));
  expect(screen.queryByRole('button',{name:/Analyzing request/})).toBeNull();
  expect(screen.getByRole('button',{name:/Reasoning/}).closest('[hidden]')).toBeNull();
});
it('streaming replies stay visible while compact mode groups adjacent reasoning and preserves its disclosure', () => {
  act(()=>conversationPreferences().update({transcriptMode:'compact'}));
  const live = {...snapshot(), attempt:{attempt_id:'live',phase:{type:'running' as const},turn:1,in_flight:{message_id:'stream',blocks:[
    {type:'reasoning' as const,block_index:0,text:'Live reasoning'},
    {type:'text' as const,block_index:1,text:'Visible live reply'},
  ]}}};
  render(<AgentTranscript snapshot={live}/>);
  expect(screen.getByText('Visible live reply').closest('[hidden]')).toBeNull();
  expect(screen.getByText('Live reasoning').closest('[hidden]')).not.toBeNull();
  fireEvent.click(screen.getByRole('button',{name:'Analyzing request'}));
  expect(screen.getByText('Live reasoning').closest('[hidden]')).toBeNull();
  act(()=>conversationPreferences().update({transcriptMode:'detailed'}));
  act(()=>conversationPreferences().update({transcriptMode:'compact'}));
  expect(screen.getByRole('button',{name:'Analyzing request'}).getAttribute('aria-expanded')).toBe('true');
});
