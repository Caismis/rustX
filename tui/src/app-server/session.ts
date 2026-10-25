import { materializeApplication } from '../../../protocol/app-server/application-observation.ts';
import { UncertainOutcomeError, AppServerRequestError } from './client.ts';
import { materializeEffective } from '../../../protocol/app-server/effective-observation.ts';
import { domainItems, domainValue } from '../../../protocol/app-server/domain-observation.ts';
import { checkpointSnapshot } from '../../../protocol/app-server/projection.ts';
import { ObservationChain, validateRootCheckpoint, validateRootEvent } from '../../../protocol/app-server/observation.ts';
import { uploadOperationId } from '../../../protocol/app-server/upload.ts';
/**
 * One attached Session: its authoritative projection and its typed operations.
 *
 * ```text
 * session/attach -> bounded Target receipt -> checkpoint -> subscribe receipt
 *                -> fold consecutive registration-scoped events
 * ```
 *
 * The locally Applied Cursor advances only after a successful complete fold.
 * Recovery installs an exact checkpoint, then registers after that cut. Cache
 * eviction never changes execution facts or the Applied Cursor.
 *
 * # Identity and fencing
 *
 * An {@link AttachmentTarget} names four distinct domains at once: the Session,
 * the Conversation, the runtime incarnation, and this attachment. They are not
 * aliases of each other, and neither is the projection cursor or a JSON-RPC
 * request id. Every notification repeats the full target, and this object folds
 * an event only when all four match — so an event addressed to a previous
 * incarnation of the same Session cannot reach a replacement projection.
 *
 * Responses are fenced the same way, by epoch: an authoritative read issued
 * against one attachment can still be in flight when the attachment is replaced
 * or released, and installing its result afterwards would overwrite current
 * truth with stale truth. The epoch check makes that impossible without any
 * reliance on timing.
 *
 * # Resync
 *
 * `session/resyncRequired` means the incremental projection can no longer be
 * trusted. The response is never to guess at the gap:
 *
 * ```text
 * incremental state no longer trusted
 *   -> session/checkpoint
 *   -> replace the projection wholesale
 *   -> session/subscribe after the new cursor
 * ```
 *
 * Nothing is replayed from what the UI thought happened.
 */

import {
  compareExact,
  type ConfigurationApplication,
  type AvailableConfiguration,
  type AttachmentTarget,
  type MethodName,
  type MethodParams,
  type ResultType,
  type ResultOf,
  type GoalControl,
  type RuntimeClientGoalView,
  type InteractionRef,
  type InteractionResponse,
  type ModelCatalogView,
  type Notification,
  type RuntimeClientJob,
  type RuntimeClientCursor,
  type RuntimeClientSnapshot,
  type RuntimeClientAgent,
  type SessionModelConfig,
  type SessionModelView,
  type SessionNodeId,
  type AgentId,
  type ToolExecutionId,
  type UserInputBlock,
} from "../protocol/app-server.ts";
import {
  deferredContent,
  presentationCheckpoint,
  mergeTranscriptPage,
  reduce,
  replaceFromSnapshot,
} from "../presentation/projection.ts";
import type { PresentationState } from "../presentation/state.ts";
import { AppServerClient, isResyncRequired, isUncertainOutcome } from "./client.ts";
import { requestEffect } from "../../../protocol/app-server/method-policy.ts";
import { sameTarget } from "../../../protocol/app-server/attachment-identity.ts";
import type { Request1 } from "../../../protocol/app-server/v57.ts";
import { SessionReaders, TRANSCRIPT_PROJECTION_PAGE_LIMIT } from "./session-readers.ts";

type StateListener = (state: PresentationState) => void;
type SnapshotListener = () => void;
type ClosedListener = () => void;

