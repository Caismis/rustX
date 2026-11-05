import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createActor } from 'xstate';
import { workflowEditorMachine } from '../src/app/workflows/machines/editor';
import { workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import WorkflowEditor from '../src/app/workflows/ui/editor/WorkflowEditor';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { viewId } from '../src/app/workflows/ui/editor/selection';
import { dataNodesDocument } from './workflow-data-document';
import { statusSource } from './workflow-fixtures';
import { nodeTemplate } from '../src/app/workflows/document/commands';
import { templateInputName } from '../src/app/workflows/document/template';

vi.mock('../src/app/workflows/ui/editor/forms/WorkflowCodeView',()=>({default:({label,value,onChange,onBlur}:{label:string;value:string;onChange:(value:string)=>void;onBlur:()=>void})=><textarea aria-label={label} value={value} onChange={event=>onChange(event.target.value)} onBlur={onBlur}/>}));
beforeEach(()=>{vi.stubGlobal('ResizeObserver',class{observe(){}unobserve(){}disconnect(){}});vi.stubGlobal('DOMMatrixReadOnly',class{m22=1;});});
afterEach(()=>{cleanup();vi.unstubAllGlobals();localStorage.clear();});

async function open() {
  const document=dataNodesDocument();document.block.nodes.left={type:'template',variables:{arg1:{type:'reference',path:['args','query']}},template:'{{ arg1 }} / {{ arg1 }}'};
  const source={...statusSource(),document};
  const port:WorkflowScopePort={endpoint:'ws://127.0.0.1:1/',scope:{kind:'user'},enable:()=>new Promise(()=>{}),toolCatalog:async()=>({sources:[]}),modelChoices:()=>new Promise(()=>{}),analyze:()=>new Promise(()=>{}),list:()=>new Promise(()=>{}),check:()=>new Promise(()=>{}),read:async id=>({...source,id,revision:'sha256:base'}),write:()=>new Promise(()=>{})};
  const surface=createActor(workflowSurfaceMachine,{input:{ports:()=>port}}).start(),editor=createActor(workflowEditorMachine,{input:{port,target:{endpoint:port.endpoint,scope:port.scope,id:'flow'}}}).start();
  await act(async()=>{await Promise.resolve();});render(<WorkflowEditor surface={surface} editor={editor} scopeLabel="Global"/>);
  const card=window.document.querySelector<HTMLElement>(`.react-flow__node[data-id='${viewId([],'node','left')}']`)!;act(()=>card.focus());fireEvent.keyDown(card,{key:'Enter'});
  const node=()=>editor.getSnapshot().context.document!.block.nodes.left as Extract<import('../src/app/workflows/document/model').NodeDefinition,{type:'template'}>;
  return {editor,node,panel:within(screen.getByRole('complementary',{name:'Inspector'}))};
}

it('renames generated template tokens and bindings together through Undo/Redo',async()=>{
  const {node,panel}=await open();
  fireEvent.change(panel.getByRole('textbox',{name:'Variable name'}),{target:{value:'question'}});fireEvent.keyDown(panel.getByRole('textbox',{name:'Variable name'}),{key:'Enter'});
  expect(node().variables).toEqual({question:{type:'reference',path:['args','query']}});expect(node().template).toBe('{{ question }} / {{ question }}');
  fireEvent.click(screen.getByRole('button',{name:'Undo'}));expect(node().template).toBe('{{ arg1 }} / {{ arg1 }}');expect(node().variables.arg1).toBeDefined();
  fireEvent.click(screen.getByRole('button',{name:'Redo'}));expect(node().template).toBe('{{ question }} / {{ question }}');
  fireEvent.click(panel.getByRole('button',{name:'Add input variable'}));
  const names=panel.getAllByRole('textbox',{name:'Variable name'});fireEvent.change(names[1],{target:{value:'question'}});fireEvent.keyDown(names[1],{key:'Enter'});
  expect(Object.keys(node().variables)).toEqual(['question','arg1']);expect(panel.getByRole('alert')).toBeTruthy();
});

it('commits source drafts and folds the fixed output description',async()=>{
  const {node,panel}=await open();
  const output=panel.getByRole('button',{name:'Output variables'});expect(output.getAttribute('aria-expanded')).toBe('false');
  fireEvent.click(output);expect(panel.getByText('The transformed content.')).toBeTruthy();
  const source=await panel.findByRole('textbox',{name:'Template code'}),text="{{ arg1.strip() | default('empty') }}";
  fireEvent.change(source,{target:{value:text}});expect(node().template).not.toBe(text);fireEvent.blur(source);expect(node().template).toBe(text);
  fireEvent.click(panel.getByRole('button',{name:'Remove input arg1'}));expect(node().variables).toEqual({});expect(node().template).toBe(text);
  expect(panel.queryByRole('combobox',{name:'Value type'})).toBeNull();expect(panel.queryByText('Vision')).toBeNull();
});

it('uses valid unique variable names for typed paths and generated rows',()=>{
  expect(templateInputName(['arg1','arg2'])).toBe('arg3');
  expect(templateInputName(['query'],'query')).toBe('query_1');
  expect(templateInputName(['arg1'],'中文')).toBe('arg2');
  expect(templateInputName([], '__proto__')).toBe('__proto__');
});

it('creates the Dify initial input and interpolation without a preset constant',()=>{
  expect(nodeTemplate('template')).toEqual({type:'template',variables:{arg1:{type:'reference',path:[]}},template:'{{ arg1 }}'});
});
