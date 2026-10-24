import assert from 'node:assert/strict';
import {test} from 'node:test';
import {AppServerClient, APP_SERVER_PROTOCOL_VERSION} from '../src/app-server/client.ts';
import {AppServerHost} from '../src/app-server/host.ts';
import {FakeTransport} from './support/app-server-peer.ts';
import {harness, nextRequest, SERVER_CAPABILITIES} from './support/app-server-harness.ts';
import {checkpoint, snapshot} from './support/fixtures.ts';

async function connection(authority='fixture-app-server-authority') {
 const transport=new FakeTransport();
 const initializing=AppServerClient.initialize({transport});
 const initialize=await nextRequest({transport},'initialize');
 transport.respond(initialize.id,{type:'initialized',authority_id:authority,protocol_version:APP_SERVER_PROTOCOL_VERSION,capabilities:SERVER_CAPABILITIES});
 const client=await initializing;
 return {transport,client,host:new AppServerHost({client,ownership:'external'})};
}

function readDomains(cursor: string, conversationId: string): import('../../protocol/app-server/v57.ts').RuntimeClientEvent {
 const cut=checkpoint(snapshot({conversation_id:conversationId}),cursor);
 return {type:'projection_transition',events:[{type:'read_domains_updated',trace:{records:[]},transcript:{entries:[]},occupancy:null,todos:null}],
   metadata:{read_cut:cut.read_cut,collections:cut.collections,content:cut.content}};
}

test('T17 reconnect subscribes from locally Applied with matching Runtime identity and joins replay',async t=>{
 const old=await harness();t.after(()=>old.client.close());
 old.transport.emit(old.target,'1',readDomains('1',old.target.conversation_id));
 assert.equal(old.session.state.cursor,'1');assert.equal(old.session.checkpoint?.cursor,'1');
 old.client.close();
 const current=await connection();t.after(()=>current.client.close());
 const attaching=current.host.attach(old.session.sessionId,old.session.nodeId,old.session);
 const attach=await nextRequest(current,'session/attach');
 const target={...old.target,attachment_id:'new-attachment'};
 current.transport.respond(attach.id,{type:'attached',target,attachment_kind:'controller',observation_source:'live',configuration_revision:'0'});
 const subscribe=await nextRequest(current,'session/subscribe');
 assert.equal(current.transport.transportCount('session/checkpoint'),0);
 assert.equal((subscribe.params as {after_cursor:string}).after_cursor,'1');
 current.transport.respond(subscribe.id,{type:'subscribed',target,registration_id:'new-registration',after_cursor:'1'});
 current.transport.emit(target,'2',readDomains('2',target.conversation_id));
 const configuration=await nextRequest(current,'session/configuration');
 current.transport.respond(configuration.id,{type:'session_configuration',projection:null});
 const session=await attaching;
 assert.equal(session.state.cursor,'2');
 assert.equal(current.transport.transportCount('session/checkpoint'),0);
});

test('T18 changed Runtime incarnation requires a fresh checkpoint before following',async t=>{
 const old=await harness();t.after(()=>old.client.close());old.client.close();
 const current=await connection();t.after(()=>current.client.close());
 const attaching=current.host.attach(old.session.sessionId,old.session.nodeId,old.session);
 const attach=await nextRequest(current,'session/attach');
 const target={...old.target,attachment_id:'new-incarnation',runtime_incarnation:'2'};
 current.transport.respond(attach.id,{type:'attached',target,attachment_kind:'controller',observation_source:'live',configuration_revision:'0'});
 const read=await nextRequest(current,'session/checkpoint');
 assert.equal(current.transport.transportCount('session/subscribe'),0);
 current.transport.respond(read.id,{type:'checkpoint',checkpoint:checkpoint(snapshot(),'9')});
 const subscribe=await nextRequest(current,'session/subscribe');
 assert.equal((subscribe.params as {after_cursor:string}).after_cursor,'9');
 current.transport.respond(subscribe.id,{type:'subscribed',target,registration_id:'fresh-registration',after_cursor:'9'});
 const configuration=await nextRequest(current,'session/configuration');
 current.transport.respond(configuration.id,{type:'session_configuration',projection:null});
 assert.equal((await attaching).state.cursor,'9');
});


test('T18 a restarted server cannot reuse a prior Applied cursor even when Runtime incarnation numbers coincide',async t=>{
 const old=await harness();t.after(()=>old.client.close());
 old.transport.emit(old.target,'1',readDomains('1',old.target.conversation_id));
 assert.equal(old.session.state.cursor,'1');old.client.close();
 const current=await connection('restarted-server-authority');t.after(()=>current.client.close());
 const attaching=current.host.attach(old.session.sessionId,old.session.nodeId,old.session);
 const attach=await nextRequest(current,'session/attach');
 const target={...old.target,attachment_id:'new-process-same-counter'};
 current.transport.respond(attach.id,{type:'attached',target,attachment_kind:'controller',observation_source:'live',configuration_revision:'0'});
 const read=await nextRequest(current,'session/checkpoint');
 assert.equal(current.transport.transportCount('session/subscribe'),0);
 current.transport.respond(read.id,{type:'checkpoint',checkpoint:checkpoint(snapshot(),'0')});
 const subscribe=await nextRequest(current,'session/subscribe');
 assert.equal((subscribe.params as {after_cursor:string}).after_cursor,'0');
 current.transport.respond(subscribe.id,{type:'subscribed',target,registration_id:'restarted-registration',after_cursor:'0'});
 const configuration=await nextRequest(current,'session/configuration');
 current.transport.respond(configuration.id,{type:'session_configuration',projection:null});
 assert.equal((await attaching).state.cursor,'0');
 assert.notEqual(current.client.authorityId,old.client.authorityId);
});
