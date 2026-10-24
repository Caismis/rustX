import { traceFieldKey } from '../../../protocol/app-server/trace-identity.ts';
import { attemptIdentityField, attemptIdentitySourceKey } from '../../../protocol/app-server/attempt-identity.ts';
import { contentRefKey } from '../../../protocol/app-server/content-identity.ts';
import { ContentCache } from '../../../protocol/app-server/content-cache.ts';
import { materializeSource } from '../../../protocol/app-server/source-observation.ts';
import { isAttemptActive } from "../presentation/state.ts";
import { archiveDestination } from "../app-server/archive.ts";
import { renderRepairInspection, type SourceRepairObservation } from "../presentation/source-repairs.ts";
/**
 * The command/input dispatcher.
 *
 * ```text
 * editor submission
 *   -> parseCommandLine
 *        |
 *        +-- a command  -> render projection state, or invoke ONE canonical
 *        |                 Runtime Client operation
 *        +-- plain text -> submit_inbound
 * ```
 *
 * The dispatcher produces *typed presentation intents* — an inspection view,
 * transient feedback, an overlay to open, or a quit request — rather than
 * touching the terminal itself. That keeps it testable without a real
 * terminal and keeps Pi at the outermost layer.
 *
 * What it must never do: read `rustx.toml`, resolve a credential, execute a
 * tool, read a `SKILL.md`, compose an Agent Status, drain a mailbox, or reach
 * a provider. Every one of those is Rust-owned, and several are reachable
 * only through operations this file calls.
 */

import type { AppServerHost } from "../app-server/host.ts";
import type { AppServerSession } from "../app-server/session.ts";
import { capabilitySummary, describeProfile, sessionLabel, skills } from "../presentation/selectors.ts";
import { isTodoComposed, selectTodos } from "../presentation/todos.ts";
import type { PresentationState } from "../presentation/state.ts";
import { renderTodoInspection, deferredTodoRecords } from "../ui/components/todos.ts";
import { parseCommandLine } from "./registry.ts";
import { expandPreference, failure, focusTransition, inspect, reasoningPreference, transient, usage, type CommandOutcome } from "./outcome.ts";
import { goalSummary, renderDebug, renderHelp, renderModel, renderSettings, renderSkills, renderStatus, renderTools, type DebugDiagnostics } from "./presentation.ts";
import type { CatalogModelView, SessionSettings, SessionUserMessageBoundaryObservation } from "../protocol/app-server.ts";

export interface DispatcherContext {
  /** The connection and its durable Session catalog. */
  host: AppServerHost;
  /** The Session currently in focus. */
  session: AppServerSession | undefined;
  /** `session/create` inputs for Sessions this client creates. */
  sessionSettings: SessionSettings;
  /** Bounded diagnostics the UI owns, surfaced by `/debug`. */
  diagnostics: () => DebugDiagnostics;
}

const NO_CURRENT_WORKSPACE = 'No Session is focused, so there is no current Workspace. Name one with /settings workspace "<path>" repairs.';

export class CommandDispatcher {
  readonly #occupancyCuts = new WeakMap<AppServerSession, {domain:string;cut:import('../../../protocol/app-server/v57.ts').CheckpointReadCut}>();
  readonly #attemptCuts=new WeakMap<AppServerSession,{identity:string;cut:import('../../../protocol/app-server/v57.ts').CheckpointReadCut}>();
  #inspectionView = {};
  readonly #observationPages = new WeakMap<AppServerSession, import('../../../protocol/app-server/v57.ts').CheckpointPage>();
  readonly #agentPages = new WeakMap<AppServerSession, import('../../../protocol/app-server/v57.ts').CheckpointPage>();
  #context: DispatcherContext;
  // A retained command is one submission until its response is classified.
  // Other controls and commands (including on a replacement attachment) remain usable.
  readonly #agentMessageSubmissions = new WeakSet<AppServerSession>();
  readonly #observationReads = new WeakMap<AppServerSession,object>();
  readonly #contentDecoders = new WeakMap<AppServerSession, { identity: string; end: number; extent?:number; representation?:string;view?:object; decoder: InstanceType<typeof TextDecoder> }>();
  readonly #contentCaches = new WeakMap<AppServerSession, ContentCache>();
  #inspected = new Map<string, import('../../../protocol/app-server/v57.ts').AvailableConfiguration>();

  constructor(context: DispatcherContext) {
    this.#context = context;
  }

  /**
   * Rebinds admission routing after the visible Session changed.
   *
   * This only affects invocations admitted after the rebind. Every public
   * operation captures the attachment at its start and passes that exact
   * attachment through all awaited phases, so an admitted command cannot
   * retarget a newer attachment.
   */
  setHost(host: AppServerHost): void {
    this.#context.host = host;
  }

