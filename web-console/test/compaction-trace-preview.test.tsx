import {cleanup,render,screen} from '@testing-library/react';
import {afterEach,expect,it,vi} from 'vitest';
import type {CheckpointPage,TraceInspectionHead,TraceRecord} from '../../protocol/app-server/v82';
import {TrajectoryInspector,type TrajectoryInspectorProps} from '../src/app/trajectory/TrajectoryInspector';
import {traceRecord} from './trace-fixture';
import {localeController} from '../src/locale/controller';
import {translator} from '../src/locale/translation';
import {checkpoint,snapshot} from './fixture';
import {traceInspectionHead} from '../../protocol/app-server/trace-inspection';

afterEach(cleanup);
const original='# First original summary\n\nKeep  exact spacing.\n';
const record=(sequence:number,text:string,truncated=false)=>traceRecord(sequence,{kind:'compaction',request:null,location:{},native_id:`compaction-${sequence}`,compaction:{mode:'blocking',status:null,outcome:'committed'},preview:{text,truncated},truncated,has_detail:true});
function inspection(record:TraceRecord):CheckpointPage{
 const read_cut=checkpoint(snapshot()).read_cut,head:TraceInspectionHead={id:record.id,kind:record.kind,record,content:{type:'compaction',page:{domain:`trace.records.${record.id.slice(6)}.detail.calls`,total:'0',items:[],next_offset:null}}};
 return {domain:`trace.records.${record.id.slice(6)}.detail`,cursor:read_cut.revision,read_cut,total:'1',next_offset:null,collections:[],items:[{offset:'0',identity:record.id,key:null,value:JSON.parse(JSON.stringify(head)),deferred:true,body_complete:false,content:[]}]};
}
function props(record:TraceRecord):TrajectoryInspectorProps{
 return {item:{type:'RecordRow',record,display_key:record.id,owner_record_id:record.id,facet:'overview',label:'Compaction',preview:''},facet:'overview',detailPage:inspection(record),onFacet:vi.fn(),onLoadDetail:vi.fn(),onClose:vi.fn(),records:[record],executions:new Map(),requestNumbers:new Map(),completeHistory:true,onOpen:vi.fn()};
}
it('validates the original paged Compaction Call head and its cursor',()=>{
 const selected=record(11,original),page=inspection(selected),value=page.items[0].value as unknown as TraceInspectionHead;
 if(value.content.type!=='compaction')throw Error('expected Compaction head');
 value.content.page={domain:'trace.records.11.detail.calls',total:'2',next_offset:'20',items:[{
  call_id:'aux-one',started_sequence:'20',settled_sequence:'21',outcome:'completed',preceding_call:null,
  previous_summary:null,strategy:null,requests_domain:'trace.records.11.detail.calls.aux-one.requests',request_count:'1',
 }]};
 expect(traceInspectionHead(page,selected.id)?.content.type).toBe('compaction');
 value.content.page.next_offset='22';
 expect(()=>traceInspectionHead(page,selected.id)).toThrow('cursor changed');
});
it('shows the exact completed projection summary in Summary and Raw without a canonical summary Message',()=>{
 const selected=record(11,original),properties=props(selected),view=render(<TrajectoryInspector {...properties}/>);
 expect(screen.getByRole('heading',{name:'First original summary'})).toBeTruthy();
 expect(screen.queryByRole('status')).toBeNull();
 expect(properties.onLoadDetail).not.toHaveBeenCalled();
 view.rerender(<TrajectoryInspector {...properties} facet="raw"/>);
 expect(view.container.querySelector('pre')?.textContent).toBe(original);
 expect(screen.queryByRole('status')).toBeNull();
});
it('a newer projection cannot replace the selected historical compaction preview',()=>{
 const first=record(11,original),second=record(21,'Second original summary'),properties=props(first);
 const view=render(<TrajectoryInspector {...properties} facet="raw" records={[first,second]}/>);
 expect(view.container.querySelector('pre')?.textContent).toBe(original);
 expect(screen.queryByText('Second original summary')).toBeNull();
 view.rerender(<TrajectoryInspector {...props(second)} facet="raw" records={[first,second]}/>);
 expect(view.container.querySelector('pre')?.textContent).toBe('Second original summary');
 view.rerender(<TrajectoryInspector {...properties} facet="raw" records={[first,second]}/>);
 expect(view.container.querySelector('pre')?.textContent).toBe(original);
});
it.each([['overview','en'],['raw','en'],['overview','zh'],['raw','zh']] as const)('a truncated native summary preserves its limitation without a permanent loader in %s/%s',(facet,locale)=>{
 localeController.setLocale(locale);
 const prefix='# Long original\n\n水🙂 exact prefix',selected=record(31,prefix,true);
 const view=render(<TrajectoryInspector {...props(selected)} facet={facet}/>);
 expect(screen.queryByRole('status')).toBeNull();
 expect(view.container.textContent).not.toContain('unread original remainder');
 expect(screen.getByText(translator(locale)('agent:context.summary-truncated'))).toBeTruthy();
 if(facet==='raw')expect(view.container.querySelector('pre')?.textContent).toBe(prefix);
 else expect(screen.getByRole('heading',{name:'Long original'})).toBeTruthy();
});
