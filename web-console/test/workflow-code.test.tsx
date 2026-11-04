import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createActor } from 'xstate';
import { workflowEditorMachine } from '../src/app/workflows/machines/editor';
import { workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import WorkflowEditor from '../src/app/workflows/ui/editor/WorkflowEditor';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { viewId } from '../src/app/workflows/ui/editor/selection';
import { codeDocument } from './workflow-code-document';
import { statusSource } from './workflow-fixtures';

vi.mock('../src/app/workflows/ui/editor/forms/BashCodeView',()=>({default:({label,value,onChange,onBlur}:{label:string;value:string;onChange:(value:string)=>void;onBlur:()=>void})=><textarea aria-label={label} value={value} onChange={event=>onChange(event.target.value)} onBlur={onBlur}/>}));
beforeEach(()=>{vi.stubGlobal('ResizeObserver',class{observe(){}unobserve(){}disconnect(){}});vi.stubGlobal('DOMMatrixReadOnly',class{m22=1;});});
afterEach(()=>{cleanup();vi.unstubAllGlobals();localStorage.clear();});

async function open() {
  const source={...statusSource(),document:codeDocument()};
  const port:WorkflowScopePort={endpoint:'ws://127.0.0.1:1/',scope:{kind:'user'},enable:()=>new Promise(()=>{}),toolCatalog:async()=>({sources:[]}),modelChoices:()=>new Promise(()=>{}),analyze:()=>new Promise(()=>{}),list:()=>new Promise(()=>{}),check:()=>new Promise(()=>{}),read:async id=>({...source,id,revision:'sha256:base'}),write:()=>new Promise(()=>{})};
  const surface=createActor(workflowSurfaceMachine,{input:{ports:()=>port}}).start(),editor=createActor(workflowEditorMachine,{input:{port,target:{endpoint:port.endpoint,scope:port.scope,id:'flow'}}}).start();
  await act(async()=>{await Promise.resolve();});render(<WorkflowEditor surface={surface} editor={editor} scopeLabel="Global"/>);
  const card=document.querySelector<HTMLElement>(`.react-flow__node[data-id='${viewId([],'node','code')}']`)!;
  act(()=>card.focus());fireEvent.keyDown(card,{key:'Enter'});
  const documentValue=()=>editor.getSnapshot().context.document!;
  const node=()=>documentValue().block.nodes.code as Extract<import('../src/app/workflows/document/model').NodeDefinition,{type:'code'}>;
  return {editor,documentValue,node,panel:within(screen.getByRole('complementary',{name:'Inspector'}))};
}

it('owns Bash inputs, editor drafts and typed output renames through Undo and Redo',async()=>{
  const {panel,node,documentValue}=await open();
  expect(panel.queryByText('Vision')).toBeNull();
  fireEvent.click(panel.getByRole('button',{name:'Add input variable'}));
  expect(Object.keys(node().variables)).toEqual(['arg1','arg2']);
  fireEvent.change(panel.getAllByRole('textbox',{name:'Variable name'})[1],{target:{value:'payload'}});
  fireEvent.keyDown(panel.getAllByRole('textbox',{name:'Variable name'})[1],{key:'Enter'});
  expect(node().variables.payload).toEqual({type:'reference',path:[]});
  fireEvent.change(panel.getAllByRole('textbox',{name:'Output name'})[0],{target:{value:'renamed'}});
  fireEvent.keyDown(panel.getAllByRole('textbox',{name:'Output name'})[0],{key:'Enter'});
  expect(documentValue().block.nodes.done).toMatchObject({outputs:[{value_selector:['code','renamed']},{value_selector:['code','count']}]});
  fireEvent.click(screen.getByRole('button',{name:'Undo'}));expect(node().outputs.result).toBe('string');
  fireEvent.click(screen.getByRole('button',{name:'Redo'}));expect(node().outputs.renamed).toBe('string');
  const code=await screen.findByRole('textbox',{name:'Bash code'});
  fireEvent.change(code,{target:{value:'printf "{}"'}});expect(node().code).not.toBe('printf "{}"');
  fireEvent.blur(code);expect(node().code).toBe('printf "{}"');
});

it('confirms removal of referenced output and restores both declaration and readers with Undo',async()=>{
  const {panel,node,documentValue}=await open();
  fireEvent.click(panel.getByRole('button',{name:'Remove output result'}));
  const dialog=within(screen.getByRole('dialog',{name:'Remove referenced output?'}));
  expect(node().outputs.result).toBe('string');
  fireEvent.click(dialog.getByRole('button',{name:'Remove output'}));
  expect(node().outputs.result).toBeUndefined();
  expect(documentValue().block.nodes.done).toMatchObject({outputs:[{value_selector:[]},{value_selector:['code','count']}]});
  fireEvent.click(screen.getByRole('button',{name:'Undo'}));
  expect(node().outputs.result).toBe('string');
  expect(documentValue().block.nodes.done).toMatchObject({outputs:[{value_selector:['code','result']},{value_selector:['code','count']}]});
});

it('uses declared output types for defaults and exposes an actual failure port',async()=>{
  const {panel,node}=await open();
  fireEvent.click(panel.getByRole('switch',{name:'Retry on failure'}));expect(node().retry).toEqual({max_retries:3,interval_ms:1000});
  await act(async()=>fireEvent.click(panel.getByRole('button',{name:'Error handling strategy'})));
  await act(async()=>fireEvent.click(await screen.findByRole('menuitem',{name:/^Default value/})));
  expect(node().on_error).toEqual({type:'default_value',value:{result:'',count:0}});
  await act(async()=>fireEvent.click(panel.getByRole('button',{name:'Error handling strategy'})));
  await act(async()=>fireEvent.click(await screen.findByRole('menuitem',{name:/^Fail branch/})));
  expect(node().on_error).toEqual({type:'fail_branch'});
  expect(document.querySelector('.react-flow__handle[data-handleid="fail"]')).toBeTruthy();
});