  setSession(session: AppServerSession | undefined): void {
    if(this.#context.session!==session)this.#inspectionView={};
    this.#context.session = session;
  }

  /**
   * Handles one editor submission.
   *
   * Plain text is always an ordinary inbound message. Questionnaire answers
   * are sent only by the focused questionnaire surface, which constructs the
   * typed whole-questionnaire response.
   */
  async submit(line: string): Promise<CommandOutcome> {
    const globalCommand = parseCommandLine(line);
    if (globalCommand?.name === "/settings") {
      try { return await this.#settings(globalCommand.argument); } catch (error) { return failure(error); }
    }
    const session = this.#context.session;
    if (session === undefined) return transient("info", "choose a Session before submitting commands");
    const command = parseCommandLine(line);
    if (command === undefined) {
      const text = line.trim();
      if (text.length === 0) {
        return { kind: "none" };
      }
      try {
        if (isAttemptActive(session.state)) await session.steer([{ type: "text", text }]);
        else await session.submitInbound([{ type: "text", text }]);
        return { kind: "none" };
      } catch (error) {
        return failure(error);
      }
    }
    return this.#dispatch(session, command.name, command.argument);
  }

  async #dispatch(
    session: AppServerSession,
    name: string,
    argument: string,
  ): Promise<CommandOutcome> {
    const state = session.state;

    try {
      switch (name) {
        case "/export": {
          const destination = archiveDestination(session.sessionId, argument);
          const path = await this.#context.host.exportSession(session.sessionId, destination);
          return { kind: "transient", level: "info", text: `Session archive saved to ${path}` };
        }
        case "/help":
          return inspect("Help", renderHelp());
        case '/observation': {
          this.#observationReads.set(session,{});
          const [domain,rawOffset='0',...extra]=argument.trim().split(/\s+/);
          if(!domain)return inspect('Observation domains',session.observationCollections.map(({frontier})=>`${frontier.domain} · ${frontier.total} items · /observation ${frontier.domain}`).join('\n')+'\n'+(session.checkpoint?.snapshot.trace.records??[]).map(record=>`${record.id} · ${record.kind} · /observation trace ${record.id}`).join('\n')+'\n/observation <domain> [page-offset] reads one native bounded page.');
          if(domain==='trace'){
            if(rawOffset==='0'||extra.length)return usage('/observation trace <native-record-id>');
            const view=this.#inspectionView;
            const {head,page}=await session.inspectTrace(rawOffset,()=>view===this.#inspectionView&&this.#context.session===session);
            if(this.#context.session!==session)return {kind:'none'};
            this.#observationPages.set(session,page);
            const definition=head.content.type==='tool'?`\nTool definition: ${head.content.definition.type}.`:'';
            const messages=head.content.type==='messages'?`\n${head.content.page.total} original canonical messages · /observation ${head.content.page.domain}`:'';
            return inspect(`Trace · ${head.id}`,`${head.kind} · ${head.record.state}\n${head.record.preview?.text??''}${definition}${messages}\n/content lists the selected original fields; each read retains this native inspection cut.`);
          }
          const offset=Number(rawOffset),previous=offset?this.#observationPages.get(session):undefined;
          if(!Number.isSafeInteger(offset)||offset<0||offset&&previous?.domain!==domain)return usage('/observation <domain> [displayed-page-offset]');
          const view=this.#inspectionView,operation={};this.#observationReads.set(session,operation);
          const page=await session.observationPage(domain,offset,previous,()=>this.#context.session===session&&view===this.#inspectionView&&this.#observationReads.get(session)===operation);
          if(this.#context.session!==session||view!==this.#inspectionView||this.#observationReads.get(session)!==operation)return {kind:'none'};
          this.#observationPages.set(session,page);
          return inspect(`Observation · ${domain}`,page.items.map(item=>`${item.offset}. ${item.identity} · ${JSON.stringify(item.value)}${item.deferred?' · deferred content':''}`).join('\n')+(page.next_offset==null?'\nEnd of page domain.':`\n/observation ${domain} ${page.next_offset} reads the next bounded page.`)+(/^messages\..+\.(?:blocks|tools)$/.test(domain)?'\n/content lists the exact advertised original byte references.':'\n/observation-value <item-offset> <byte-offset> [path-json] reads selected source bytes.'));
        }
        case '/observation-value': {
          const match=/^(\d+)\s+(\d+)(?:\s+(.+))?$/.exec(argument.trim()),page=this.#observationPages.get(session);
          if(!match||!page)return usage('/observation-value <displayed-item-offset> <byte-offset> [path-json]');
          const itemOffset=Number(match[1]),offset=Number(match[2]);
          const path:unknown=match[3]?JSON.parse(match[3]):[];
          if(!Array.isArray(path)||path.length>32||path.some(part=>typeof part!=='string'))return usage('/observation-value <displayed-item-offset> <byte-offset> [path-json]');
          const identity=`observation:${JSON.stringify(page.read_cut)}:${page.domain}:${itemOffset}:${JSON.stringify(path)}`,prior=this.#contentDecoders.get(session),view=this.#inspectionView,operation={};
          if(offset&&(!prior||prior.identity!==identity||prior.view!==view||prior.end!==offset||prior.extent==null))throw new Error('Read the selected observation from byte zero first.');
          this.#observationReads.set(session,operation);
          const current=()=>this.#context.session===session&&this.#inspectionView===view&&this.#observationPages.get(session)===page&&this.#observationReads.get(session)===operation;
          const range=await session.observationValue(page,itemOffset,path,offset,current,offset?prior?.extent:undefined);
          if(!current())throw new Error('Observation viewport was replaced.');
          if(offset&&prior?.representation!==range.representation)throw new Error('Selected observation representation changed.');
          const decoder=offset&&prior?prior.decoder:new TextDecoder('utf-8', {ignoreBOM: true});
          const text=decoder.decode(range.bytes,{stream:range.next!==undefined});this.#contentDecoders.set(session,{identity,end:offset+range.bytes.length,extent:range.end,representation:range.representation,view,decoder});
          return inspect('Selected observation bytes',`${offset}–${offset+range.bytes.length} / ${range.end} bytes\n${text}\n${range.next===undefined?'End of selected value.':`/observation-value ${itemOffset} ${range.next} ${JSON.stringify(path)} reads the next byte range.`}`);
        }
        case '/context-model': {
          const view=this.#inspectionView,operation={};this.#observationReads.set(session,operation);
          const readCurrent=()=>this.#context.session===session&&this.#inspectionView===view&&this.#observationReads.get(session)===operation;
          const occupancy = session.state.context.occupancy;
          if (!occupancy) return inspect('Context model', 'No native context reading has been published.');
          if (!occupancy.model_deferred) {
            if (occupancy.model == null) throw new Error('Original context model unavailable.');
            return inspect('Context model', occupancy.model);
          }
          const offset = argument.trim() === '' ? 0 : Number(argument);
          if (!Number.isSafeInteger(offset) || offset < 0) return usage('/context-model [byte-offset]');
          const previous = this.#occupancyCuts.get(session), domain = occupancy.model_source;
          if (!domain || offset > 0 && previous?.domain !== domain) throw new Error('Load the original context model from byte zero first.');
          const range = await session.readOccupancyModel(offset, offset === 0 ? undefined : previous?.cut);
          if (!readCurrent() || session.state.context.occupancy?.model_source !== domain)
            throw new Error('Context model viewport was replaced.');
          this.#occupancyCuts.set(session,{domain,cut:range.cut});
          const identity=`occupancy:${domain}:${JSON.stringify(range.cut)}`, prior=this.#contentDecoders.get(session);
          const decoder=prior?.identity===identity && prior.end===offset ? prior.decoder : new TextDecoder('utf-8',{ignoreBOM:true});
          const text=decoder.decode(range.bytes,{stream:range.next!==undefined});
          this.#contentDecoders.set(session,{identity,end:offset+range.bytes.length,decoder});
          return inspect('Original context model bytes',`${offset}–${offset+range.bytes.length} / ${range.end} bytes\n${text}\n${range.next===undefined?'End of model field.':`/context-model ${range.next} reads the next byte range.`}`);
        }
        case "/content": {
          const view=this.#inspectionView,operation={};this.#observationReads.set(session,operation);
          const readCurrent=()=>this.#context.session===session&&this.#inspectionView===view&&this.#observationReads.get(session)===operation;
          const ranges = session.deferredContent;
          const nativeFields=session.deferredJournalFields,attemptFields=session.deferredAttemptIdentities,traceFields=session.deferredTraceFields,observationFields=session.observationContent;
          if (!argument) return inspect('Deferred content', [...ranges.map((binding, index) => `${index + 1}. ${binding.path.join('/')} · ${binding.content_ref.readable_end} bytes`),...nativeFields.map((field,index)=>`${ranges.length+index+1}. ${field.label} ${field.reference.sequence} · ${field.reference.encoded_bytes} bytes`),...attemptFields.map((field,index)=>`${ranges.length+nativeFields.length+index+1}. Turn owner identity · ${attemptIdentityField(field.source).encodedBytes} bytes`),...traceFields.map((field,index)=>`${ranges.length+nativeFields.length+attemptFields.length+index+1}. ${field.record} · ${field.source.field} · ${field.source.type==='value'?field.source.encoded_bytes:field.source.binding.content_ref.readable_end} bytes`),...observationFields.map((field,index)=>`${ranges.length+nativeFields.length+attemptFields.length+traceFields.length+index+1}. ${field.page.domain} · ${field.item}/${field.binding.path.join('/')} · ${field.binding.content_ref.readable_end} bytes`)].join('\n') + '\n/content <range-number> [byte-offset] reads one bounded range.');
          const [selected, start = '0'] = argument.trim().split(/\s+/), index = Number(selected) - 1, offset = Number(start);
          if (!Number.isSafeInteger(index) || index < 0 || !Number.isSafeInteger(offset) || offset < 0 || !ranges[index]&&!nativeFields[index-ranges.length]&&!attemptFields[index-ranges.length-nativeFields.length]&&!traceFields[index-ranges.length-nativeFields.length-attemptFields.length]&&!observationFields[index-ranges.length-nativeFields.length-attemptFields.length-traceFields.length]) return usage('/content <range-number> [byte-offset]');
          const nativeField=nativeFields[index-ranges.length];
          if(nativeField){
            const identity=`journal:${JSON.stringify(nativeField.reference)}`,range=await session.readJournalField(nativeField.reference,offset);
            if(!readCurrent() ||JSON.stringify(session.deferredJournalFields[index-ranges.length]?.reference)!==JSON.stringify(nativeField.reference))throw new Error('Native field view was replaced.');
            const prior=this.#contentDecoders.get(session),decoder=prior?.identity===identity&&prior.end===offset?prior.decoder:new TextDecoder('utf-8', {ignoreBOM: true});
            const text=decoder.decode(range.bytes,{stream:range.next!==undefined});this.#contentDecoders.set(session,{identity,end:offset+range.bytes.length,decoder});
            return inspect(nativeField.label + ' bytes',`${offset}–${offset+range.bytes.length} / ${nativeField.reference.encoded_bytes} bytes\n${text}\n${range.next===undefined?'End of native field.':`/content ${index+1} ${range.next} loads the next byte range.`}`);
          }
          const attemptField=attemptFields[index-ranges.length-nativeFields.length];
          if(attemptField){
            const identity=attemptIdentitySourceKey(attemptField.source),previous=this.#attemptCuts.get(session);
            if(offset>0&&previous?.identity!==identity)throw new Error('Read the original attempt identity from byte zero first.');
            const range=await session.readAttemptIdentity(attemptField.source,offset,offset===0?undefined:previous?.cut);
            const current=session.deferredAttemptIdentities[index-ranges.length-nativeFields.length];
            if(!readCurrent()||!current||attemptIdentitySourceKey(current.source)!==identity)throw new Error('Native owner identity view was replaced.');
            this.#attemptCuts.set(session,{identity,cut:range.cut});
            const decoderIdentity=`attempt:${identity}:${JSON.stringify(range.cut)}`,prior=this.#contentDecoders.get(session),decoder=prior?.identity===decoderIdentity&&prior.end===offset?prior.decoder:new TextDecoder('utf-8',{ignoreBOM:true});
            const text=decoder.decode(range.bytes,{stream:range.next!==undefined});this.#contentDecoders.set(session,{identity:decoderIdentity,end:offset+range.bytes.length,decoder});
            return inspect('Original attempt identity bytes',`${offset}–${offset+range.bytes.length} / ${range.end} bytes\n${text}\n${range.next===undefined?'End of original identity.':`/content ${index+1} ${range.next} loads the next byte range.`}`);
          }
          const traceField=traceFields[index-ranges.length-nativeFields.length-attemptFields.length];
          if(traceField){
            const identity=`trace:${traceFieldKey(traceField.source)}:${JSON.stringify(traceField.cut)}`;
            const range=await session.readTraceField(traceField,offset);
            if(!readCurrent())throw new Error('Trace field view was replaced.');
            const prior=this.#contentDecoders.get(session),decoder=prior?.identity===identity&&prior.end===offset?prior.decoder:new TextDecoder('utf-8',{ignoreBOM:true});
            const text=range.representation==='binary_bytes'?`Binary bytes (Base64)\n${Buffer.from(range.bytes).toString('base64')}`:decoder.decode(range.bytes,{stream:range.next!==undefined});
            this.#contentDecoders.set(session,{identity,end:offset+range.bytes.length,decoder});
            return inspect(`Original Trace ${traceField.source.field} bytes`,`${offset}–${offset+range.bytes.length} / ${range.end} bytes\n${text}\n${range.next===undefined?'End of original field.':`/content ${index+1} ${range.next} loads the next byte range.`}`);
          }
          const observationField=observationFields[index-ranges.length-nativeFields.length-attemptFields.length-traceFields.length];
          if(observationField){
            const identity=`observation-content:${contentRefKey(observationField.binding.content_ref)}:${JSON.stringify(observationField.cut)}`;
            const range=await session.readObservationContent(observationField,offset);
            if(!readCurrent())throw new Error('Original observation content view was replaced.');
            const prior=this.#contentDecoders.get(session),decoder=prior?.identity===identity&&prior.end===offset?prior.decoder:new TextDecoder('utf-8',{ignoreBOM:true});
            const text=range.representation==='binary_bytes'?`Binary bytes (Base64)\n${Buffer.from(range.bytes).toString('base64')}`:decoder.decode(range.bytes,{stream:range.next!==undefined});
            this.#contentDecoders.set(session,{identity,end:offset+range.bytes.length,decoder});
            return inspect('Original observation content bytes',`${offset}–${offset+range.bytes.length} / ${range.end} bytes\n${text}\n${range.next===undefined?'End of original content.':`/content ${index+1} ${range.next} loads the next byte range.`}`);
          }
          let cache = this.#contentCaches.get(session);
          if (!cache) { cache = new ContentCache((ref, at) => session.readContent(ref, at)); this.#contentCaches.set(session, cache); }
          const ref = ranges[index]!.content_ref, identity = contentRefKey(ref);
          const range = await cache.read(ref, offset, () => readCurrent() && session.deferredContent[index]!=null && contentRefKey(session.deferredContent[index]!.content_ref) === identity);
          if(!readCurrent())throw new Error('Content view was replaced.');
          let text: string;
          if (ref.representation === 'binary_bytes') {
            text = `Binary bytes (Base64)\n${Buffer.from(range.bytes).toString('base64')}`;
          } else {
            const prior = this.#contentDecoders.get(session);
            const decoder = prior?.identity === identity && prior.end === offset ? prior.decoder : new TextDecoder('utf-8', {ignoreBOM: true});
            text = decoder.decode(range.bytes, { stream: range.next !== undefined });
            this.#contentDecoders.set(session, { identity, end: offset + range.bytes.length, decoder });
          }
          return inspect('Deferred content bytes', `${offset}–${offset + range.bytes.length} / ${ref.readable_end} bytes\n${text}\n${range.next === undefined ? 'End of content.' : `/content ${index + 1} ${range.next} loads the next byte range.`}`);
        }
        case "/files":
          // One bounded native transcript page; older pages load on request.
          return { kind: "choose_delivery", page: await session.deliveryPage() };
        case "/model-position": {
          const values=argument.trim().split(/\s+/);
          if(values.length<2||values.length>3||values.some(value=>!/^\d+$/.test(value)))return usage('/model-position <binding-revision> <model-position> [profile-position]');
          const [revision,model,profile]=values;
          if(BigInt(revision!)>18446744073709551615n)return usage('/model-position <binding-revision> <model-position> [profile-position]');
          const modelPosition=Number(model),profilePosition=profile==null?null:Number(profile);
          if(!Number.isSafeInteger(modelPosition)||profilePosition!=null&&!Number.isSafeInteger(profilePosition))return usage('/model-position <binding-revision> <model-position> [profile-position]');
          const receipt=await session.modelSelect({binding_revision:revision!,model_position:modelPosition,profile_position:profilePosition});
          return transient('info',`Native catalog selection confirmed · settings revision ${receipt.settings_revision}; future attempts use its defaults.`);
        }
        case "/model":
          return await this.#model(session, state, argument);
        case "/new":
          return await this.newSession();
        case "/resume":
          return await this.#resume(session, argument);
        case "/session":
          return argument ? await this.#configuration(session, argument) : await this.#sessionInfo(session);
        case "/name":
          return await this.#name(session, argument);
        case "/clone":
          return await this.#clone(session);
        case "/fork":
          return await this.#fork(session);
        case "/tree":
          return await this.#tree(session);
        case "/tools":
          return inspect("Tools", renderTools(state));
        case "/skills":
          return inspect("Skills", renderSkills(state));
        case "/todos":
          return inspect(
            "Todos",
            renderTodoInspection(selectTodos(state), isTodoComposed(state), deferredTodoRecords(session.checkpoint?.collections)),
          );
        case '/workflow-recovery': {
          const [startId,node,after='0',...extra]=argument.trim().split(/\s+/);
          if(!startId || extra.length || !/^(0|[1-9][0-9]*)$/.test(after) || BigInt(after)>18446744073709551615n)return usage('/workflow-recovery <start-id> [node-id] [after-effect-ordinal]');
          const host=this.#context.host,view=this.#inspectionView;
          const current=()=>this.#context.session===session && this.#context.host===host && this.#inspectionView===view;
          const metadata=node?undefined:(await host.client.call('session/read',{session_id:session.sessionId},'session')).session;
          if(!current())return {kind:'none'};
          const nodeId=node??session.nodeId??metadata?.active_node;if(!nodeId)return failure('Native Session node is unavailable.');
          const response=await host.client.call('workflow/scheduled/getRecovery',{session_id:session.sessionId,node_id:nodeId,start_id:startId,after_effect_ordinal:after,limit:20},'scheduled_workflow_recovery');
          if(!current())return {kind:'none'};
          const value=response.recovery;if(!value)return failure('Native scheduled Workflow is unavailable.');
          const state=value.start.state.type==='settled'?value.start.state.outcome.type:value.start.state.type;
          const nativeState=value.start.state;
          const diagnostic=nativeState.type==='accepted'?nativeState.waiting:nativeState.type==='settled' && 'reason' in nativeState.outcome?nativeState.outcome.reason:undefined;
          const lines=[`${startId} · ${state}`,`Session ${session.sessionId} · Node ${nodeId}`,value.capacity_blocked?'Native ownership still blocks capacity and Session deletion.':'Native ownership settled.',...(diagnostic?[diagnostic]:[]),...(value.start.cancellation_requested_at?['Cancellation requested; it is not settlement proof.']:[]),...(value.start.recovery_blocked?[value.start.recovery_blocked]:[]),...value.effects.map(effect=>`${effect.ordinal}. ${effect.node.node} · ${effect.owner.type} · ${effect.physical_settlement_proven?'native proof confirmed':'waiting for native proof'}${effect.diagnostic?` · ${effect.diagnostic}`:''}`),...(value.result==null?[]:[`Committed result:\n${JSON.stringify(value.result,null,2)}`]),...(value.next_effect_ordinal==null?[]:[`/workflow-recovery ${startId} ${nodeId} ${value.next_effect_ordinal}`])];
          return inspect('Scheduled Workflow recovery',lines.join('\n'));
        }
        case '/schedules': {
          const [node, after = '0', ...extra] = argument.trim() ? argument.trim().split(/\s+/) : [];
          if (extra.length || !/^(0|[1-9][0-9]*)$/.test(after) || BigInt(after) > 18446744073709551615n) return usage('/schedules [node-id] [after-ordinal]');
          const host = this.#context.host, view = this.#inspectionView;
          const current = () => this.#context.session === session && this.#context.host === host && this.#inspectionView === view;
          const metadata = node ? undefined : (await host.client.call('session/read', { session_id: session.sessionId }, 'session')).session;
          if (!current()) return { kind: 'none' };
          const nodeId = node ?? session.nodeId ?? metadata?.active_node;
          if (!nodeId) return failure('Native Session node is unavailable.');
          const page = await host.client.call('schedule/list', { session_id: session.sessionId, node_id: nodeId, after_ordinal: after, limit: 20 }, 'schedules');
          if (!current()) return { kind: 'none' };
          const rows = page.schedules.map(item => {
            const rule = item.spec.type === 'at' ? `At ${item.spec.at_utc}` : item.spec.type === 'interval' ? `Every ${item.spec.every_seconds}s; UTC anchor ${item.spec.anchor_utc}` : `Cron ${item.spec.expression}; ${item.spec.time_zone}`;
            const state = item.state === 'completed' ? 'No future occurrence' : item.state;
            return `${item.id.ordinal}. ${item.title} · ${state}\n${item.target.type==='workflow'?`Workflow ${item.target.workflow_id}\n`:''}${rule}\nNext admission: ${item.next_due_utc ?? 'none'}${item.last_trigger ? `\nLatest admission: ${item.last_trigger.accepted_at}` : ''}${item.last_failure ? `\n${item.last_failure}` : ''}`;
          });
          return inspect(`Schedules · ${session.sessionId}/${nodeId}`, (rows.join('\n\n') || 'No schedules on this page.') + (page.next_ordinal == null ? '' : `\n\n/schedules ${nodeId} ${page.next_ordinal}`));
        }
        case "/goal":
          return await this.#goal(session, argument);
        case "/status":
          return inspect("Agent Status", renderStatus(state));
        case "/context": {
          const [action = "history", value, offset] = argument.trim().split(/\s+/);
          if (action === "history") {
            const page = await session.projections(value ?? "0");
            return inspect("Historical model projections", page.records.map(head => `${head.activation_id} · ${head.checkpoint_id} · base ${head.base_frontier} · activation Ledger ${head.activation_ledger} · ${head.destination_installation == null ? "local" : "inherited"} from ${head.origin.conversation_id}/${head.origin.activation}\n${head.summary_preview?.text ?? ""}`).join("\n\n") + (page.next_activation ? `\n\n/context history ${page.next_activation}` : ""));
          }
          if (action === "projection" && value) {
            const view=this.#inspectionView;
            const detail = await session.projectionDetail(value, Number(offset ?? 0),()=>view===this.#inspectionView&&this.#context.session===session);
            if(view!==this.#inspectionView||this.#context.session!==session)return {kind:'none'};
            return inspect("Projected messages", detail.items.map(item => `${item.occurrence.item_index + 1} · ${item.content.role} · ${item.content.provenance.type}\n${JSON.stringify(item.content.blocks, null, 2)}`).join("\n\n") + (detail.next_item == null ? "" : `\n\n/context projection ${value} ${detail.next_item}`) + "\n/content lists advertised original provenance fields at this returned cut.");
          }
          return usage("/context history [activation] | projection <checkpoint> [offset]");
        }
        case "/compact":
          return await this.#compact(session, argument);
        case "/debug":
          return inspect("Client diagnostics", renderDebug(state, this.#context.diagnostics()));
        case "/show-reasoning":
          return reasoningPreference(argument);
        case "/expand":
          return expandPreference(argument);
        case "/cancel":
          return await this.#cancel(session, argument);
        case "/agents": {
          const offset=argument ? Number(argument) : 0;
          if(!Number.isSafeInteger(offset)||offset<0)return usage('/agents [page-offset]');
          const previous=offset ? this.#agentPages.get(session) : undefined;
          if(offset&&!previous)return transient('error','Open /agents before following its continuation.');
          const {page,agents}=await session.agentPage(offset,previous);
          if(this.#context.session!==session)return {kind:'none'};
          this.#agentPages.set(session,page);
          return inspect('Agents', agents.map(agent=>`- ${agent.agent_id} · ${agent.agent} · ${agent.state} · activation ${agent.current_activation ?? "none"} · conversation ${agent.child_conversation_id}`).join('\n') + (page.next_offset==null?'\nEnd of Agents.':`\n/agents ${page.next_offset} loads the next bounded page.`) + '\n/agent-read <agent-id> opens its observation.');
        }
        case '/agent-read':
          if(!argument.trim()||/\s/.test(argument.trim()))return usage('/agent-read <agent-id>');
          return {kind:'inspect_agent',agentId:argument.trim()};
        case "/send-message": {
          const split = argument.indexOf(" ");
          if (split < 1 || !argument.slice(split + 1).trim()) return usage("/send-message <agent-id> <message>");
          if (this.#agentMessageSubmissions.has(session)) return transient("error", "An Agent message submission is still awaiting its outcome. Not sent again.");
          this.#agentMessageSubmissions.add(session);
          try {
            await session.sendMessage(argument.slice(0, split), argument.slice(split + 1).trim());
          } finally {
            this.#agentMessageSubmissions.delete(session);
          }
          return transient("info", "Message admitted by the Agent runtime.");
        }
        case "/wait-agent": {
          if (!argument) return usage("/wait-agent <agent-id>");
          const target = await session.waitAgent(argument);
          return transient("info", target.activation_id == null
            ? `${target.agent_id} was inactive at the wait boundary.`
            : target.outcome == null
              ? `${target.agent_id}: admission ${target.activation_id} ended before an activation committed.`
              : `${target.agent_id}: captured activation ${target.activation_id} settled (${target.outcome}).`);
        }
        case "/interrupt-agent": {
          if (!argument) return usage("/interrupt-agent <agent-id>");
          const target = await session.interruptAgent(argument);
          return transient("info", target.activation_id == null
            ? `${target.agent_id} was inactive at the interrupt boundary.`
            : `${target.agent_id}: interruption intent accepted for ${target.activation_id}; settlement remains observable through /wait-agent.`);
        }
        case "/jobs": {
          const listing = await session.listJobs();
          const rows = listing.jobs.map(job => `- ${job.job_id} · ${job.tool_name} · ${job.state}${job.deferred_fields?.length ? "; fields deferred: " + (job.deferred_fields ?? []).map(path => path.join(".")).join(", ") + "; /observation loads bounded original bytes" : ""}`).join("\n") || "No Jobs.";
          return inspect("Jobs", `${rows}\nReturned ${listing.returned} of ${listing.matched} Jobs (limit ${listing.limit})${listing.truncated ? "; older Jobs omitted" : ""}.`);
        }
        case "/job-status":
        case "/job-wait":
        case "/job-cancel": {
          if (!argument) return usage(`${name} <job-id>`);
          if (name === "/job-cancel") {
            const receipt = await session.cancelJob(argument);
            return inspect("Job", `${receipt.job_id}: cancellation intent accepted; /job-status or /job-wait reads native settlement.`);
          }
          const job = name === "/job-status" ? await session.jobStatus(argument) : await session.waitJob(argument);
          return inspect("Job", `- ${job.job_id} · ${job.tool_name} · ${job.state}${job.deferred_fields?.length ? "; fields deferred: " + (job.deferred_fields ?? []).map(path => path.join(".")).join(", ") + "; /observation loads bounded original bytes" : ""}`);
        }
        case "/quit":
          return { kind: "quit" };
        default:
          return transient("error", `unknown command ${name}. Try /help.`);
      }
    } catch (error) {
      return failure(error);
    }
  }

  /**
   * Selection seams used by the native-data overlays.
   *
   * Every one of them ends in a focus change. None of them replaces a process,
   * detaches another Session, or asks the server to stop anything.
   */
  selectSession(sessionId: string): CommandOutcome {
    return { kind: "focus_session", sessionId };
  }

  selectTreeNode(sessionId: string, nodeId: string): CommandOutcome {
    return { kind: "focus_session", sessionId, nodeId };
  }

  /** Creates an independent Session from an exact historical boundary. */
  async forkAt(
    boundary: SessionUserMessageBoundaryObservation,
  ): Promise<CommandOutcome> {
    const session = this.#context.session;
    if (session === undefined) return transient("info", "choose a Session before submitting commands");
    try {
      const forked = await this.#context.host.forkSession(
        session.sessionId,
        boundary.surface_revision,
        boundary.locator,
        session.nodeId,
      );
      return focusTransition(forked, "forked");
    } catch (error) {
      return failure(error);
    }
  }

  /** Creates a branch node inside the same Session graph. */
  async branchAt(
    boundary: SessionUserMessageBoundaryObservation,
  ): Promise<CommandOutcome> {
    const session = this.#context.session;
    if (session === undefined) return transient("info", "choose a Session before submitting commands");
    try {
      const current = await this.#context.host.readSession(session.sessionId);
      const branched = await this.#context.host.branchSession(
        session.sessionId,
        session.nodeId ?? current.active_node,
        boundary.surface_revision,
        boundary.locator,
      );
      return focusTransition(branched, "branched");
    } catch (error) {
      return failure(error);
    }
  }

  /** Creates a fresh Session from this client's Session settings. */
  async newSession(): Promise<CommandOutcome> {
    try {
      const created = await this.#context.host.createSession(
        this.#context.sessionSettings,
      );
      return focusTransition(created, "created");
    } catch (error) {
      return failure(error);
    }
  }

  /**
   * Clones the exact committed current canonical head.
   *
   * The head revision is read authoritatively first, so the copy means one
   * exact conversation cut even if the Session commits again immediately
   * afterwards.
   */
  async #clone(session: AppServerSession): Promise<CommandOutcome> {
    const head = await session.boundaries(0, 1);
    const cloned = await this.#context.host.forkSession(
      session.sessionId,
      head.surfaceRevision,
      undefined,
      session.nodeId,
    );
    return focusTransition(cloned, "cloned");
  }

  async #resume(
    session: AppServerSession,
    argument: string,
  ): Promise<CommandOutcome> {
    if (argument.length > 0) return this.selectSession(argument);
    const page = await this.#context.host.listSessions();
    return {
      kind: "choose_session",
      sessions: page.sessions,
      nextOffset: page.nextOffset,
      query: "",
    };
  }

  async #sessionInfo(session: AppServerSession): Promise<CommandOutcome> {
    const refreshed = await this.#context.host.readSession(session.sessionId);
    return inspect(
      "Session",
      [
        // An unnamed Session has no name line rather than a placeholder one:
        // the identity below is what it is actually known by.
        ...(refreshed.name == null ? [] : [`name ${refreshed.name}`]),
        `session ${refreshed.id}`,
        `active node ${refreshed.active_node}`,
        `conversation ${refreshed.active_conversation_id}`,
        `nodes ${refreshed.node_count}`,
      ].join("\n"),
    );
  }

  async #compact(
    session: AppServerSession,
    argument: string,
  ): Promise<CommandOutcome> {
    if (argument.length > 0) {
      return transient("error", "usage: /compact");
    }
    const receipt = await session.compactContext();
    return transient(
      "info",
      `context compacted to generation ${receipt.generation}: ${receipt.tokens_before.input_tokens} → ${receipt.estimated_tokens_after} tokens`,
    );
  }

  async #goal(session: AppServerSession, argument: string): Promise<CommandOutcome> {
    const [action = "show", ...words] = argument.trim().split(/\s+/);
    const text = words.join(" ");
    if (action === "create") {
      if (!text) return transient("error", "usage: /goal create <objective>");
      // A typed native Goal control. Creating a Goal never sends a model
      // request, and nothing follows it: an Active Goal continues on its own
      // whenever the runtime reaches an eligible idle boundary.
      return inspect("Goal", goalSummary(await session.goal({ action: "create", objective: text, budget: 10 })));
    }
    const view = await session.goal({ action: "show" });
    if (!action || action === "show") return inspect("Goal", goalSummary(view));
    if (!view.current) return transient("error", "No current Goal. Use /goal create <objective>.");
    let mutation: import("../protocol/app-server.ts").GoalMutation;
    if ((action === "pause" || action === "resume") && !text) mutation = { action };
    else if (action === "edit" && text) mutation = { action: "edit", objective: text };
    else if (action === "budget" && /^\d+$/.test(text)) mutation = { action: "budget", rounds: Number(text) };
    else return transient("error", "usage: /goal [show | create <objective> | pause | resume | edit <objective> | budget <rounds>]");
    const updated = await session.goal({ action: "mutate", expected: view.current.reference, mutation });
    return inspect("Goal", goalSummary(updated));
  }

  async #settings(argument: string): Promise<CommandOutcome> {
    const words = argument.match(/"(?:[^"\\]|\\.)*"|\S+/g)?.map(word => word.startsWith('"') ? JSON.parse(word) as string : word) ?? [];
    const usage = transient("error", 'usage: /settings [repairs] | [user | workspace ["<canonical absolute path>"]] [repairs | rescan | approval policy|full_access|inherit]');
    if (words.length === 1 && words[0] === "repairs") return this.#repairs("current");
    const owner = words.shift() ?? "user";
    let target: import("../../../protocol/app-server/v57.ts").SourceTarget;
    if (owner === "user") target = { kind: "user" };
    else if (owner === "workspace" && words[0] === "repairs" && words.length === 1) return this.#repairs("current workspace");
    else if (owner === "workspace" && words[0]) target = { kind: "workspace", directory: words.shift()! };
    else return usage;
    if (words[0] === "repairs") return words.length === 1 ? this.#repairs(target) : usage;
    const action = words.shift();
    const client = this.#context.host.client;
    const readSource = async () => materializeSource((await client.call('configuration/sourcesRead', { target }, 'source_settings')).projection,
      async (reference, offset) => (await client.call('configuration/sourceValueRead', { target, reference, offset, max_bytes: 65_536 }, 'source_value_chunk')).chunk,
      () => true);
    let source = await readSource();
    if (action === "rescan") await client.call("configuration/reconcile", { target }, "configuration_receipt");
    else if (action === "approval") {
      const mode = words.shift();
      if (mode !== "policy" && mode !== "full_access" && mode !== "inherit") return transient("error", "approval expects policy, full_access, or inherit");
      await client.call("configuration/sourceWrite", { target, expected_revision: source[target.kind]!.revision,
        mutation: { kind: "config", mutation: { unit: "approval", authored: mode === "inherit" ? null : mode } } }, "sources_written");
    } else if (action) return transient("error", "Unknown Settings action");
    if (action) source = await readSource();
    return inspect(target.kind === "user" ? "User Settings" : "Workspace Settings", JSON.stringify(source, null, 2));
  }

  /**
   * The redacted native repair receipts of the User source, a Workspace
   * source, or both, read authoritatively. Read-only: this never triggers a
   * repair, and nothing in it reaches the conversation.
   *
   * `"current"` is the User and the current Workspace; `"current workspace"`
   * the current Workspace alone. The current Workspace is the one the focused
   * Session is natively bound to — its canonical durable cwd, read from the
   * App Server (`session/summary`, which loads no runtime) — never this
   * client's Session-creation defaults, and never a path resolved on this
   * machine, which may not be the App Server's. The App Server and Session
   * are captured when the command starts: a later focus change does not
   * retarget it, and a result from a replaced App Server is never shown.
   */
  async #repairs(
    target: import("../../../protocol/app-server/v57.ts").SourceTarget | "current" | "current workspace",
  ): Promise<CommandOutcome> {
    const { host, session } = this.#context;
    let workspace: import("../../../protocol/app-server/v57.ts").SourceTarget | undefined;
    if (typeof target === "string") {
      if (session !== undefined) {
        const { summary } = await host.client.call("session/summary", { session_id: session.sessionId }, "session_summary");
        workspace = { kind: "workspace", directory: summary.cwd };
      } else if (target === "current workspace") {
        return transient("error", NO_CURRENT_WORKSPACE);
      }
    } else if (target.kind === "workspace") {
      workspace = target;
    }
    const projection = await host.readSourceRepairs(workspace ?? { kind: "user" });
    if (this.#context.host !== host) return transient("error", "The App Server connection changed during the inspection; nothing was shown. Run it again.");
    const observations: SourceRepairObservation[] = [];
    if (target === "current" || (typeof target !== "string" && target.kind === "user")) {
      observations.push({ target: { kind: "user" }, receipt: projection.user_repair ?? undefined, sequence: projection.repair_sequence });
    }
    if (workspace !== undefined) {
      observations.push({ target: projection.target, receipt: projection.workspace_repair ?? undefined, sequence: projection.repair_sequence });
    }
    const body = renderRepairInspection(observations);
    return inspect("Configuration repairs", target === "current" && workspace === undefined ? `${body}\n\n${NO_CURRENT_WORKSPACE}` : body);
  }

  async #configuration(session: AppServerSession, argument: string): Promise<CommandOutcome> {
    if (argument === "adopt") {
      const candidate = this.#inspected.get(session.sessionId);
      if (!candidate) return transient("error", "Inspect /session settings before adopting.");
      try { await session.adoptConfiguration(candidate); }
      catch (error) {
        const authority = await session.readConfiguration();
        if (authority?.candidate) this.#inspected.set(session.sessionId, authority.candidate);
        else this.#inspected.delete(session.sessionId);
        return transient("error", String(error) + "\nNot replayed. Native authority: " + JSON.stringify(authority));
      }
    } else if (argument !== "settings") return transient("error", "usage: /session [settings | adopt]");
    const application = await session.readConfiguration();
    if (application?.candidate) this.#inspected.set(session.sessionId, application.candidate);
    else this.#inspected.delete(session.sessionId);
    return inspect("Session settings", [
      renderSettings(session.state, await session.configuration()),
      JSON.stringify(application, null, 2),
      // Eligibility is the runtime's own advisory publication, not part of
      // the configuration application; native adoption revalidates it.
      `Runtime adoption eligibility: ${session.state.adoptionEligibility} (advisory; native adoption revalidates).`,
      application?.candidate ? "Inspect the exact candidate above; /session adopt explicitly adopts it." : "No pending candidate reported by native authority.",
    ].join("\n"));
  }

