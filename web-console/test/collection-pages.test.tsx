import {act,cleanup,fireEvent,render} from '@testing-library/react';
import {afterEach,expect,it,vi} from 'vitest';
import {CollectionPages} from '../src/app/agent/CollectionPages';
import {checkpoint,Server,snapshot} from './fixture';
import type {CheckpointPage} from '../../protocol/app-server/v51';
let server:Server;
afterEach(()=>{cleanup();server?.client.disconnect();});
it('T15 native collection pages replace the loaded workset and never advance Applied',async()=>{
  server=new Server();await server.attached('A');
  const cut=checkpoint(snapshot()).read_cut,target=server.client.target('A');
  server.handlers.set('conversation/checkpointPage',request=>{
    if(request.method!=='conversation/checkpointPage')throw new Error('method');
    const offset=request.params.offset;
    expect(request.params).toMatchObject({target,cut,domain:'agents',limit:32});
    const page:CheckpointPage={domain:'agents',cursor:'0',read_cut:cut,total:'80',items:Array.from({length:Math.min(32,80-offset)},(_,index)=>({offset:String(offset+index),identity:`agent-${offset+index}`,value:{agent_id:`agent-${offset+index}`},content:[],deferred:false})),next_offset:offset+32<80?String(offset+32):null,collections:[]};
    return {type:'checkpoint_page',page};
  });
  const ui=render(<CollectionPages client={server.client} target={target} cut={cut} collections={[{domain:'agents',revision:'0',total:'80',loaded:'32',next_offset:'32'}]} owned={()=>true}/>);
  fireEvent.change(ui.getByRole('combobox'),{target:{value:'agents'}});
  await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read first page'})));
  expect(ui.getByText('agent-0')).toBeTruthy();
  await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read next page'})));
  expect(ui.queryByText('agent-0')).toBeNull();expect(ui.getByText('agent-32')).toBeTruthy();
  await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read next page'})));
  expect(ui.queryByText('agent-32')).toBeNull();expect(ui.getByText('agent-79')).toBeTruthy();
  expect(ui.queryByRole('button',{name:'Read next page'})).toBeNull();
  expect(server.client.getSnapshot().views.A.cursor).toBe('0');
});
it('T14 changing the source revision fences a late collection page',async()=>{
  server=new Server();await server.attached('A');
  const cut=checkpoint(snapshot()).read_cut,target=server.client.target('A');
  server.held.add('conversation/checkpointPage');
  const view=(revision:string)=><CollectionPages client={server.client} target={target} cut={{...cut,domain_revisions:{...cut.domain_revisions,agents:revision}}} collections={[{domain:'agents',revision,total:'80',loaded:'32',next_offset:'32'}]} owned={()=>true}/>;
  const ui=render(view('1'));fireEvent.change(ui.getByRole('combobox'),{target:{value:'agents'}});
  await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read first page'})));
  const old=await server.waitFor('conversation/checkpointPage',1);
  ui.rerender(view('2'));
  await act(async()=>server.socket.success(old,{type:'checkpoint_page',page:{domain:'agents',cursor:'0',read_cut:{...cut,domain_revisions:{...cut.domain_revisions,agents:'1'}},total:'80',items:[{offset:'0',identity:'obsolete-agent',value:null,content:[],deferred:true}],next_offset:'1',collections:[]}}));
  expect(ui.queryByText('obsolete-agent')).toBeNull();
  expect((ui.getByRole('button',{name:'Read first page'}) as HTMLButtonElement).disabled).toBe(false);
});

it('T13/T15 a deferred catalog identity remains selectable through its exact native page ordinal',async()=>{
  server=new Server();await server.attached('A');
  const cut=checkpoint(snapshot()).read_cut,target=server.target('A'),domain='model_catalog.models';
  const page:CheckpointPage={domain,cursor:'0',read_cut:cut,total:'73',items:[{offset:'0',identity:'model position 0',value:null,content:[],deferred:true}],next_offset:'1',collections:[]};
  server.handlers.set('conversation/checkpointPage',()=>({type:'checkpoint_page',page}));
  const chosen=vi.fn(async(selected:CheckpointPage,item:CheckpointPage['items'][number],current:()=>boolean)=>{
    expect(selected).toEqual(page);expect(item.offset).toBe('0');expect(item.value).toBeNull();expect(current()).toBe(true);
  });
  const ui=render(<CollectionPages client={server.client} target={target} cut={cut} collections={[{domain,revision:'7',total:'73',loaded:'0',next_offset:'0'}]} owned={()=>true} onModel={chosen}/>);
  fireEvent.change(ui.getByRole('combobox'),{target:{value:domain}});
  await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read first page'})));
  await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Choose catalog defaults'})));
  expect(chosen).toHaveBeenCalledOnce();
  expect(server.requests.some(row=>row.request.method==='conversation/checkpointValueRead')).toBe(false);
});

