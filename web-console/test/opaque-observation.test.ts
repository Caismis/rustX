import { materializeSettings } from '../../protocol/app-server/settings-observation';
import { expect, it } from 'vitest';
import { domainOriginalValue, domainValue, parseObservedValue } from '../../protocol/app-server/domain-observation';
import { materializeFields } from '../../protocol/app-server/field-observation';
import { checkpoint, snapshot } from './fixture';
import type { CheckpointPage } from '../../protocol/app-server/v57';

it('T08/T19 source JSON preserves typed decimal quantities and refuses rounded opaque integers', () => {
  expect(parseObservedValue('{"revision":"18446744073709551615","safe":9007199254740991}')).toEqual({revision:'18446744073709551615',safe:9007199254740991});
  expect(() => parseObservedValue('{"opaque":9007199254740993}')).toThrow('exact byte view');
  expect(() => parseObservedValue('{"opaque":-9007199254740993}')).toThrow('exact byte view');
});

it('T08/T19 a deferred model field cannot round a parameter and become new mutation input', async () => {
  const raw = '{"requestParams":{"seed":9007199254740993}}';
  const reference = {revision:'r',byte_length:raw.length};
  await expect(materializeFields({revision:'r',fields:[{name:'configured',reference}]},async()=>({reference,offset:0,end_offset:raw.length,data_base64:btoa(raw),next_offset:null}),()=>true)).rejects.toThrow('exact byte view');
});

it('T08/T19 deferred domain JSON refuses precision loss while raw UTF-8 stays exact', async () => {
  const raw = '{ "seed": 9007199254740993, "escape": "\\u0061" }';
  const page: CheckpointPage = {domain:'model',cursor:'0',read_cut:checkpoint(snapshot()).read_cut,total:'1',items:[{offset:'0',identity:'model',value:null,content:[],deferred:true}],next_offset:null,collections:[]};
  const read = (representation: 'canonical_json_v1' | 'utf8_text') => ({current:()=>true,page:async()=>page,value:async()=>({cut:page.read_cut,domain:page.domain,item_offset:0,path:[],representation,offset:0,bytes_base64:btoa(raw),readable_end:raw.length,next_offset:null})});
  await expect(domainValue(page,read('canonical_json_v1'))).rejects.toThrow('exact byte view');
  expect(await domainValue(page,read('utf8_text'))).toBe(raw);
});

it('T08/T19 a bounded settings read cannot round opaque request parameters into editable state', async () => {
  const raw = '{"cwd":"/native","model":{"model":"exact/model","requestParams":{"seed":18446744073709551615}}}';
  const reference = {session_id:'S',revision:'7',byte_length:raw.length};
  await expect(materializeSettings({revision:'7',cwd:'/native',reference,inline:null},async()=>({reference,offset:0,end_offset:raw.length,data_base64:btoa(raw),next_offset:null}),()=>true)).rejects.toThrow('exact byte view');
});

it('T08/T19 authoritative UTF-8 text retains a literal leading BOM', async () => {
  const raw = '\uFEFFnative text';
  const bytes = new TextEncoder().encode(raw);
  const page: CheckpointPage = {domain:'model',cursor:'0',read_cut:checkpoint(snapshot()).read_cut,total:'1',items:[{offset:'0',identity:'model',value:null,content:[],deferred:true}],next_offset:null,collections:[]};
  const read = {current:()=>true,page:async()=>page,value:async()=>({cut:page.read_cut,domain:page.domain,item_offset:0,path:[],representation:'utf8_text' as const,offset:0,bytes_base64:btoa(String.fromCharCode(...bytes)),readable_end:bytes.length,next_offset:null})};
  expect(await domainValue(page,read)).toBe(raw);
});

it('T08/T15 selecting original domain bytes does not mistake a finite summary for the full value', async () => {
  const page: CheckpointPage = {domain:'trace.records.7.detail',cursor:'0',read_cut:checkpoint(snapshot()).read_cut,total:'1',items:[{offset:'0',identity:'request',value:{id:'trace:7',request:{request_id:'native'}},content:[],deferred:true}],next_offset:null,collections:[]};
  const original = {id:'trace:7',request:{request_id:'native',tools:[{name:'read'}],text:'é'.repeat(40_000)}};
  const bytes = new TextEncoder().encode(JSON.stringify(original));
  const offsets: number[] = [];
  const read = {current:()=>true,page:async()=>page,value:async(_page: CheckpointPage,_item: unknown,offset: number)=>{
    offsets.push(offset); const body=bytes.subarray(offset,offset+65_536), end=offset+body.length;
    return {cut:page.read_cut,domain:page.domain,item_offset:0,path:[],representation:'canonical_json_v1' as const,offset,bytes_base64:btoa(String.fromCharCode(...body)),readable_end:bytes.length,next_offset:end<bytes.length?end:null};
  }};
  expect(await domainValue(page,read)).toEqual(page.items[0].value);
  expect(offsets).toEqual([]);
  expect(await domainOriginalValue(page,read)).toEqual(original);
  expect(offsets).toEqual([0,65_536]);
});