  /**
   * `/name` shows the active Session's name, and `/name <text>` sets it.
   *
   * Reporting the current name is the useful answer to a bare `/name`: a
   * Session is unnamed until someone names it, so "this one has no name" is
   * the fact the user is asking about, not a syntax mistake to correct.
   */
  async #name(
    session: AppServerSession,
    argument: string,
  ): Promise<CommandOutcome> {
    const host = this.#context.host;
    if (argument.trim().length === 0) {
      const current = await host.readSession(session.sessionId);
      return transient(
        "info",
        current.name == null
          ? `session ${current.id} is unnamed; use /name <text> to name it`
          : `session name: ${current.name}`,
      );
    }
    const named = await host.renameSession(session.sessionId, argument);
    return transient("info", `session named ${sessionLabel(named)}`);
  }

  async #fork(session: AppServerSession): Promise<CommandOutcome> {
    const page = await session.boundaries();
    return {
      kind: "choose_fork",
      boundaries: page.boundaries,
      nextOffset: page.nextOffset,
    };
  }

  async #tree(session: AppServerSession): Promise<CommandOutcome> {
    // The graph is a durable catalog read and the boundaries are a live Surface
    // read: two owners, two requests, and no third place that could disagree.
    const host = this.#context.host;
    const [current, tree, page] = await Promise.all([
      host.readSession(session.sessionId),
      host.sessionTree(session.sessionId),
      session.boundaries(),
    ]);
    return {
      kind: "choose_tree",
      session: current,
      nodes: tree.nodes,
      nextNodeOffset: tree.nextOffset,
      boundaries: page.boundaries,
      nextHistoryOffset: page.nextOffset,
    };
  }

  /**
   * `/model` — a presentation over the runtime's authoritative model
   * operations.
   *
   * With no argument it opens the searchable selector over the runtime's
   * `model_catalog_get` result. With `show` it renders the projection's own
   * model view. With a model reference it reads the catalog and replaces the
   * whole runtime configuration through `model_set`. It never parses a
   * provider catalog file, never touches a provider SDK, and never resolves
   * an API key.
   */
  async #model(
    session: AppServerSession,
    state: PresentationState,
    argument: string,
  ): Promise<CommandOutcome> {
    // `show` is answered from the projection alone; every other spelling
    // needs the runtime's authoritative catalog.
    if (/^profile(?:\s|$)/.test(argument)) {
      const parts = argument.split(/\s+/);
      const clear = parts.length === 2 && parts[1] === "clear";
      const profile = parts.length === 3 && parts[1] === "set" ? parts[2] : undefined;
      if (!clear && !profile) return transient("error", "usage: /model profile set <id> | /model profile clear");
      const current = await session.modelGet();
      const configured = { ...current.configured };
      if (clear) delete configured.profile;
      else if (profile !== undefined) configured.profile = profile;
      const updated = await session.modelSet(configured);
      return transient("info", withModelOutcomeNotice(session, `Session model profile -> ${updated.configured.profile ?? "model default"}; future independent attempts. Running attempts stay frozen.`));
    }
    if (argument === "show") {
      return inspect("Model", withModelOutcomeNotice(session, renderModel(state)));
    }
    const page = argument.match(/^page (\d+)$/);
    const catalog = await session.modelCatalog(page ? Number(page[1]) : undefined);
    const models = catalog.models ?? [];
    if (argument.length === 0 || argument === "list" || page) {
      return { kind: "choose_model", models };
    }

    const chosen = models.find((model) => model.model === argument);
    if (chosen === undefined) {
      const known = models.map((model) => model.model).join(", ");
      return transient(
        "error",
        `${argument} is not in the runtime's catalog. Selectable: ${known || "none"}`,
      );
    }
    return this.#selectModel(session, chosen);
  }

  /**
   * Applies one catalog selection as a whole-state replacement.
   *
   * The update affects future admissions only. An already-admitted attempt
   * keeps the model it froze — the runtime enforces that, and the client
   * simply reports both facts truthfully.
   */
  async selectModel(model: CatalogModelView): Promise<CommandOutcome> {
    const session = this.#context.session;
    if (session === undefined) return transient("info", "choose a Session before submitting commands");
    return this.#selectModel(session, model);
  }

  async #selectModel(
    session: AppServerSession,
    model: CatalogModelView,
  ): Promise<CommandOutcome> {
    try {
      const current = (await session.modelGet()).configured;
      const sameModel = current.model === model.model;

      // `/model X` is a deliberate whole-state replacement: the selected
      // primary model gets its own runtime defaults — the runtime resolves
      // its default profile — while the independently configured summary
      // policy is copied from the authoritative current configuration
      // unchanged.
      // Model-name equality cannot prove an explicit durable selection. Send
      // the complete current configuration and let its native owner decide
      // whether to persist inherited intent or return a genuine NoOp.
      const replacement = sameModel ? current : {
        model: model.model,
        requestParams: {},
        summaryModel: current.summaryModel,
        titleModel: current.titleModel,
        classificationModel: current.classificationModel,
        btwModel: current.btwModel,
      };
      const updated = await session.modelSet({
        ...replacement,
      });
      const selectionNote = sameModel
        ? "profile, overrides and auxiliary policies preserved"
        : "primary overrides reset; auxiliary policies preserved";

      const attempt = session.state?.attempt;
      const attemptNote =
        attempt !== undefined && attempt.phase.type === "running"
          ? `current attempt remains ${attempt.model?.primary.model ?? "unavailable"}${attempt.model ? ` (profile ${describeProfile(attempt.model.primary)})` : ""}`
          : "";
      const modelFeedback = attemptNote.length > 0
        ? [
            `session model -> ${updated.configured.model}; ${attemptNote}`,
            `change applies to next attempt (independent admission); ${selectionNote}`,
          ]
        : [
            `session model -> ${updated.configured.model}; ${selectionNote}`,
          ];
      // Leave a visible line for the unanswered-request warning within the
      // transient surface's three-line bound.
      if (!session.modelSelectionOutcomeUnknown) modelFeedback.push(`capabilities: ${updated.effective == null ? "unavailable" : capabilitySummary(updated.effective)}`);
      return transient(
        "info",
        withModelOutcomeNotice(session, modelFeedback.join("\n")),
      );
    } catch (error) {
      return failure(error);
    }
  }

  async #cancel(
    session: AppServerSession,
    argument: string,
  ): Promise<CommandOutcome> {
    if (argument.length > 0) return usage("/cancel (use /job-cancel <job-id> for a Job)");
    const attemptId = await session.cancelCurrentAttempt();
    return transient(
      "info",
      `cancellation requested for attempt ${attemptId}\nThis is acceptance; the runtime owns the terminal settlement.`,
    );
  }


}

function withModelOutcomeNotice(session: AppServerSession, text: string): string {
  return session.modelSelectionOutcomeUnknown
    ? `Earlier model selection outcome unknown; readback cannot settle it.\n${text}`
    : text;
}
