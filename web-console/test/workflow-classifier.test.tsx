import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createActor } from 'xstate';
import { workflowEditorMachine } from '../src/app/workflows/machines/editor';
import { workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import WorkflowEditor from '../src/app/workflows/ui/editor/WorkflowEditor';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import type { WorkflowEdit } from '../../protocol/app-server/v66';
import { encodeField, nodeField } from '../src/app/workflows/document/fields';
import { viewId } from '../src/app/workflows/ui/editor/selection';
import { dataNodesDocument } from './workflow-data-document';
import { statusSource } from './workflow-fixtures';

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} });
  vi.stubGlobal('DOMMatrixReadOnly', class { m22 = 1; });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); localStorage.clear(); });
async function open() {
  const source = {...statusSource(),document:dataNodesDocument()}, writes:WorkflowEdit[]=[];
  const port:WorkflowScopePort={endpoint:'ws://127.0.0.1:1/',scope:{kind:'user'},enable:()=>new Promise(()=>{}),toolCatalog:async()=>({sources:[]}),
    modelChoices:()=>new Promise(()=>{}),analyze:()=>new Promise(()=>{}),list:()=>new Promise(()=>{}),check:()=>new Promise(()=>{}),
    read:async id=>({...source,id,revision:'sha256:base'}),write:(_id,_expected,edit)=>{writes.push(edit);return new Promise(()=>{});}};
  const surface=createActor(workflowSurfaceMachine,{input:{ports:()=>port}}).start();
  const editor=createActor(workflowEditorMachine,{input:{port,target:{endpoint:port.endpoint,scope:port.scope,id:'flow'}}}).start();
  await act(async()=>{await Promise.resolve();});render(<WorkflowEditor surface={surface} editor={editor} scopeLabel="Global"/>);
  const card=document.querySelector<HTMLElement>(`.react-flow__node[data-id='${viewId([],'node','classify')}']`)!;
  act(()=>card.focus());fireEvent.keyDown(card,{key:'Enter'});
  const node=()=>editor.getSnapshot().context.document!.block.nodes.classify as Extract<import('../src/app/workflows/document/model').NodeDefinition,{type:'classifier'}>;
  return {editor,writes,node,panel:within(screen.getByRole('complementary',{name:'Inspector'}))};
}

it('renames display labels with Unicode without changing exit identity, and exposes Dify output fields',async()=>{
  const {editor,node,panel}=await open(), edges=structuredClone(editor.getSnapshot().context.document!.block.edges ?? []);
  fireEvent.doubleClick(panel.getByRole('button',{name:'Rename CLASS 1'}));
  fireEvent.change(panel.getByRole('textbox',{name:'Class label'}),{target:{value:'售后支持'}});
  fireEvent.keyDown(panel.getByRole('textbox',{name:'Class label'}),{key:'Enter'});
  expect(node().classes[0]).toMatchObject({id:'class_1',label:'售后支持'});
  expect(editor.getSnapshot().context.document!.block.edges).toEqual(edges);
  expect(panel.queryByRole('textbox',{name:'Class ID'})).toBeNull();
  expect(panel.queryByRole('switch',{name:'Vision'})).toBeNull();
  for(const name of ['class_name','class_label','usage'])expect(panel.getByText(name,{exact:true})).toBeTruthy();
  fireEvent.click(panel.getByRole('button',{name:'Advanced settings'}));
  expect(panel.queryByRole('textbox',{name:'Instruction'})).toBeNull();
  fireEvent.click(panel.getByRole('button',{name:'Advanced settings'}));
  expect(panel.getByRole('textbox',{name:'Instruction'})).toBeTruthy();
});

it('moves class drafts with their owner through reorder, Undo and Redo',async()=>{
  const {editor,node,panel}=await open();
  const address=nodeField([],'classify',['classes',0,'label']);
  act(()=>editor.send({type:'DRAFT',address,text:JSON.stringify('Pending label')}));
  fireEvent.keyDown(panel.getByRole('button',{name:'Move CLASS 1'}),{altKey:true,key:'ArrowDown'});
  expect(node().classes.map(item=>item.id)).toEqual(['class_2','class_1']);
  const moved=encodeField(nodeField([],'classify',['classes',1,'label']));
  expect(editor.getSnapshot().context.drafts[moved]?.text).toBe(JSON.stringify('Pending label'));
  fireEvent.click(screen.getByRole('button',{name:'Undo'}));
  expect(node().classes.map(item=>item.id)).toEqual(['class_1','class_2']);
  expect(editor.getSnapshot().context.drafts[encodeField(address)]?.text).toBe(JSON.stringify('Pending label'));
  fireEvent.click(screen.getByRole('button',{name:'Redo'}));
  expect(node().classes.map(item=>item.id)).toEqual(['class_2','class_1']);
  expect(editor.getSnapshot().context.drafts[moved]?.text).toBe(JSON.stringify('Pending label'));
});

it('removing a class retires only its edges; Undo restores its content and exit',async()=>{
  const {editor,node,panel}=await open(), edges=structuredClone(editor.getSnapshot().context.document!.block.edges ?? []);
  fireEvent.click(panel.getByRole('button',{name:'Remove class CLASS 1'}));
  expect(node().classes.map(item=>item.id)).toEqual(['class_2']);
  expect(editor.getSnapshot().context.document!.block.edges).toEqual(edges.filter(edge=>edge.from!=='classify'||edge.port!=='class_1'));
  fireEvent.click(screen.getByRole('button',{name:'Undo'}));
  expect(node().classes.map(item=>item.id)).toEqual(['class_1','class_2']);
  expect(editor.getSnapshot().context.document!.block.edges).toEqual(edges);
});
