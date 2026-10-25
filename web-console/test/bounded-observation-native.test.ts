// @vitest-environment node
import { expect, it, onTestFailed } from 'vitest';
import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
import { TempFixture } from '../../tui/test/support/temp-fixture.ts';
import { ProviderEmulator } from '../../tui/test/support/provider-emulator.ts';
import { AppServerHost } from '../../tui/src/app-server/host.ts';
import type { AppServerSession } from '../../tui/src/app-server/session.ts';
import { AppServerClient } from '../src/client/app-server';
import type { Socket } from '../src/client/contracts';
import { RpcFailure } from '../src/client/errors';
import type { AttachmentTarget, ContentRef, RuntimeClientCheckpoint } from '../../protocol/app-server/v57';

/** Real native ownership and transport; every wait follows data or process exit. */
it('T10 T11 disconnect preserves native execution and WebUI/TUI recover exact bounded content over real connections', async () => {
  const binary = process.env.RUSTX_BINARY ?? fileURLToPath(new URL('../../target/debug/rustx', import.meta.url));
  const fixture = TempFixture.create('rustx-467-client-parity-');
  let stage = 'provider readiness', originalFailure: string | undefined, nativeStderr = '';
  let providerEvidence: ProviderEmulator | undefined, nativeSession: AppServerSession | undefined, observedSessionId: string | undefined;
  let processOwner: ChildProcessWithoutNullStreams | undefined, terminal: AppServerHost | undefined;
  const web = new AppServerClient((url, protocols) => new WebSocket(url, protocols) as unknown as Socket);
  const targetEvidence = (target?: AttachmentTarget) => target && {
    session: target.session_id.slice(0, 256), conversation: target.conversation_id.slice(0, 256),
    incarnation: target.runtime_incarnation, attachment: target.attachment_id.slice(0, 256),
  };
  const cutEvidence = (checkpoint?: RuntimeClientCheckpoint) => {
    const source = checkpoint?.read_cut.source_cut;
    return checkpoint && { cursor: checkpoint.cursor, revision: checkpoint.read_cut.revision, through: checkpoint.read_cut.through,
      source: source && { journal: source.read.journal, transcript: source.read.transcript,
        ledger: source.ledger_through, mutation: source.read.mutation_revision } };
  };
  onTestFailed(() => {
    const view = observedSessionId ? web.getSnapshot().views[observedSessionId] : undefined;
    console.error('T10/T11 native parity failure', JSON.stringify({
      stage, originalFailure, nativePid: processOwner?.pid, nativeExitCode: processOwner?.exitCode, nativeSignal: processOwner?.signalCode,
      terminal: nativeSession && { phase: nativeSession.observationPhase, controlsReady: nativeSession.controlReady,
        applied: nativeSession.state.cursor, target: targetEvidence(nativeSession.target), cut: cutEvidence(nativeSession.checkpoint),
        attemptId: nativeSession.state.attempt?.attemptId?.slice(0, 256), attemptPhase: nativeSession.state.attempt?.phase.type,
        observationError: nativeSession.observationError?.slice(0, 512) },
      web: view && { connection: web.getSnapshot().connection, phase: view.observationState, applied: view.cursor,
        target: targetEvidence(view.target), cut: cutEvidence(view.checkpoint),
        attemptId: view.snapshot?.attempt?.attempt_id?.slice(0, 256), attemptPhase: view.snapshot?.attempt?.phase.type,
        error: view.error?.slice(0, 512) },
      nativeStderr, providerStderr: providerEvidence?.diagnostics.slice(-2048),
    }));
  });
  const provider = await ProviderEmulator.start('bounded_observation');
  providerEvidence = provider;
  const token = 'issue467-parity-token-'.padEnd(43, 'x');
  try {
    const home=fixture.path('home'),cwd=fixture.path('workspace');mkdirSync(home,{recursive:true});mkdirSync(cwd,{recursive:true});
    const env: NodeJS.ProcessEnv={...process.env,HOME:home,RUSTX_467_FIXTURE_KEY:'fixture-only'};delete env.XDG_CONFIG_HOME;delete env.XDG_STATE_HOME;
    stage = 'native init';
    const initialized=spawnSync(binary,['init','--template','openai-chat','--provider','fixture','--model-id','integration-model','--endpoint',provider.url(),'--credential-env','RUSTX_467_FIXTURE_KEY','--context-window','128000','--max-output','4096','--tool-calls','true','--reasoning','false','--compat','chat_reasoning_replay = "omit"'],{env,encoding:'utf8'});
    expect(initialized.status,initialized.stderr).toBe(0);
    writeFileSync(join(cwd,'rustx.toml'),'[agent.tools]\nbuiltin = ["read"]\n');writeFileSync(fixture.path('token'),token+'\n');
    processOwner=spawn(binary,['app-server','--runtime-root',fixture.path('runtime'),'--listen','ws://127.0.0.1:0','--token-file',fixture.path('token')],{env,cwd,stdio:['pipe','pipe','pipe']});
    processOwner.stderr.on('data', (chunk: Buffer) => { nativeStderr = (nativeStderr + chunk.toString('utf8')).slice(-4096); });
    const lines=createInterface({input:processOwner.stderr});
    stage = 'native endpoint readiness';
    const endpoint=await new Promise<string>((resolve,reject)=>{processOwner!.once('exit',code=>reject(new Error(`Native server exited before readiness: ${code}`)));lines.on('line',line=>{const ready=line.match(/rustx app-server listening (ws:\/\/\S+)/);if(ready)resolve(ready[1]);});});
    stage = 'terminal connect'; terminal=await AppServerHost.connectRemote({endpoint,token});
    stage = 'source settings read';
    const source=await terminal.client.call('configuration/sourcesRead',{target:{kind:'workspace',directory:cwd}},'source_settings');
    expect(source.projection.inline?.prospective_diagnostic??null,JSON.stringify(source.projection)).toBe(null);
    expect(source.projection.inline?.session_models?.kind,JSON.stringify(source.projection)).toBe('available');
    stage = 'session create';
    const created=await terminal.createSession({cwd,model:{model:'fixture/integration-model',requestParams:{temperature:0.25}}});
    await terminal.renameSession(created.session.id, 'Bounded observation fixture');
    observedSessionId = created.session.id; stage = 'terminal attach';
    const session=await terminal.attach(created.session.id,created.session.active_node); nativeSession = session;
    expect(session.observationPhase).toBe('following');
    const text='hello from the tui\n'+'水🙂'.repeat(12_000),bytes=Buffer.from(text);
    const running=new Promise<void>(resolve=>{const stop=session.onState(state=>{if(state.attempt?.phase.type==='running'){stop();resolve();}});});
    stage = 'inbound admission'; const accepted=await session.submitInbound([{type:'text',text}]);
    // Admission accepts input; the provider arrival deadline starts after native execution starts.
    stage = 'native Running publication'; await running;
    stage = 'provider execution gate'; await provider.awaitGate('observation-execution');
    const attempt=session.state.attempt!.attemptId,incarnation=session.target.runtime_incarnation;
    const conversation=session.state.conversationId;
    async function reconstruct(ref:ContentRef,read:(offset:number)=>Promise<import('../../protocol/app-server/v57').ContentChunk>){const parts:Buffer[]=[];let offset=0;do{const chunk=await read(offset);const part=Buffer.from(chunk.data_base64,'base64');expect(part.length).toBeLessThanOrEqual(65536);expect(chunk.content_ref).toEqual(ref);expect(chunk.offset).toBe(offset);expect(chunk.end_offset).toBe(offset+part.length);parts.push(part);offset=chunk.end_offset;expect(chunk.next_offset??null).toBe(offset<ref.readable_end?offset:null);}while(offset<ref.readable_end);return Buffer.concat(parts);}
    // Closing the physical observer leaves the gated native Attempt running.
    stage = 'first terminal disconnect'; terminal.client.close();await terminal.shutdown();terminal=undefined;
    expect(session.state.attempt?.phase.type).toBe('running');
    web.setSessionAdmission(async(_id,current)=>({current,validate:async()=>current()}));
    stage = 'web connect'; await web.connect(endpoint,token);
    stage = 'web attach'; await web.attach(created.session.id);
    let view=web.getSnapshot().views[created.session.id]!;
    expect(view.snapshot?.conversation_id).toBe(conversation);expect(view.observationState).toBe('following');
    expect(view.target?.runtime_incarnation).toBe(incarnation);
    expect(view.snapshot?.attempt?.attempt_id).toBe(attempt);expect(view.snapshot?.attempt?.phase.type).toBe('running');
    const settled=new Promise<void>(resolve=>{const stop=web.subscribe(()=>{
      if(web.getSnapshot().views[created.session.id]?.snapshot?.attempt?.phase.type==='settled'){stop();resolve();}
    });});
    stage = 'provider gate release'; await provider.releaseGate('observation-execution');
    stage = 'native Settled publication'; await settled;
    view=web.getSnapshot().views[created.session.id]!;
    expect(view.snapshot?.attempt?.attempt_id).toBe(attempt);
    if(view.snapshot?.attempt?.phase.type==='settled'&&view.snapshot.attempt.phase.outcome?.type!=='completed'){
      stage = 'failed-outcome Trace diagnostic'; const trace=await web.request({method:'session/trace',params:{target:view.target!,limit:32}},'trace');
      throw new Error(JSON.stringify({phase:view.snapshot.attempt.phase,records:trace.page.records.map(record=>({id:record.id,state:record.state,request:record.request,timing:record.timing})),request_count:(await provider.requests()).length}).slice(0,12000));
    }
    expect(view.snapshot?.attempt?.phase,JSON.stringify(view.snapshot?.attempt?.phase)).toMatchObject({type:'settled',outcome:{type:'completed'}});
    const originalInput = (reference: ContentRef) => reference.source.type === 'canonical_message_block'
      && reference.source.component === 'payload' && reference.source.block_index === 0
      && reference.source.locator.type === 'by_message_id' && reference.source.locator.message_id === accepted.messageId
      && reference.representation === 'utf8_text';
    const reference=view.checkpoint!.content.find(binding=>originalInput(binding.content_ref))!.content_ref;
    const current=()=>web.getSnapshot().views[created.session.id]?.target===view.target;
    stage = 'web original content ranges'; expect(await reconstruct(reference,offset=>web.reads.readContent(view.target!,undefined,reference,offset,current))).toEqual(bytes);
    const execution=view.snapshot!.attempt!.phase;
    stage = 'web forged-frontier refusal'; await expect(web.reads.readContent(view.target!,undefined,{...reference,readable_end:reference.readable_end+1},0,current)).rejects.toBeInstanceOf(RpcFailure);
    expect(web.getSnapshot().views[created.session.id]!.snapshot!.attempt!.phase).toEqual(execution);
    stage = 'web explicit repair'; await web.refresh(created.session.id);view=web.getSnapshot().views[created.session.id]!;expect(view.observationState).toBe('following');expect(view.snapshot!.attempt!.phase).toEqual(execution);
    stage = 'original provider request count'; expect(await provider.requests()).toHaveLength(1);
    // The TUI independently loads the settled native cut after the Web view
    // releases its connection; neither client transfers a complete mirror.
    stage = 'web disconnect'; await web.disconnect();
    stage = 'second terminal connect'; terminal=await AppServerHost.connectRemote({endpoint,token});
    stage = 'second terminal attach'; const recovered=await terminal.attach(created.session.id); nativeSession = recovered;
    expect(recovered.state.attempt?.attemptId).toBe(attempt);
    expect(recovered.state.attempt?.phase).toEqual(execution);
    const canonical=recovered.checkpoint!.content.find(binding=>originalInput(binding.content_ref))!.content_ref;
    expect(canonical).toEqual(reference);
    stage = 'terminal original content ranges'; expect(await reconstruct(canonical,offset=>recovered.readContent(canonical,offset))).toEqual(bytes);
    stage = 'terminal original transcript'; const history=await recovered.transcriptPage();expect(history.entries?.some(row=>row.item.type==='message'&&row.item.message.id===(canonical.source.type==='canonical_message_block'&&canonical.source.locator.type==='by_message_id'?canonical.source.locator.message_id:undefined))).toBe(true);
  } catch (error) {
    originalFailure = String(error).slice(0, 512);
    if(error instanceof Error && 'method' in error && 'error' in error)throw new Error(`${error.method}: ${JSON.stringify(error.error)}`,{cause:error});
    throw error;
  } finally {
    stage = 'cleanup web disconnect'; await web.disconnect();
    stage = 'cleanup terminal shutdown'; await terminal?.shutdown();
    if(processOwner&&processOwner.exitCode===null){stage = 'cleanup native SIGTERM exit'; const exited=once(processOwner,'exit');processOwner.kill('SIGTERM');await exited;}
    stage = 'cleanup fixture removal'; fixture.cleanup();
    stage = 'cleanup provider stop'; await provider.stop();
  }
},120_000);
