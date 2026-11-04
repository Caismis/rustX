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
import { WorkflowAnalysisSession } from '../src/app/workflows/document/analysis';
import type { WorkflowAnalysisReport } from '../../protocol/app-server/v68';

vi.mock('../src/app/workflows/ui/editor/forms/WorkflowCodeView',()=>({default:({label,value,onChange,onBlur}:{label:string;value:string;onChange:(value:string)=>void;onBlur:()=>void})=><textarea aria-label={label} value={value} onChange={event=>onChange(event.target.value)} onBlur={onBlur}/>}));
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

it('owns typed inputs, editor drafts and typed output renames through Undo and Redo',async()=>{
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
  const code=await screen.findByRole('textbox',{name:'Code'});
  fireEvent.change(code,{target:{value:'def main(): return {}'}});expect(node().code).not.toBe('def main(): return {}');
  fireEvent.blur(code);expect(node().code).toBe('def main(): return {}');
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

it('switches between language defaults as one undoable document edit',async()=>{
  const {panel,node,documentValue}=await open(),original=structuredClone(node());
  fireEvent.change(panel.getByRole('combobox',{name:'Language'}),{target:{value:'javascript'}});
  expect(node()).toMatchObject({code_language:'javascript',variables:{arg1:{type:'reference',path:[]},arg2:{type:'reference',path:[]}},outputs:{result:'string'}});
  expect(node().code).toContain('function main({arg1, arg2})');
  expect(documentValue().block.nodes.done).toMatchObject({outputs:[{value_selector:['code','result']},{value_selector:[]}]});
  fireEvent.click(screen.getByRole('button',{name:'Undo'}));expect(node()).toEqual(original);
  expect(documentValue().block.nodes.done).toMatchObject({outputs:[{value_selector:['code','result']},{value_selector:['code','count']}]});
  fireEvent.click(screen.getByRole('button',{name:'Redo'}));expect(node().code_language).toBe('javascript');
});

it('does not replace code typed while native parameter analysis is pending',async()=>{
  let release!:(report:WorkflowAnalysisReport)=>void;
  const request=vi.spyOn(WorkflowAnalysisSession.prototype,'request').mockImplementation(()=>new Promise(resolve=>{release=resolve;}));
  const {panel,node}=await open(),before=node().code;
  fireEvent.click(panel.getByRole('button',{name:'Sync function signature'}));expect(request).toHaveBeenCalledTimes(1);
  const code=await screen.findByRole('textbox',{name:'Code'}),latest='def main(arg1):\n    return {"result":"new draft"}\n';
  fireEvent.change(code,{target:{value:latest}});
  await act(async()=>release({target:{kind:'user'},id:'flow',candidate_revision:`sha256:${'a'.repeat(64)}`,generation:'1',presentation:[],analysis:{outputs:[],references:[],selected:{path:['args','query'],schema:{type:'string'},availability:'available',compatibility:'compatible'},incomplete:false,truncated:false}}));
  expect(node().code).toBe(before);expect((code as HTMLTextAreaElement).value).toBe(latest);
  expect(panel.queryByRole('alert')).toBeNull();
});