export class AppServerSession {
  readonly nodeId: SessionNodeId | undefined;
  readonly #client: AppServerClient;
  #target: AttachmentTarget;
  #state: PresentationState;
  #configuration?: ConfigurationApplication;
  get application(): ConfigurationApplication | undefined { return this.#configuration; }
  readonly #listeners = new Set<StateListener>();
  readonly #snapshotListeners = new Set<SnapshotListener>();
  readonly #closedListeners = new Set<ClosedListener>();
  #resyncCount = 0;
  #observationError?:string;
  get observationError(){return this.#observationError;}
  /** Advanced whenever authoritative ownership of this projection changes. */
  #epoch = 0;
  #released = false;
  #serverClosed = false;
  #modelMutationInFlight = false;
  /** Local evidence about unanswered requests, never authoritative Session state. */
  readonly #unknownModelSelections: Set<string>;
  /** Serializes repairs so two resyncs cannot interleave their installs. */
  #repair: Promise<void> | undefined;
  readonly #observation = new ObservationChain<Parameters<typeof reduce>[1]['event']>();
  #checkpoint?: import('../../../protocol/app-server/v57.ts').RuntimeClientCheckpoint;
  get observationPhase() { return this.#observation.phase; }
  /** Cancellation needs the live attachment, not an observed execution baseline. */
  get cancellationReady() { return !this.#released && !this.#serverClosed; }
  get controlReady() { return this.cancellationReady && this.#observation.phase === 'following'; }
  get checkpoint() { return this.#checkpoint; }

  #assertControlReady(): void {
    if (!this.controlReady) {
      throw new Error('Session observation is not following; request not sent.');
    }
  }

  /** New effects require the same accepted observation baseline as the editor. */
  #call<M extends Exclude<MethodName, 'delivery/cancel'>, T extends ResultType>(
    method: M, params: MethodParams<M>, expected: T,
  ): Promise<ResultOf<T>> {
    if ('target' in params && 'session_id' in params.target
      && requestEffect({ method, params } as Request1) === 'side_effecting'
      && method !== 'session/detach' && method !== 'session/switchNode') {
      try { this.#assertControlReady(); } catch (error) { return Promise.reject(error); }
    }
    return this.#client.call(method, params, expected);
  }
  get deferredContent() { return deferredContent(this.#state); }
  readonly #reads = new SessionReaders({
    client: () => this.#client,
    call: (method, params, expected) => this.#call(method, params, expected),
    target: () => this.#target,
    epoch: () => this.#epoch,
    released: () => this.#released,
    serverClosed: () => this.#serverClosed,
    checkpoint: () => this.#checkpoint,
    state: () => this.#state,
  });
  // Bounded native readers, fenced by this Session's epoch and target.
  get deferredJournalFields() { return this.#reads.deferredJournalFields; }
  get deferredAttemptIdentities() { return this.#reads.deferredAttemptIdentities; }
  get observationCollections() { return this.#reads.observationCollections; }
  get deferredTraceFields() { return this.#reads.deferredTraceFields; }
  get observationContent() { return this.#reads.observationContent; }
  get deliveryAccess() { return this.#reads.deliveryAccess; }
  inspectTrace(...args: Parameters<SessionReaders['inspectTrace']>) { return this.#reads.inspectTrace(...args); }
  agentTraceInspection(...args: Parameters<SessionReaders['agentTraceInspection']>) { return this.#reads.agentTraceInspection(...args); }
  agentInspectionPage(...args: Parameters<SessionReaders['agentInspectionPage']>) { return this.#reads.agentInspectionPage(...args); }
  readAgentTraceField(...args: Parameters<SessionReaders['readAgentTraceField']>) { return this.#reads.readAgentTraceField(...args); }
  readObservationContent(...args: Parameters<SessionReaders['readObservationContent']>) { return this.#reads.readObservationContent(...args); }
  readTraceField(...args: Parameters<SessionReaders['readTraceField']>) { return this.#reads.readTraceField(...args); }
  readAttemptIdentity(...args: Parameters<SessionReaders['readAttemptIdentity']>) { return this.#reads.readAttemptIdentity(...args); }
  readJournalField(...args: Parameters<SessionReaders['readJournalField']>) { return this.#reads.readJournalField(...args); }
  readChildJournalField(...args: Parameters<SessionReaders['readChildJournalField']>) { return this.#reads.readChildJournalField(...args); }
  transcriptPage(...args: Parameters<SessionReaders['transcriptPage']>) { return this.#reads.transcriptPage(...args); }
  deliveryPage(...args: Parameters<SessionReaders['deliveryPage']>) { return this.#reads.deliveryPage(...args); }
  deliveryCollection(...args: Parameters<SessionReaders['deliveryCollection']>) { return this.#reads.deliveryCollection(...args); }
  deliveryMetadata(...args: Parameters<SessionReaders['deliveryMetadata']>) { return this.#reads.deliveryMetadata(...args); }
  readDelivery(...args: Parameters<SessionReaders['readDelivery']>) { return this.#reads.readDelivery(...args); }
  locateDelivery(...args: Parameters<SessionReaders['locateDelivery']>) { return this.#reads.locateDelivery(...args); }
  observeAgent(...args: Parameters<SessionReaders['observeAgent']>) { return this.#reads.observeAgent(...args); }
  readOccupancyModel(...args: Parameters<SessionReaders['readOccupancyModel']>) { return this.#reads.readOccupancyModel(...args); }
  readContent(...args: Parameters<SessionReaders['readContent']>) { return this.#reads.readContent(...args); }
  agentTranscriptPage(...args: Parameters<SessionReaders['agentTranscriptPage']>) { return this.#reads.agentTranscriptPage(...args); }
  boundaries(...args: Parameters<SessionReaders['boundaries']>) { return this.#reads.boundaries(...args); }
  observationPage(...args: Parameters<SessionReaders['observationPage']>) { return this.#reads.observationPage(...args); }
  observationValue(...args: Parameters<SessionReaders['observationValue']>) { return this.#reads.observationValue(...args); }
  checkpointPage(...args: Parameters<SessionReaders['checkpointPage']>) { return this.#reads.checkpointPage(...args); }
  interactionValue(...args: Parameters<SessionReaders['interactionValue']>) { return this.#reads.interactionValue(...args); }
  capabilitiesPage(...args: Parameters<SessionReaders['capabilitiesPage']>) { return this.#reads.capabilitiesPage(...args); }
  projections(...args: Parameters<SessionReaders['projections']>) { return this.#reads.projections(...args); }
  projectionDetail(...args: Parameters<SessionReaders['projectionDetail']>) { return this.#reads.projectionDetail(...args); }

  private constructor(
    client: AppServerClient,
    target: AttachmentTarget,
    state: PresentationState,
    nodeId?: SessionNodeId,
    unknownModelSelections = new Set<string>(),
  ) {
    this.nodeId = nodeId;
    this.#client = client;
    this.#target = target;
    this.#state = state;
    this.#unknownModelSelections = unknownModelSelections;
    this.#observation.installCheckpoint(state.cursor);
    client.onClose(() => {
      this.#released = true;
      this.#epoch += 1;
    });
  }

  /**
   * Attaches to a durable Session and installs its authoritative projection.
   *
   * Attach loads or reuses the runtime and acquires this connection's external
   * control of it. It is not a process operation: attaching to a second Session
   * never replaces the first, and both stay live in the same App Server.
   */
  static async attach(
    client: AppServerClient,
    sessionId: string,
    nodeId?: SessionNodeId,
    prepared?: (session: AppServerSession) => void,
    previous?: AppServerSession,
    unknownModelSelections = new Set<string>(),
  ): Promise<AppServerSession> {
    let attached: Extract<import('../../../protocol/app-server/v57.ts').MethodResult,{type:'attached'}>;
    try {
      attached = await client.call('session/attach', {session_id:sessionId,node_id:nodeId??null}, 'attached');
    } catch (error) {
      if (client.closed || !(error instanceof UncertainOutcomeError || error instanceof AppServerRequestError && error.kind === 'outcome_unknown')) throw error;
      const status = await client.call('session/attachmentStatus',{session_id:sessionId},'attachment_status');
      if (!status.target || status.target.session_id !== sessionId) throw error;
      const node = nodeId == null
        ? (await client.call('session/read',{session_id:sessionId},'session')).session.active_node : nodeId;
      const found = await client.call('session/node',{session_id:sessionId,node_id:node},'node');
      if (found.node.conversation_id !== status.target.conversation_id) throw error;
      attached = {type:'attached',target:status.target,configuration_revision:status.configuration_revision,attachment_kind:'controller',observation_source:'live'};
    }
    const applied=previous ? previous.#observation.applied : undefined;
    const resume=previous && client.authorityId!==undefined && client.authorityId===previous.#client.authorityId && applied!==undefined && previous.#state.cursor===applied && previous.#checkpoint?.cursor===applied
      && previous.#target.session_id===attached.target.session_id && previous.#target.conversation_id===attached.target.conversation_id
      && previous.#target.runtime_incarnation===attached.target.runtime_incarnation ? previous : undefined;
    const checkpoint=(resume ? resume.#checkpoint : undefined) ?? (await client.call('session/checkpoint',{target:attached.target,trace_records:[]},'checkpoint')).checkpoint;
    validateRootCheckpoint(attached.target, checkpoint, resume ? resume.#observation.applied : undefined);
    const session = new AppServerSession(client, attached.target,
      (resume ? resume.#state : undefined) ?? replaceFromSnapshot(checkpointSnapshot(checkpoint), checkpoint.cursor), nodeId,unknownModelSelections);
    session.#checkpoint = checkpoint;
    prepared?.(session);
    const stopTemporaryRouting = prepared ? () => {} : client.onNotification(notification => session.applyNotification(notification));
    try {
      if (session.#released || session.#serverClosed) return session;
      const epoch = session.#epoch, chain = session.#observation;
      session.#observation.beginSubscription(checkpoint.cursor);
      try {
        const receipt = await client.call('session/subscribe', { target: attached.target, after_cursor: checkpoint.cursor }, 'subscribed');
        if (epoch !== session.#epoch || session.#released || session.#serverClosed || session.#observation !== chain) return session;
        if (!sameTarget(receipt.target, attached.target) || !session.#observation.acceptReceipt(receipt.registration_id, receipt.after_cursor)) {
          session.#observationError ??= `Subscription receipt could not join Applied ${session.#observation.applied}; receipt cursor ${receipt.after_cursor}, phase ${session.#observation.phase}`;
          session.#publish();
          if (session.#observation.phase !== 'unavailable') await session.resync();
        }
        session.#publish();
      } catch(error) {if (session.#released || session.#serverClosed) return session;if(!isResyncRequired(error))throw error;await session.resync();}
      if (session.#released || session.#serverClosed) return session;
      const configuration = await client.call('session/configuration', { session_id: sessionId }, 'session_configuration');
      const application=await materializeApplication(configuration.projection,async(reference,offset)=>(await client.call('session/configurationValueRead',{session_id:sessionId,reference,offset,max_bytes:65536},'session_application_value_chunk')).chunk,()=>!session.#released);
      if(application)session.#installConfiguration(application);
    } finally { stopTemporaryRouting(); }
    return session;
  }

  /** The full four-domain identity of this attachment. */
  get target(): AttachmentTarget {
    return this.#target;
  }

  get sessionId(): string {
    return this.#target.session_id;
  }

  /** The current presentation state. Always a projection, never authority. */
  get state(): PresentationState {
    return this.#state;
  }

  /** How many authoritative repairs this attachment has performed. */
  get resyncCount(): number {
    return this.#resyncCount;
  }

  /** Whether the server reported this attachment closed. */
  get serverClosed(): boolean {
    return this.#serverClosed;
  }

  /** Whether this client has released the attachment. */
  get released(): boolean {
    return this.#released;
  }

  /** Subscribes to presentation state changes. */
  onState(listener: StateListener): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  /**
   * Subscribes to authoritative projection replacement, including resync.
   *
   * The controlling TUI uses this signal to invalidate attachment-local
   * presentation leases. It carries no runtime semantics.
   */
  onSnapshot(listener: SnapshotListener): () => void {
    this.#snapshotListeners.add(listener);
    return () => this.#snapshotListeners.delete(listener);
  }

  /** Subscribes to the server reporting this attachment closed. */
  onClosed(listener: ClosedListener): () => void {
    this.#closedListeners.add(listener);
    return () => this.#closedListeners.delete(listener);
  }

  // -------------------------------------------------------------------------
  // Notification routing
  // -------------------------------------------------------------------------

  /**
   * Folds one routed notification, if it addresses exactly this attachment.
   *
   * Returns whether the notification belonged here. A notification for another
   * Session, or for a superseded incarnation or attachment of this Session, is
   * declined rather than applied: a stale target must never mutate the
   * projection that replaced it.
   */
  applyNotification(notification: Notification): boolean {
    if (notification.method === "configuration/changed") {
      const application = notification.params.application;
      if (this.#released || application.scope !== this.sessionId) return false;
      this.#observeConfiguration(application);
      return true;
    }
    if (notification.method === "session/summaryInvalidated" || notification.method === "session/ownershipRetired") {
      // Both explicit catalog_changed values are valid. The TUI holds
      // no live catalog/summary cache: `/resume` reads the catalog afresh every
      // time it opens, so there is nothing here to repair. The notification is
      // accepted and declined — never folded into the conversation projection,
      // and never treated as invalid protocol input.
      return false;
    }
    if (notification.method === "configuration/sourceRepairChanged") {
      // Source-scoped (Issue #464): it addresses a configuration source, never
      // a Session, and is never folded into conversation presentation.
      return false;
    }
    if (this.#released || this.#serverClosed || !sameTarget(notification.params.target, this.#target)) {
      return false;
    }
    switch (notification.method) {
      case "session/event": {
        const result = this.#observation.deliver({ registrationId: notification.params.registration_id, cursor: notification.params.cursor, event: notification.params.event },
          (event, cursor) => this.#applyEvent(cursor, event));
        if (result === 'recovery') {
          this.#observationError ??= `Observation continuity lost at cursor ${notification.params.cursor}; Applied ${this.#observation.applied}`;
          this.#publish();
          this.#enqueueRepair();
        }
        return true;
      }
      case "session/observationUnavailable": {
        const result = this.#observation.deliver({ registrationId: notification.params.registration_id, unavailable: true }, (event, cursor) => this.#applyEvent(cursor, event));
        if (result === 'unavailable') {
          this.#observationError = `Observation source unavailable: ${notification.params.message}`.slice(0, 512);
          this.#publish();
        }
        return true;
      }
      case "session/resyncRequired": {
        const result = this.#observation.deliver({ registrationId: notification.params.registration_id, resync: true }, (event, cursor) => this.#applyEvent(cursor, event));
        if (result === 'recovery') {
          this.#observationError = `Observation recovery required: ${notification.params.reason}`.slice(0,512);
          this.#publish();
          this.#enqueueRepair();
        }
        return true;
      }
      case "auxiliary/segment":
        return false; // Optional output has no TUI product and never advances Projection.
      case "session/closed":
        this.#serverClosed = true;
        this.#observation.retire();
        this.#epoch += 1;
        // Residency ended. That is a statement about this attachment's
        // observability, not a runtime outcome: nothing here fabricates a
        // settled attempt, an answered interaction, or a completed tool.
        for (const listener of [...this.#closedListeners]) {
          listener();
        }
        return true;
      default: {
        const exhaustive: never = notification;
        void exhaustive;
        return false;
      }
    }
  }

  // -------------------------------------------------------------------------
  // Turn and interaction
  // -------------------------------------------------------------------------

  /** Submits one inbound message. Acceptance, never completion. */
  async submitInbound(
    content: UserInputBlock[],
  ): Promise<{ messageId: string; sequence: string }> {
    const accepted = await this.#call(
      "turn/start",
      { target: this.#target, content },
      "inbound_accepted",
    );
    return {
      messageId: accepted.message_id,
      sequence: accepted.inbound_sequence,
    };
  }

  /** Steering and queue admission stay native; neither retains a client queue. */
  async steer(content: UserInputBlock[]): Promise<void> {
    await this.#call("turn/steer", { target: this.#target, content }, "inbound_accepted");
  }

  async editPending(expected: import("../protocol/app-server.ts").MethodParams<"inbound/edit">["expected"], text: string): Promise<void> {
    const result = await this.#call("inbound/edit", { target: this.#target, expected, text }, "inbound_mutation");
    if (result.outcome.status !== "applied") throw new Error(`Pending edit: ${result.outcome.status}. Not retried.`);
  }

  async removePending(expected: import("../protocol/app-server.ts").MethodParams<"inbound/remove">["expected"]): Promise<void> {
    const result = await this.#call("inbound/remove", { target: this.#target, expected }, "inbound_mutation");
    if (result.outcome.status !== "applied") throw new Error(`Pending removal: ${result.outcome.status}. Not retried.`);
  }

  get uploadPolicy() { return this.#client.capabilities?.upload_policy; }
  async upload(name: string, bytes: Uint8Array): Promise<UserInputBlock[]> {
    this.#assertControlReady();
    const operation = uploadOperationId();
    const epoch = this.#epoch, target = this.#target;
    const current = () => !this.#released && !this.#serverClosed && epoch === this.#epoch && sameTarget(target, this.#target);
    const check = () => { if (!current()) throw new Error(`Upload authority changed; reconcile operation ${operation}`); };
    check();
    const policy = this.uploadPolicy;
    if (!policy) throw new Error('Upload policy unavailable');
    const { transfer } = await this.#call("session/uploadPrepare", {
      target, operation_id: operation, files: [{ name, size: bytes.byteLength }],
    }, "upload_prepared");
    check();
    try { await this.#client.uploadCarrier(transfer, [new Blob([Uint8Array.from(bytes)])], policy, this.#client.uploadEndpoint); } catch { /* Repair the exact operation; never resend bytes. */ }
    check();
    const { outcome } = await this.#call("session/uploadStatus", { target, operation_id: operation }, "upload_status");
    check();
    if (outcome.state !== 'ready') throw new Error(`Upload ${operation}: ${outcome.state}. No bytes replayed.`);
    return outcome.files.map(({ receipt }) => ({ type: "upload", ...receipt }));
  }

  /** Requests cancellation of the current attempt. Acceptance, not settlement. */
  async cancelCurrentAttempt(): Promise<string> {
    if (!this.cancellationReady) throw new Error('Session attachment is unavailable.');
    // The native coordinator selects its current Attempt under its admission
    // lock. A retained observation cannot establish that owner's absence.
    const accepted = await this.#client.call(
      "turn/cancel",
      { target: this.#target },
      "cancellation_accepted",
    );
    return accepted.attempt_id;
  }

  /** Answers one runtime-owned interaction. */
  async respondInteraction(
    interaction: InteractionRef,
    response: InteractionResponse,
  ): Promise<void> {
    await this.#call(
      "interaction/respond",
      { target: this.#target, interaction, response },
      "interaction_settled",
    );
  }

  /** Cancels one pending interaction through its originating coordinator. */
  async cancelInteraction(interaction: InteractionRef): Promise<void> {
    await this.#call(
      "interaction/cancel",
      { target: this.#target, interaction },
      "interaction_settled",
    );
  }

  // -------------------------------------------------------------------------
  // Authoritative reads
  // -------------------------------------------------------------------------

  /** Loads the next older page without changing the live event cursor. */
  async loadOlderTranscript(
    limit = TRANSCRIPT_PROJECTION_PAGE_LIMIT,
  ): Promise<boolean> {
    const beforeCursor = this.#state.transcriptNextCursor;
    if (beforeCursor === undefined || beforeCursor === null) {
      return false;
    }
    const epoch = this.#epoch;
    const page = await this.transcriptPage(beforeCursor, limit);
    if (epoch !== this.#epoch || this.#state.transcriptNextCursor !== beforeCursor) {
      // A page belongs to the exact boundary requested. A live refresh can
      // replace that window without replacing attachment ownership. Neither
      // that stale page nor its next cursor may be spliced into the new window.
      return false;
    }
    this.#state = mergeTranscriptPage(this.#state, page);
    this.#publish();
    return (page.entries ?? []).length > 0;
  }

  get catalogDeferredNotice(): string | undefined {
    const page = this.#modelCatalogPage;
    if (!page) return undefined;
    const deferred = page.items.filter(item => item.value == null || item.deferred
      || (item.value as {profilesDeferred?: boolean}).profilesDeferred === true);
    if (!deferred.length) return undefined;
    return `Binding ${this.#state.modelStatus?.binding_revision ?? 'unavailable'}: deferred model/profile choices at native positions ${deferred.map(item => item.offset).join(', ')}. Read /observation model_catalog.models; /model-position selects native defaults at the displayed binding revision.`;
  }
  #modelCatalogPage?: import('../../../protocol/app-server/v57.ts').CheckpointPage;
  get nextModelOffset() { return this.#modelCatalogPage?.next_offset == null ? undefined : Number(this.#modelCatalogPage.next_offset); }
  /** Runs one manual idle compaction to its durable terminal result. */
  async compactContext(): Promise<NonNullable<ResultOf<'context_compacted'>['compaction']>> {
    if (this.#released || this.#serverClosed) throw new Error('Session attachment is unavailable.');
    const { compaction } = await this.#client.call(
      "context/compact",
      { target: this.#target, request_id: globalThis.crypto.randomUUID() },
      "context_compacted",
    );
    if (!compaction) throw new Error('Native compaction response omitted its commit receipt.');
    return compaction;
  }

  /** Reads or mutates the root Goal through its native owner. */
  async goal(control: GoalControl): Promise<RuntimeClientGoalView> {
    if (control.action !== 'show') await this.#call('goal/control',{target:this.#target,control},'goal_mutation');
    const {page}=await this.#call('goal/control',{target:this.#target,control:{action:'show'}},'domain_page');
    return domainValue<RuntimeClientGoalView>(page,this.#reads.domainReader());
  }

  #configurationCut?:import('../../../protocol/app-server/v57.ts').BoundedSessionApplication;
  #configurationRead?:Promise<void>;
  #configurationObservationError?:string;
  get configurationObservationError(){return this.#configurationObservationError;}
  get deferredConfiguration(){return this.#configurationRead?this.#configurationCut:undefined;}
  #observeConfiguration(cut:import('../../../protocol/app-server/v57.ts').BoundedSessionApplication) {
    if(this.#configurationCut && BigInt(cut.version)<=BigInt(this.#configurationCut.version))return;
    this.#configurationCut=cut;
    if(cut.inline){this.#configurationObservationError=undefined;this.#installConfiguration(cut.inline);return;}
    this.#configuration=undefined;this.#configurationObservationError=undefined;this.#publish();
    if(this.#configurationRead)return;
    const epoch=this.#epoch;
    const work=materializeApplication(cut,async(reference,offset)=>(await this.#call('session/configurationValueRead',{session_id:this.sessionId,reference,offset,max_bytes:65536},'session_application_value_chunk')).chunk,()=>!this.#released&&epoch===this.#epoch&&this.#configurationCut===cut)
      .then(application=>{if(application){this.#configurationObservationError=undefined;this.#installConfiguration(application);}}).catch(error=>{if(this.#configurationCut===cut){this.#configurationObservationError=String(error).slice(0,512);this.#publish();}}).finally(()=>{
        if(this.#configurationRead!==work)return;this.#configurationRead=undefined;
        const latest=this.#configurationCut;if(latest!==cut&&latest){this.#configurationCut=undefined;this.#observeConfiguration(latest);}
      });
    this.#configurationRead=work;this.#publish();
  }
  #installConfiguration(application: ConfigurationApplication): void {
    if (this.#configuration && compareExact(application.version, this.#configuration.version) < 0) return;
    this.#configuration = application;
    this.#publish();
  }

  async readConfiguration() {
    const result = await this.#call("session/configuration", { session_id: this.sessionId }, "session_configuration");
    const epoch=this.#epoch;
    const application=await materializeApplication(result.projection,async(reference,offset)=>(await this.#call('session/configurationValueRead',{session_id:this.sessionId,reference,offset,max_bytes:65536},'session_application_value_chunk')).chunk,()=>!this.#released&&epoch===this.#epoch);
    if(application)this.#installConfiguration(application);
    return application;
  }

  async adoptConfiguration(candidate: AvailableConfiguration): Promise<ConfigurationApplication> {
    await this.#call("session/adoptConfiguration", {
      session_id: this.sessionId, candidate: candidate.identity, expected_binding: candidate.expected_binding,
    }, "configuration_receipt");
    // Acknowledgement is not a new observation. The caller rereads native authority.
    const application = await this.readConfiguration();
    if (!application) throw new Error("Configuration source unavailable.");
    return application;
  }

  /** The safe public catalog. This is why the client never reads rustx.toml. */
  async modelCatalog(offset?: number): Promise<ModelCatalogView> {
    const epoch = this.#epoch;
    const binding = this.#state.modelStatus?.binding_revision;
    const page=offset===undefined ? (await this.#call('session/models',{target:this.#target},'domain_page')).page
      : this.#modelCatalogPage ? await this.#reads.domainReader().page(this.#modelCatalogPage,offset) : undefined;
    if(!page) throw new Error('Load the catalog before its continuation.');
    if (this.#released || this.#serverClosed || epoch !== this.#epoch
      || binding !== this.#state.modelStatus?.binding_revision) {
      throw new Error('Catalog read belongs to a retired model binding or attachment.');
    }
    this.#modelCatalogPage=page;
    // Browsing reads finite native headers only. Giant identities and incomplete
    // profile heads remain explicit source descriptors, never automatic reads.
    const models: NonNullable<ModelCatalogView['models']> = [];
    for (const item of page.items) {
      const value = item.value as (NonNullable<ModelCatalogView['models']>[number] & {profilesDeferred?: boolean}) | null;
      if (!value || item.deferred || value.profilesDeferred === true
        || typeof value.model !== 'string' || new TextEncoder().encode(value.model).length > 256
        || value.defaultProfile != null && new TextEncoder().encode(value.defaultProfile).length > 256
        || (value.profiles ?? []).some(profile => new TextEncoder().encode(profile.id).length > 256)) continue;
      models.push(value);
    }
    return {models};
  }
  async modelGet(): Promise<SessionModelView> {
    if (this.#repair) await this.#repair;
    const epoch=this.#epoch;
    const {page}=await this.#call('session/model',{target:this.#target},'domain_page');
    const model=await domainValue<SessionModelView>(page,this.#reads.domainReader());
    if(this.#released||this.#serverClosed||epoch!==this.#epoch)throw new Error('Model read belongs to a retired attachment; reread native authority.');
    return model;
  }

  /** A snapshot cannot establish the terminal outcome of an unanswered request. */
  get modelSelectionOutcomeUnknown(): boolean {
    return this.#unknownModelSelections.has(this.sessionId);
  }

  /**
   * Replaces the authoritative session model configuration.
   *
   * A whole-state replacement, never a patch: callers send back the complete
   * configuration they read. The update affects future admissions only; an
   * already-admitted attempt keeps the model it froze.
   *
   * A new deliberate request is allowed after a lost response. Native revision
   * checks fence older prepared work when the new selection changes authority;
   * a NoOp acknowledgement cannot settle an earlier unanswered request.
   */
  /** Selects native catalog defaults by finite ordinals at the displayed source revision. */
  async modelSelect(choice:import('../../../protocol/app-server/v57.ts').CatalogModelSelection) {
    if(!Number.isSafeInteger(choice.model_position)||choice.model_position<0 ||choice.profile_position!=null&&(!Number.isSafeInteger(choice.profile_position)||choice.profile_position<0))throw new Error('Invalid native model selection ordinal.');
    if(this.#released||this.#serverClosed)throw new Error('Session attachment is unavailable.');
    if(this.#modelMutationInFlight)throw new Error('A model selection request is still in flight; wait for its response or connection recovery.');
    this.#assertControlReady();
    const epoch=this.#epoch;
    this.#modelMutationInFlight=true;
    try {
      const receipt=await this.#call('session/selectModel',{target:this.#target,choice},'model_set');
      if(this.#released||this.#serverClosed||epoch!==this.#epoch)throw new Error('Model acknowledgement belongs to a retired observation; reread native authority. Not replayed.');
      // Confirmation never requires materializing a growing model identity or its defaults.
      return receipt;
    }catch(error){
      if(isUncertainOutcome(error))this.#unknownModelSelections.add(this.sessionId);
      if(!this.#released&&!this.#serverClosed)await this.resync().catch(()=>{});
      throw error;
    }finally{this.#modelMutationInFlight=false;}
  }

  async modelSet(config: SessionModelConfig): Promise<SessionModelView> {
    if (this.#released || this.#serverClosed) throw new Error("Session attachment is unavailable.");
    if (this.#modelMutationInFlight) throw new Error("A model selection request is still in flight; wait for its response or connection recovery.");
    this.#assertControlReady();
    const epoch = this.#epoch;
    this.#modelMutationInFlight = true;
    try {
      await this.#call(
        "session/setModel",
        { target: this.#target, config },
        "model_set",
      );
      if (this.#released || this.#serverClosed || epoch !== this.#epoch) {
        throw new Error("Model acknowledgement belongs to a retired observation; reread native authority. Not replayed.");
      }
      // A response confirms this operation, but is not an ordered observation.
      // Only native events and snapshots update the Session or Attempt view.
      return await this.modelGet();
    } catch (error) {
      if (isUncertainOutcome(error)) this.#unknownModelSelections.add(this.sessionId);
      // Readback repairs the projection, not the outcome of an unanswered
      // request. Native work can survive its waiter and commit after a snapshot.
      if (!this.#released && !this.#serverClosed) await this.resync().catch(() => {});
      throw error;
    } finally {
      this.#modelMutationInFlight = false;
    }
  }

  /** One native read of the published immutable configuration. */
  async configuration(): Promise<import("../protocol/app-server.ts").EffectiveConfiguration> {
    const result = await this.#call("session/effectiveConfiguration", { target: this.#target }, "effective_configuration");
    const epoch=this.#epoch;
    return materializeEffective(result.projection,async(reference,offset)=>(await this.#call('session/effectiveConfigurationValueRead',{target:this.#target,reference,offset,max_bytes:65536},'configuration_value_chunk')).chunk,()=>!this.#released&&epoch===this.#epoch);
  }

  // -------------------------------------------------------------------------
  // Finite Jobs and durable Agents
  // -------------------------------------------------------------------------

  /** Commit native cancellation intent; lifecycle and physical settlement remain observed facts. */
  async cancelJob(jobId: ToolExecutionId) {
    const receipt = await this.#call("job/cancel", { target: this.#target, job_id: jobId }, "job_cancellation_accepted");
    if (receipt.job_id !== jobId) throw new Error('Job cancellation receipt named another native execution.');
    return receipt;
  }

  async jobStatus(jobId: ToolExecutionId): Promise<RuntimeClientJob> {
    const {page}=await this.#call('job/status',{target:this.#target,job_id:jobId},'domain_page');
    return domainValue<RuntimeClientJob>(page,this.#reads.domainReader());
  }

  async listJobs() {
    const {page}=await this.#call('job/list',{target:this.#target},'domain_page');
    const jobs=await domainItems<RuntimeClientJob>(page,this.#reads.domainReader());
    return {jobs,matched:Number(page.total),returned:jobs.length,limit:32,truncated:page.next_offset!=null};
  }

  async waitJob(jobId: ToolExecutionId): Promise<RuntimeClientJob> {
    await this.#call("job/wait", { target: this.#target, job_id: jobId }, "job_outcome");
    return this.jobStatus(jobId);
  }

  /** One native bounded Agent viewport; continuations retain its exact cut. */
  async agentPage(offset = 0, previous?: import('../../../protocol/app-server/v57.ts').CheckpointPage) {
    const epoch=this.#epoch;
    const page=previous
      ? await this.#reads.domainReader().page(previous,offset)
      : (await this.#call('agent/list',{target:this.#target},'domain_page')).page;
    if(this.#released||epoch!==this.#epoch)throw new Error('Agent viewport was replaced.');
    const agents=await domainItems<RuntimeClientAgent>(page,this.#reads.domainReader());
    if(this.#released||epoch!==this.#epoch)throw new Error('Agent viewport was replaced.');
    return {page,agents};
  }

  async agentStatus(agentId: AgentId): Promise<RuntimeClientAgent> {
    const {page}=await this.#call('agent/status',{target:this.#target,agent_id:agentId},'domain_page');
    return domainValue<RuntimeClientAgent>(page,this.#reads.domainReader());
  }

  async sendMessage(agentId: AgentId, message: string, attachments: import("../../../protocol/app-server/v57.js").UploadReceipt[] = []) {
    return this.#call("agent/sendMessage", { target: this.#target, agent_id: agentId, message, attachments }, "agent_message");
  }

  async waitAgent(agentId: AgentId) {
    return this.#call("agent/wait", { target: this.#target, agent_id: agentId }, "agent_wait");
  }

  async interruptAgent(agentId: AgentId) {
    const receipt = await this.#call("agent/interrupt", { target: this.#target, agent_id: agentId }, "agent_interruption_accepted");
    if (receipt.agent_id !== agentId) throw new Error('Agent interruption receipt named another native owner.');
    return receipt;
  }

  // -------------------------------------------------------------------------
  // Repair and release
  // -------------------------------------------------------------------------

  /**
   * Takes a fresh authoritative snapshot and replaces the projection.
   *
   * The snapshot and the re-subscription are two requests, so the cursor is
   * what joins them: the new registration starts exactly where the snapshot
   * ends, and any observation in between is either already described by the
   * snapshot or arrives after that cursor.
   */
  resync(): Promise<void> {
    if (this.#repair) return this.#repair;
    if (this.#released || this.#serverClosed) return Promise.resolve();
    this.#observation.requireRecovery();
    let epoch = ++this.#epoch;
    const target = this.#target;
    const work = (async () => {
      for (let attempt = 0; attempt < 3; attempt++) {
        this.#observation.beginCheckpointLoading();
        const { checkpoint } = await this.#call("session/checkpoint", { target, trace_records: [] }, "checkpoint");
        if (epoch !== this.#epoch || this.#released || this.#serverClosed) return;
        validateRootCheckpoint(target, checkpoint, this.#observation.applied);
        this.#resyncCount += 1;
        this.#checkpoint = checkpoint;
        this.#install(checkpointSnapshot(checkpoint), checkpoint.cursor);
        this.#observation.installCheckpoint(checkpoint.cursor);
        epoch = this.#epoch;
        try {
          const chain = this.#observation;
          chain.beginSubscription(checkpoint.cursor);
          const receipt = await this.#call("session/subscribe", { target, after_cursor: checkpoint.cursor }, "subscribed");
          if (epoch !== this.#epoch || this.#released || this.#serverClosed || this.#observation !== chain) return;
          if (!sameTarget(receipt.target, target) || !chain.acceptReceipt(receipt.registration_id, receipt.after_cursor)) throw new Error(chain.phase === 'unavailable' ? "Observation source unavailable for the accepted registration." : "Subscription recovery required.");
          this.#publish();
          return;
        } catch (error) {
          if (epoch !== this.#epoch || this.#released || this.#serverClosed) return;
          if (!isResyncRequired(error) || attempt === 2) throw error;
        }
      }
    })().catch(error => {
      if (epoch === this.#epoch && !this.#released && !this.#serverClosed) {
        if (error instanceof AppServerRequestError && error.kind === 'observation_unavailable' || this.#observation.phase === 'unavailable') this.#observation.markUnavailable();
        else this.#observation.requireRecovery();
        this.#observationError = `Observation source unavailable: ${String(error)}`.slice(0, 512);
        this.#publish();
      }
      throw error;
    }).finally(() => {
      if (this.#repair === work) this.#repair = undefined;
    });
    this.#repair = work;
    this.#publish();
    return work;
  }

  /**
   * Releases this attachment.
   *
   * Detach removes exactly this connection's control relationship. It does not
   * cancel a turn, settle an interaction, unload the runtime, or shut anything
   * down, and the Session keeps running in the App Server afterwards.
   */
  async detach(): Promise<void> {
    if (this.#released) {
      return;
    }
    this.#released = true;
    this.#epoch += 1;
    this.#observation.retire();
    await this.#call(
      "session/detach",
      { target: this.#target },
      "detached",
    );
  }

  /** Switch the durable branch; native retirement remains manager-owned. */
  async switchNode(nodeId: string): Promise<void> {
    this.#released = true;
    this.#epoch += 1;
    this.#observation.retire();
    await this.#call("session/switchNode", { target: this.#target, node_id: nodeId }, "session");
  }

  // -------------------------------------------------------------------------
  // Internals
  // -------------------------------------------------------------------------

  #enqueueRepair(): void {
    void this.resync().catch(() => {});
  }

  #applyEvent(
    cursor: RuntimeClientCursor,
    event: Parameters<typeof reduce>[1]["event"],
  ): void {
    validateRootEvent(this.#target, event, cursor);
    // An event at or before the installed cursor is already described by the
    // snapshot; folding it again would double-apply a fact. Cursors are exact
    // u64 decimal text, so this is a numeric comparison and never a string one.
    if (compareExact(cursor, this.#state.cursor) <= 0) {
      return;
    }
    if (BigInt(cursor) !== BigInt(this.#state.cursor) + 1n) throw new Error("Non-contiguous observation.");
    try { this.#state = reduce(this.#state, { cursor, event }); }
    catch(error) {
      this.#observationError=`${event.type} at ${cursor}: ${String(error)}`.slice(0,512);
      this.#publish();
      throw error;
    }
    this.#observationError=undefined;
    this.#checkpoint = presentationCheckpoint(this.#state) ?? this.#checkpoint;
    this.#publish();

  }

  #install(snapshot: RuntimeClientSnapshot, cursor: RuntimeClientCursor): void {
    this.#epoch += 1;
    this.#state = replaceFromSnapshot(snapshot, cursor);
    this.#observationError = undefined;
    for (const listener of [...this.#snapshotListeners]) {
      listener();
    }
    this.#publish();
  }

  #publish(): void {
    for (const listener of [...this.#listeners]) {
      listener(this.#state);
    }
  }
}