it('T11 a late chooser failure cannot overwrite a replacement catalog page',async()=>{
  server=new Server();await server.attached('A');
  const cut=checkpoint(snapshot()).read_cut,target=server.target('A'),domain='model_catalog.models';
  let reject!:(error:Error)=>void;
  const chosen=vi.fn(()=>new Promise<void>((_resolve,failed)=>{reject=failed;}));
  server.handlers.set('conversation/checkpointPage',request=>{if(request.method!=='conversation/checkpointPage')throw new Error('method');return {type:'checkpoint_page',page:{domain,cursor:'0',read_cut:request.params.cut,total:'73',items:[{offset:'0',identity:'native model',value:null,content:[],deferred:true}],next_offset:'1',collections:[]}};});
  const view=(revision:string)=><CollectionPages client={server.client} target={target} cut={{...cut,domain_revisions:{model_catalog:revision}}} collections={[{domain,revision,total:'73',loaded:'0',next_offset:'0'}]} owned={()=>true} onModel={chosen}/>;
  const ui=render(view('7'));fireEvent.change(ui.getByRole('combobox'),{target:{value:domain}});
  await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read first page'})));
  fireEvent.click(ui.getByRole('button',{name:'Choose catalog defaults'}));
  ui.rerender(view('8'));
  await act(async()=>reject(new Error('obsolete selection error')));
  expect(ui.queryByText('obsolete selection error')).toBeNull();
  expect(ui.queryByText('native model')).toBeNull();
  expect(chosen).toHaveBeenCalledOnce();
});

for(const domain of ['trace.records','transcript.entries'])it(`T15 ${domain} advances through native exclusive cursors while replacing the bounded page`,async()=>{
 server=new Server();await server.attached('A');
 const cut=checkpoint(snapshot()).read_cut,target=server.target('A');
 const pages=new Map([[0,{rows:[70,80,90],next:'70'}],[70,{rows:[30,40,60],next:'30'}],[30,{rows:[1,20],next:null}]]);
 const observed:number[]=[];
 server.handlers.set('conversation/checkpointPage',request=>{
  if(request.method!=='conversation/checkpointPage')throw new Error('method');
  observed.push(request.params.offset);
  const page=pages.get(request.params.offset)!;
  return {type:'checkpoint_page',page:{domain,cursor:'0',read_cut:cut,total:'8',items:page.rows.map(offset=>({offset:String(offset),identity:`native-${offset}`,value:null,content:[],deferred:true})),next_offset:page.next,collections:[]}};
 });
 const ui=render(<CollectionPages client={server.client} target={target} cut={cut} collections={[{domain,revision:'0',total:'8',loaded:'3',next_offset:'70'}]} owned={()=>true}/>);
 fireEvent.change(ui.getByRole('combobox'),{target:{value:domain}});
 await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read first page'})));
 expect(ui.getByText('native-90')).toBeTruthy();
 await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read next page'})));
 expect(ui.queryByText('native-90')).toBeNull();expect(ui.getByText('native-60')).toBeTruthy();
 await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read next page'})));
 expect(ui.queryByText('native-60')).toBeNull();expect(ui.getByText('native-1')).toBeTruthy();
 expect(ui.queryByRole('alert')).toBeNull();expect(ui.queryByRole('button',{name:'Read next page'})).toBeNull();
 expect(observed).toEqual([0,70,30]);expect(server.client.getSnapshot().views.A.cursor).toBe('0');
});

it('T11/T12 a page from another captured native cut cannot become the loaded observation',async()=>{
 server=new Server();await server.attached('A');
 const cut=checkpoint(snapshot()).read_cut,target=server.target('A');
 server.handlers.set('conversation/checkpointPage',()=>({type:'checkpoint_page',page:{domain:'agents',cursor:'2',read_cut:{...cut,revision:'2'},total:'1',items:[{offset:'0',identity:'foreign-cut-agent',value:null,content:[],deferred:true}],next_offset:null,collections:[]}}));
 const ui=render(<CollectionPages client={server.client} target={target} cut={cut} collections={[{domain:'agents',revision:'0',total:'2',loaded:'0',next_offset:'0'}]} owned={()=>true}/>);
 fireEvent.change(ui.getByRole('combobox'),{target:{value:'agents'}});
 await act(async()=>fireEvent.click(ui.getByRole('button',{name:'Read first page'})));
 expect(ui.queryByText('foreign-cut-agent')).toBeNull();expect(ui.getByRole('alert')).toBeTruthy();
 expect(server.client.getSnapshot().views.A.cursor).toBe('0');
});
