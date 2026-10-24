// Generated from Rust App Server DTOs. Run pnpm generate in protocol/app-server. Do not edit.

/**
 * Complete public wire surface used by schema and client generation.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ProtocolMessage".
 */
export type ProtocolMessage = Request | Response | Notification;
/**
 * A single public method space, with no nested Runtime Client envelope.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "Request".
 */
export type Request = {
  jsonrpc: JsonRpcVersion;
  id: AppServerRequestId;
} & Request1;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "JsonRpcVersion".
 */
export type JsonRpcVersion = '2.0';
/**
 * Integer request IDs use the JavaScript safe-integer range at admission.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AppServerRequestId".
 */
export type AppServerRequestId = string | number;
export type Request1 =
  | {
      method: 'auxiliary/classify';
      params: {
        target: AttachmentTarget;
        submission: ClassificationSubmission;
      };
    }
  | {
      method: 'auxiliary/btw';
      params: {
        target: AttachmentTarget;
        submission: BtwSubmission;
      };
    }
  | {
      method: 'auxiliary/status';
      params: {
        target: AttachmentTarget;
        call_id: AuxiliaryCallId;
      };
    }
  | {
      method: 'auxiliary/result';
      params: {
        target: AttachmentTarget;
        call_id: AuxiliaryCallId;
      };
    }
  | {
      method: 'auxiliary/cancel';
      params: {
        target: AttachmentTarget;
        call_id: AuxiliaryCallId;
      };
    }
  | {
      method: 'auxiliary/discard';
      params: {
        target: AttachmentTarget;
        call_id: AuxiliaryCallId;
      };
    }
  | {
      method: 'auxiliary/submissionLookup';
      params: {
        target: AttachmentTarget;
        correlation: CorrelationId;
        scope?: AuxiliaryAttribution | null;
      };
    }
  | {
      method: 'agent/artifactRead';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
        artifact_id: ArtifactId;
      };
    }
  | {
      method: 'agent/deliveryRead';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
        locator: CanonicalContentLocator;
        delivery_index: number;
      };
    }
  | {
      method: 'agent/deliveryLocate';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
        locator: CanonicalContentLocator;
        delivery_index: number;
      };
    }
  | {
      method: 'artifact/read';
      params: {
        target: AttachmentTarget;
        artifact_id: ArtifactId;
      };
    }
  | {
      method: 'delivery/read';
      params: {
        target: AttachmentTarget;
        locator: CanonicalContentLocator;
        delivery_index: number;
      };
    }
  | {
      method: 'delivery/locate';
      params: {
        target: AttachmentTarget;
        locator: CanonicalContentLocator;
        delivery_index: number;
      };
    }
  | {
      method: 'delivery/cancel';
      params: {
        request_id: AppServerRequestId;
      };
    }
  | {
      method: 'session/uploadPrepare';
      params: {
        target: AttachmentTarget;
        operation_id: string;
        /**
         * One file per wire operation keeps committed receipts recoverable within a record.
         *
         * @minItems 1
         * @maxItems 1
         */
        files: [UploadMetadata];
      };
    }
  | {
      method: 'session/uploadStatus';
      params: {
        target: AttachmentTarget;
        operation_id: string;
      };
    }
  | {
      method: 'session/switchNode';
      params: {
        target: AttachmentTarget;
        node_id: SessionNodeId;
      };
    }
  | {
      method: 'session/projections';
      params: {
        target: AttachmentTarget;
        after: ProjectionActivationId;
        limit: number;
      };
    }
  | {
      method: 'session/projectionDetail';
      params: {
        target: AttachmentTarget;
        checkpoint_id: CheckpointId;
        offset: number;
        limit: number;
      };
    }
  | {
      method: 'session/trace';
      params: {
        target: AttachmentTarget;
        /**
         * @maxItems 8
         */
        records?:
          | []
          | [TraceCursor]
          | [TraceCursor, TraceCursor]
          | [TraceCursor, TraceCursor, TraceCursor]
          | [TraceCursor, TraceCursor, TraceCursor, TraceCursor]
          | [TraceCursor, TraceCursor, TraceCursor, TraceCursor, TraceCursor]
          | [TraceCursor, TraceCursor, TraceCursor, TraceCursor, TraceCursor, TraceCursor]
          | [
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor
            ]
          | [
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor
            ];
        before?: TraceCursor | null;
        limit: number;
      };
    }
  | {
      method: 'session/traceDetail';
      params: {
        target: AttachmentTarget;
        record_id: string;
      };
    }
  | {
      method: 'session/traceLocateTool';
      params: {
        target: AttachmentTarget;
        locator: TraceToolLocator;
      };
    }
  | {
      method: 'session/transcript';
      params: {
        target: AttachmentTarget;
        at: ConversationWindowAt;
        limit: number;
      };
    }
  | {
      method: 'session/turns';
      params: {
        target: AttachmentTarget;
        /**
         * Absent selects the newest native outline page.
         */
        offset?: number | null;
        limit: number;
      };
    }
  | {
      method: 'session/model';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'session/models';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'session/setModel';
      params: {
        target: AttachmentTarget;
        config: SessionModelConfig;
      };
    }
  | {
      method: 'session/selectModel';
      params: {
        target: AttachmentTarget;
        choice: CatalogModelSelection;
      };
    }
  | {
      method: 'resources/read';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'context/compact';
      params: {
        target: AttachmentTarget;
        request_id: ManualCompactionRequestId;
      };
    }
  | {
      method: 'workflow/scheduled/getRecovery';
      params: {
        session_id: SessionId;
        node_id: SessionNodeId;
        start_id: ScheduledWorkflowStartId;
        after_effect_ordinal: string;
        limit: number;
      };
    }
  | {
      method: 'workflow/scheduled/cancel';
      params: {
        session_id: SessionId;
        node_id: SessionNodeId;
        start_id: ScheduledWorkflowStartId;
      };
    }
  | {
      method: 'workflow/scheduled/reconcile';
      params: {
        session_id: SessionId;
        node_id: SessionNodeId;
        start_id: ScheduledWorkflowStartId;
        after_effect_ordinal: string;
        limit: number;
      };
    }
  | {
      method: 'schedule/list';
      params: {
        session_id: SessionId;
        node_id: SessionNodeId;
        after_ordinal: string;
        limit: number;
      };
    }
  | {
      method: 'schedule/get';
      params: {
        session_id: SessionId;
        node_id: SessionNodeId;
        id: ScheduleId;
      };
    }
  | {
      method: 'schedule/command';
      params: {
        session_id: SessionId;
        node_id: SessionNodeId;
        command: ScheduleCommand;
      };
    }
  | {
      method: 'schedule/recoverStorage';
      params: {
        session_id: SessionId;
        node_id: SessionNodeId;
      };
    }
  | {
      method: 'goal/control';
      params: {
        target: AttachmentTarget;
        control: GoalControl;
      };
    }
  | {
      method: 'job/status';
      params: {
        target: AttachmentTarget;
        job_id: ToolExecutionId;
      };
    }
  | {
      method: 'job/list';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'job/wait';
      params: {
        target: AttachmentTarget;
        job_id: ToolExecutionId;
      };
    }
  | {
      method: 'job/cancel';
      params: {
        target: AttachmentTarget;
        job_id: ToolExecutionId;
      };
    }
  | {
      method: 'agent/status';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
      };
    }
  | {
      method: 'agent/list';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'agent/observeCancel';
      params: {
        request_id: AppServerRequestId;
      };
    }
  | {
      method: 'agent/observe';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
        after?: AgentObservationCursor | null;
      };
    }
  | {
      method: 'agent/statistics';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
      };
    }
  | {
      method: 'agent/sendMessage';
      params: {
        attachments: UploadReceipt[];
        target: AttachmentTarget;
        agent_id: AgentId;
        message: string;
      };
    }
  | {
      method: 'agent/wait';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
      };
    }
  | {
      method: 'agent/interrupt';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
      };
    }
  | {
      method: 'agent/trace';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
        before?: TraceCursor | null;
        limit: number;
        /**
         * @maxItems 8
         */
        records?:
          | []
          | [TraceCursor]
          | [TraceCursor, TraceCursor]
          | [TraceCursor, TraceCursor, TraceCursor]
          | [TraceCursor, TraceCursor, TraceCursor, TraceCursor]
          | [TraceCursor, TraceCursor, TraceCursor, TraceCursor, TraceCursor]
          | [TraceCursor, TraceCursor, TraceCursor, TraceCursor, TraceCursor, TraceCursor]
          | [
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor
            ]
          | [
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor
            ];
      };
    }
  | {
      method: 'agent/traceDetail';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
        record_id: string;
      };
    }
  | {
      method: 'agent/traceLocateTool';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
        locator: TraceToolLocator;
      };
    }
  | {
      method: 'agent/transcript';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
        at: ConversationWindowAt;
        limit: number;
      };
    }
  | {
      method: 'agent/turns';
      params: {
        target: AttachmentTarget;
        agent_id: AgentId;
        offset?: number | null;
        limit: number;
      };
    }
  | {
      method: 'initialize';
      params: InitializeParams;
    }
  | {
      method: 'server/info';
      params: {};
    }
  | {
      method: 'server/diagnostics';
      params: {};
    }
  | {
      method: 'session/exportPrepare';
      params: {
        session_id: SessionId;
      };
    }
  | {
      method: 'session/list';
      params: {
        query?: string | null;
        offset: number;
        limit: number;
      };
    }
  | {
      method: 'session/create';
      params: {
        settings: SessionPersistentState;
      };
    }
  | {
      method: 'session/read';
      params: {
        session_id: SessionId;
      };
    }
  | {
      method: 'session/history';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
        at: ConversationWindowAt;
        limit: number;
      };
    }
  | {
      method: 'session/traceHistory';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
        before?: TraceCursor | null;
        limit: number;
      };
    }
  | {
      method: 'session/traceHistoryDetail';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
        record_id: string;
      };
    }
  | {
      method: 'session/tracePage';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
        cut: CheckpointReadCut;
        domain: string;
        offset: number;
        limit: number;
      };
    }
  | {
      method: 'session/traceValueRead';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
        cut: CheckpointReadCut;
        domain: string;
        item_offset: number;
        /**
         * @maxItems 32
         */
        path: string[];
        offset: number;
        max_bytes: number;
      };
    }
  | {
      method: 'session/traceContentRead';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
        cut: CheckpointReadCut;
        content_ref: ContentRef;
        offset: number;
        max_bytes: number;
      };
    }
  | {
      method: 'session/historyContentRead';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
        cut: ObservationReadCut;
        content_ref: ContentRef;
        offset: number;
        max_bytes: number;
      };
    }
  | {
      method: 'session/historyPage';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
        cut: ObservationReadCut;
        domain: string;
        offset: number;
        limit: number;
      };
    }
  | {
      method: 'session/historyValueRead';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
        cut: ObservationReadCut;
        domain: string;
        item_offset: number;
        /**
         * @maxItems 32
         */
        path: string[];
        offset: number;
        max_bytes: number;
      };
    }
  | {
      method: 'session/statistics';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
      };
    }
  | {
      method: 'session/summary';
      params: {
        session_id: SessionId;
      };
    }
  | {
      method: 'session/name';
      params: {
        session_id: SessionId;
        name: string;
      };
    }
  | {
      method: 'session/node';
      params: {
        session_id: SessionId;
        node_id: SessionNodeId;
      };
    }
  | {
      method: 'session/nodeOriginHistory';
      params: {
        session_id: SessionId;
        node_id: SessionNodeId;
        limit: number;
      };
    }
  | {
      method: 'session/nodeValueRead';
      params: {
        reference: SessionNodeValueRef;
        offset: number;
        max_bytes: number;
      };
    }
  | {
      method: 'session/tree';
      params: {
        session_id: SessionId;
        offset: number;
        limit: number;
      };
    }
  | {
      method: 'session/boundaries';
      params: {
        target: AttachmentTarget;
        offset: number;
        limit: number;
      };
    }
  | {
      method: 'session/forkProjection';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
        ledger: LedgerFrontier;
        activation: ProjectionActivationId;
      };
    }
  | {
      method: 'session/fork';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
        surface_revision: SurfaceRevision;
        boundary?: CanonicalContentLocator | null;
        side: LineageSide;
      };
    }
  | {
      method: 'session/branch';
      params: {
        session_id: SessionId;
        node_id: SessionNodeId;
        surface_revision: SurfaceRevision;
        boundary: CanonicalContentLocator;
        side: LineageSide;
      };
    }
  | {
      method: 'session/deletePreview';
      params: {
        session_id: SessionId;
      };
    }
  | {
      method: 'session/delete';
      params: {
        session_id: SessionId;
        expected_target_revision: string;
      };
    }
  | {
      method: 'session/recoverDeletion';
      params: {
        session_id: SessionId;
      };
    }
  | {
      method: 'session/attach';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
      };
    }
  | {
      method: 'session/editorRead';
      params: {
        locator: EditorLocator;
        block_index: number;
        offset: number;
        max_bytes: number;
      };
    }
  | {
      method: 'session/detach';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'session/attachmentStatus';
      params: {
        session_id: SessionId;
      };
    }
  | {
      method: 'conversation/contentRead';
      params: {
        target: AttachmentTarget;
        agent_id?: AgentId | null;
        content_ref: ContentRef;
        offset: number;
        max_bytes: number;
      };
    }
  | {
      method: 'conversation/checkpointValueRead';
      params: {
        target: AttachmentTarget;
        agent_id?: AgentId | null;
        cut: CheckpointReadCut;
        domain: string;
        item_offset: number;
        /**
         * @maxItems 32
         */
        path: string[];
        offset: number;
        max_bytes: number;
      };
    }
  | {
      method: 'conversation/checkpointPage';
      params: {
        target: AttachmentTarget;
        agent_id?: AgentId | null;
        cut: CheckpointReadCut;
        domain: string;
        offset: number;
        limit: number;
      };
    }
  | {
      method: 'session/checkpoint';
      params: {
        target: AttachmentTarget;
        /**
         * Bounded loaded Trace identities to repair at the same snapshot cut.
         *
         * @maxItems 8
         */
        trace_records?:
          | []
          | [TraceCursor]
          | [TraceCursor, TraceCursor]
          | [TraceCursor, TraceCursor, TraceCursor]
          | [TraceCursor, TraceCursor, TraceCursor, TraceCursor]
          | [TraceCursor, TraceCursor, TraceCursor, TraceCursor, TraceCursor]
          | [TraceCursor, TraceCursor, TraceCursor, TraceCursor, TraceCursor, TraceCursor]
          | [
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor
            ]
          | [
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor,
              TraceCursor
            ];
      };
    }
  | {
      method: 'session/subscribe';
      params: {
        target: AttachmentTarget;
        after_cursor: RuntimeClientCursor;
      };
    }
  | {
      method: 'turn/start';
      params: {
        target: AttachmentTarget;
        content: UserInputBlock[];
      };
    }
  | {
      method: 'turn/steer';
      params: {
        target: AttachmentTarget;
        content: UserInputBlock[];
      };
    }
  | {
      method: 'inbound/edit';
      params: {
        target: AttachmentTarget;
        expected: PendingInboundRef;
        text: string;
      };
    }
  | {
      method: 'inbound/remove';
      params: {
        target: AttachmentTarget;
        expected: PendingInboundRef;
      };
    }
  | {
      method: 'turn/cancel';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'interaction/respond';
      params: {
        target: AttachmentTarget;
        interaction: InteractionRef;
        response: InteractionResponse;
      };
    }
  | {
      method: 'interaction/cancel';
      params: {
        target: AttachmentTarget;
        interaction: InteractionRef;
      };
    }
  | {
      method: 'session/effectiveConfiguration';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'session/effectiveConfigurationValueRead';
      params: {
        target: AttachmentTarget;
        reference: EffectiveFieldRef;
        offset: number;
        max_bytes: number;
      };
    }
  | {
      method: 'session/effectiveConfigurationPage';
      params: {
        target: AttachmentTarget;
        revision: string;
        field: EffectiveFieldName;
        offset: number;
        limit: number;
      };
    }
  | {
      method: 'configuration/sourcesRead';
      params: {
        target: SourceTarget;
      };
    }
  | {
      method: 'mcp/probe';
      params: {
        target: SourceTarget;
        id: McpServerId;
        expected_revision: string;
      };
    }
  | {
      method: 'configuration/sourcePage';
      params: {
        target: SourceTarget;
        revision: string;
        field: SourceFieldName;
        offset: number;
        limit: number;
      };
    }
  | {
      method: 'configuration/sourceValueRead';
      params: {
        target: SourceTarget;
        reference: SourceFieldRef;
        offset: number;
        max_bytes: number;
      };
    }
  | {
      method: 'workflow/list';
      params: {
        target: SourceTarget;
      };
    }
  | {
      method: 'workflow/read';
      params: {
        target: SourceTarget;
        id: WorkflowId;
      };
    }
  | {
      method: 'workflow/modelChoices';
      params: {
        target: SourceTarget;
        query: WorkflowModelChoicesQuery;
        expected_revision?: string | null;
      };
    }
  | {
      method: 'workflow/check';
      params: {
        target: SourceTarget;
        id: WorkflowId;
        document: WorkflowDefinition;
        /**
         * A User (Global) file's explicit Workspace dependency context. A
         * Workspace file is always checked in its own Workspace.
         */
        context?: string | null;
        explain?: boolean;
      };
    }
  | {
      method: 'configuration/sourceWrite';
      params: {
        target: SourceTarget;
        expected_revision: string;
        mutation: SourceMutation;
      };
    }
  | {
      method: 'session/settings';
      params: {
        session_id: SessionId;
      };
    }
  | {
      method: 'session/settingsValueRead';
      params: {
        session_id: SessionId;
        reference: SessionSettingsRef;
        offset: number;
        max_bytes: number;
      };
    }
  | {
      method: 'configuration/reconcile';
      params: {
        target: SourceTarget;
      };
    }
  | {
      method: 'session/configurationValueRead';
      params: {
        session_id: SessionId;
        reference: SessionApplicationRef;
        offset: number;
        max_bytes: number;
      };
    }
  | {
      method: 'session/configuration';
      params: {
        session_id: SessionId;
      };
    }
  | {
      method: 'session/adoptConfiguration';
      params: {
        session_id: SessionId;
        candidate: ApplicationIdentity;
        expected_binding: string;
      };
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionId".
 */
export type SessionId = string;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConversationId".
 */
export type ConversationId = string;
/**
 * Process-local live composition identity. Never a durable or transport identity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeIncarnationId".
 */
export type RuntimeIncarnationId = string;
/**
 * The identity of one Runtime Client attachment.
 *
 * Distinct from [`ConversationId`], [`AttemptId`], [`RuntimeClientCursor`],
 * and request ids: one attachment is one client session, and reconnecting
 * always receives a new attachment identity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AttachmentId".
 */
export type AttachmentId = string;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CorrelationId".
 */
export type CorrelationId = string;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ClassificationInput".
 */
export type ClassificationInput =
  | {
      text: string;
      mode: 'text';
    }
  | {
      message_id: MessageId;
      mode: 'history';
    };
/**
 * Identifies a committed canonical message block.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "MessageId".
 */
export type MessageId = string;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AuxiliaryCallId".
 */
export type AuxiliaryCallId = string;
/**
 * Bounded consumer-defined attribution, never execution authority.
 *
 * Purpose names, activation causes and business references use this same text
 * contract. Consumers own their vocabulary; adding a consumer does not change
 * the Store schema. Values must not contain prompts, output or secrets.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AuxiliaryAttribution".
 */
export type AuxiliaryAttribution = string;
/**
 * Identifies an Agent independently of its finite activations.
 * For a continuable child, this is the durable Agent-domain identity of
 * its child Conversation and remains unchanged across resume.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentId".
 */
export type AgentId = string;
/**
 * Identifies a durable artifact produced or referenced by the runtime.
 *
 * An artifact is identified by an opaque runtime-owned id, never by a
 * local filesystem path: paths are executor concerns and are not a
 * universal durable artifact identity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ArtifactId".
 */
export type ArtifactId = string;
/**
 * Finite identity in the existing canonical Ledger, never a new source authority.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CanonicalContentLocator".
 */
export type CanonicalContentLocator =
  | {
      message_id: MessageId;
      type: 'by_message_id';
    }
  | {
      position: number;
      type: 'by_ledger_position';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionNodeId".
 */
export type SessionNodeId = string;
/**
 * Checkpoint activation order, independent of the Ledger and Journal domains.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ProjectionActivationId".
 */
export type ProjectionActivationId = string;
/**
 * Immutable historical context content, independent of canonical message identity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CheckpointId".
 */
export type CheckpointId = string;
/**
 * Opaque Trace-only exclusive boundary, valid only in its conversation.
 *
 * It is not a live cursor, a transcript cursor, an Event Journal sequence,
 * a Request ID, or an artifact ID. Clients order by it and page with it;
 * they never parse it.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceCursor".
 */
export type TraceCursor = string;
/**
 * A stable index identifying one content block within the ordered content
 * list of the canonical message being assembled.
 *
 * Streaming facts (text deltas, reasoning deltas, provider continuation
 * state, tool-call content) reference the block they belong to by this
 * index, so interleaved and multiple blocks remain unambiguous without
 * exposing any provider-specific block id type.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ContentBlockIndex".
 */
export type ContentBlockIndex = number;
/**
 * Provider/model-issued opaque correlation string, scoped to a request/publication.
 * Not a rustX global identity: canonical ownership uses `ToolCallOccurrenceRef`.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolCallId".
 */
export type ToolCallId = string;
/**
 * Identifies a tool definition in the capability set.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolId".
 */
export type ToolId = string;
/**
 * Bounded windows positioned by native Turn identity or by a captured read cut.
 * Append-only live growth preserves a cut; mutation invalidates it.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConversationWindowAt".
 */
export type ConversationWindowAt =
  | {
      type: 'latest';
    }
  | {
      before: TranscriptCursor;
      cut?: ConversationReadCut | null;
      type: 'older';
    }
  | {
      after: TranscriptCursor;
      cut: ConversationReadCut;
      type: 'newer';
    }
  | {
      id: ConversationTurnId;
      cut: ConversationReadCut;
      type: 'turn';
    }
  | {
      ordinal: number;
      cut: ConversationReadCut;
      type: 'turn_ordinal';
    }
  | {
      locator: CanonicalContentLocator;
      cut?: ConversationReadCut | null;
      type: 'canonical';
    }
  | {
      id: MessageId;
      cut?: ConversationReadCut | null;
      type: 'message';
    };
/**
 * A durable transcript cursor.
 *
 * This cursor belongs to the transcript ordering spine. It is deliberately
 * distinct from the Runtime Client observation cursor, the Event Journal
 * sequence, and the inbound mailbox sequence.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TranscriptCursor".
 */
export type TranscriptCursor = string;
/**
 * Identifies one attempt to execute an agent manifest.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AttemptId".
 */
export type AttemptId = string;
/**
 * The identity of one Model Profile declared by a model.
 *
 * The runtime assigns no meaning to the name: `fast`, `precise`, `creative`,
 * `thinking-32k`, and `deep` are all just names whose behaviour is exactly
 * the profile's declared reasoning state, output default, and
 * `request_params`.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelProfileId".
 */
export type ModelProfileId = string;
/**
 * A Session auxiliary invocation follows primary or selects a catalog model.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AuxiliaryModelPolicy".
 */
export type AuxiliaryModelPolicy =
  | {
      mode: 'session';
    }
  | {
      /**
       * The catalog model reference.
       */
      model: string;
      /**
       * The selected Model Profile; the model's `default_profile` is used
       * when absent.
       */
      profile?: ModelProfileId | null;
      /**
       * The explicit auxiliary request-parameter overrides.
       */
      request_params?: {
        [k: string]: unknown;
      };
      /**
       * The explicit auxiliary output-budget override.
       */
      max_output_tokens?: number | null;
      mode: 'explicit';
    };
/**
 * Manual compaction correlation: 1..64 ASCII letters, digits, underscores or hyphens; not an idempotency key.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ManualCompactionRequestId".
 */
export type ManualCompactionRequestId = string;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduledWorkflowStartId".
 */
export type ScheduledWorkflowStartId = string;
/**
 * Trusted product-control commands. Model request grants are a separate seam.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleMutation".
 */
export type ScheduleMutation =
  | {
      title: string;
      workflow_id: WorkflowId;
      arguments: unknown;
      timing: ScheduleTiming;
      type: 'create_workflow';
    }
  | {
      id: ScheduleId;
      expected_revision: string;
      title: string;
      workflow_id: WorkflowId;
      arguments: unknown;
      timing?: ScheduleTiming | null;
      type: 'update_workflow';
    }
  | {
      title: string;
      text: string;
      timing: ScheduleTiming;
      type: 'create';
    }
  | {
      id: ScheduleId;
      expected_revision: string;
      title: string;
      text: string;
      timing?: ScheduleTiming | null;
      type: 'update';
    }
  | {
      id: ScheduleId;
      expected_revision: string;
      enabled: boolean;
      type: 'set_enabled';
    }
  | {
      id: ScheduleId;
      expected_revision: string;
      type: 'delete';
    };
/**
 * The configured workflow identity.
 *
 * This is both the catalog key and the eventual model-facing Tool name. It
 * is deliberately not repeated inside YAML.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowId".
 */
export type WorkflowId = string;
/**
 * Authored time, before the first successful command resolves relative input.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleTiming".
 */
export type ScheduleTiming =
  | {
      at: string;
      type: 'at';
    }
  | {
      seconds: number;
      type: 'after';
    }
  | {
      every_seconds: number;
      type: 'interval';
    }
  | {
      expression: string;
      time_zone: string;
      type: 'cron';
    };
/**
 * Typed Runtime Client control; an existing-state mutation always names its observation.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "GoalControl".
 */
export type GoalControl =
  | {
      action: 'show';
    }
  | {
      objective: string;
      budget?: number;
      action: 'create';
    }
  | {
      expected: GoalRef;
      mutation: GoalMutation;
      action: 'mutate';
    };
/**
 * Explicit user/control mutations. Model adapters expose only Block/Complete.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "GoalMutation".
 */
export type GoalMutation =
  | {
      action: 'pause';
    }
  | {
      action: 'resume';
    }
  | {
      reason: string;
      action: 'block';
    }
  | {
      action: 'complete';
    }
  | {
      objective: string;
      action: 'edit';
    }
  | {
      rounds: number;
      action: 'budget';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolExecutionId".
 */
export type ToolExecutionId = string;
/**
 * Identifies one finite activation owned by an Agent, or one finite
 * Workflow child execution.
 *
 * Repeated native Agent activations have distinct `SubagentId`s while
 * retaining the same durable `AgentId`. This is not an OS pid: process
 * identity or PID reuse can never establish durable ownership or proof
 * of physical settlement.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SubagentId".
 */
export type SubagentId = string;
/**
 * The external cursor of the Runtime Client observation stream.
 *
 * A cursor identifies one position in the externally visible
 * `RuntimeClientEvent` sequence: cursor `C` means "every Runtime Client
 * event through event `C` has been applied to the snapshot". It is:
 *
 * - monotonic within the Runtime Client observation stream;
 * - **not** an alias of `u64`;
 * - **not** the mailbox [`InboundSequence`](crate::runtime::inbound::InboundSequence);
 * - **not** [`RuntimeEventEnvelope::sequence`](crate::events::types::RuntimeEventEnvelope);
 * - **not** any future Event Journal sequence;
 * - owned by the runtime observation stream, so it survives attachment
 *   detach/reconnect;
 * - independent of its numeric representation: protocol versioning never
 *   derives anything from cursor encoding.
 *
 * Cursor allocation is committed by exactly one linearization owner (the
 * Runtime Client projection) together with event publication; overflow
 * fails explicitly and never wraps.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientCursor".
 */
export type RuntimeClientCursor = string;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ContentSource".
 */
export type ContentSource =
  | {
      component: CanonicalContentComponent;
      locator: CanonicalContentLocator;
      block_index: ContentBlockIndex;
      type: 'canonical_message_block';
    }
  | {
      component: PublicationContentComponent;
      stream_id: PublicationStreamId;
      message_id: MessageId;
      attempt_id: AttemptId;
      turn_id: TurnId;
      publication_request_id: RequestId;
      block_index: ContentBlockIndex;
      type: 'durable_transcript_audit';
    }
  | {
      component: PublicationContentComponent;
      stream_id: PublicationStreamId;
      message_id: MessageId;
      attempt_id: AttemptId;
      turn_id: TurnId;
      publication_request_id: RequestId;
      block_index: ContentBlockIndex;
      type: 'live_publication_prefix';
    };
/**
 * Exact deterministic component of one immutable canonical message.
 * Metadata encodings omit content payload fields; loading them merges metadata
 * into the corresponding message/block without replacing already loaded bytes.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CanonicalContentComponent".
 */
export type CanonicalContentComponent = ('payload' | 'block_metadata') | 'message_metadata';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "PublicationContentComponent".
 */
export type PublicationContentComponent = 'payload' | 'block_metadata';
/**
 * Identifies one user-facing publication stream (Issue #108, FND-03).
 *
 * A publication stream is the durable release plane of exactly one
 * provider request: it is the identity under which committed-for-release
 * frames are staged, under which the publication terminal marker
 * commits, and under which the stream settles exactly once as canonical,
 * unaccepted, or incomplete. It is deliberately distinct from
 * [`RequestId`] (the provider execution fact) and from [`MessageId`]
 * (the canonical conversation fact), because publication durability is
 * a separate plane from both.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "PublicationStreamId".
 */
export type PublicationStreamId = string;
/**
 * Identifies one turn within an attempt.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TurnId".
 */
export type TurnId = string;
/**
 * Identifies one actual provider-neutral model request.
 *
 * A request identity is distinct from an attempt, turn, retry ordinal,
 * and Event Journal sequence. Primary requests derive the `request:`
 * namespace from immutable [`RequestIdentity`](crate::model::snapshot::RequestIdentity).
 * Auxiliary requests allocate the separate `req_` `UUIDv7` namespace with
 * [`RequestId::generate_auxiliary`]. Each physical retry is a distinct
 * request. Identity alone grants no execution permission.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RequestId".
 */
export type RequestId = string;
/**
 * The identity of one exact historical Conversation Surface state.
 *
 * A revision is a monotonic counter in its own identity domain. The empty
 * Surface of a new conversation is [`SurfaceRevision::INITIAL`] (`0`), and
 * every accepted [`SurfaceOp`] advances it by exactly one, so revision `n`
 * is precisely "the Surface after the first `n` accepted operations".
 *
 * A revision is deliberately **not** a `MessageId`, an `AttemptId`, a
 * `RuntimeClientCursor`, an `InboundSequence`, an Event Journal sequence,
 * or a `CapabilityRevision`: none of those identify a Surface state, and
 * none of them may be substituted for one.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SurfaceRevision".
 */
export type SurfaceRevision = string;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ContentRepresentation".
 */
export type ContentRepresentation =
  ('utf8_text' | 'raw_json_utf8' | 'binary_bytes') | 'canonical_json_v1';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionNodeValueField".
 */
export type SessionNodeValueField = 'origin_source_message';
/**
 * Inclusive canonical commit position, independent of Journal and activation order.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "LedgerFrontier".
 */
export type LedgerFrontier = string;
/**
 * Which history prefix a lineage operation retains.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "LineageSide".
 */
export type LineageSide = 'before' | 'after';
/**
 * Clients author text and reference completed server receipts only.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UserInputBlock".
 */
export type UserInputBlock =
  | {
      /**
       * The text content.
       */
      text: string;
      type: 'text';
    }
  | {
      session_id: SessionId;
      batch_id: string;
      token: string;
      type: 'upload';
    };
/**
 * A typed response to one native interaction.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InteractionResponse".
 */
export type InteractionResponse =
  | {
      response: ReviewResponse;
      type: 'review';
    }
  | {
      /**
       * The finite approval decision.  It has no tool arguments.
       */
      decision:
        | {
            type: 'allow';
          }
        | {
            /**
             * A bounded client-facing reason.
             */
            reason: string;
            type: 'deny';
          };
      type: 'approval';
    }
  | {
      /**
       * A submitted answer set or explicit decline.
       */
      response:
        | {
            type: 'submitted';
            value: QuestionnaireSubmission;
          }
        | {
            type: 'declined';
          };
      type: 'questionnaire';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ReviewDecision".
 */
export type ReviewDecision =
  | {
      type: 'accepted';
    }
  | {
      feedback: string;
      type: 'rejected';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectiveFieldName".
 */
export type EffectiveFieldName =
  | 'process_bindings'
  | 'application'
  | 'adopted_binding'
  | 'source_revisions'
  | 'generation'
  | 'document'
  | 'root_agent'
  | 'context'
  | 'model_timeout'
  | 'tool_deadline'
  | 'child_capacity'
  | 'approval_mode'
  | 'provenance'
  | 'resources'
  | 'available_tools'
  | 'session_model'
  | 'effective_model'
  | 'admitted_attempt';
/**
 * Native source authority. A Session is never an authoring target.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceTarget".
 */
export type SourceTarget =
  | {
      kind: 'user';
    }
  | {
      directory: string;
      kind: 'workspace';
    };
/**
 * Identifies an MCP server bound to the runtime.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "McpServerId".
 */
export type McpServerId = string;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceFieldName".
 */
export type SourceFieldName =
  | 'target'
  | 'process_policy_impacts'
  | 'process_bindings'
  | 'application'
  | 'prospective_approval_mode'
  | 'prospective_resources'
  | 'prospective_diagnostic'
  | 'absent_resource_revision'
  | 'resource_revisions'
  | 'resolved'
  | 'session_models'
  | 'provenance'
  | 'user'
  | 'workspace'
  | 'user_repair'
  | 'workspace_repair'
  | 'repair_sequence'
  | 'user_resource_root'
  | 'workspace_resource_root'
  | 'runtime_root'
  | 'user_mcp'
  | 'workspace_mcp'
  | 'agents'
  | 'user_models'
  | 'workspace_models'
  | 'resolved_models';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowModelChoicesQuery".
 */
export type WorkflowModelChoicesQuery =
  | {
      offset: number;
      type: 'models';
    }
  | {
      model: ModelRef;
      offset: number;
      type: 'profiles';
    };
/**
 * An authored Model identity. Its spelling has no provider or wire semantics.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelRef".
 */
export type ModelRef = string;
/**
 * One Workflow node in the authoring/domain layer.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowNodeDefinition".
 */
export type WorkflowNodeDefinition =
  | {
      model?: AuxiliaryModelPolicy;
      instruction: string;
      input?: {
        [k: string]: WorkflowValue;
      };
      output: WorkflowModelOutput;
      type: 'model';
    }
  | {
      input: WorkflowValue;
      body: WorkflowBlock1;
      until: WorkflowPredicate;
      carry: WorkflowValue;
      max_iterations: number;
      type: 'loop';
    }
  | {
      subject: WorkflowReviewSubject;
      context: WorkflowValue[];
      type: 'review';
    }
  | {
      selector: ExactToolSelector;
      arguments: WorkflowValue;
      result: WorkflowToolResult;
      type: 'tool';
    }
  | {
      /**
       * The native named profile to resolve at `AgentRun` admission.
       */
      profile: string;
      /**
       * The fixed task instruction for this `AgentRun`.
       */
      task: string;
      /**
       * Explicit input bindings from workflow-local values.
       */
      input?: {
        [k: string]: WorkflowValue;
      };
      /**
       * The frozen `AgentRun` output contract.
       */
      output: {
        [k: string]: unknown;
      };
      override?: SubagentInvocationOverride;
      type: 'agent';
    }
  | {
      /**
       * The sole boolean condition binding.
       */
      condition:
        | {
            value: WorkflowValue;
            type: 'boolean';
          }
        | {
            left: WorkflowValue;
            right: WorkflowValue;
            type: 'equal';
          }
        | {
            left: WorkflowValue;
            right: WorkflowValue;
            type: 'not_equal';
          }
        | {
            predicate: WorkflowPredicate;
            type: 'not';
          }
        | {
            predicates: WorkflowPredicate[];
            type: 'and';
          }
        | {
            predicates: WorkflowPredicate[];
            type: 'or';
          };
      type: 'branch';
    }
  | {
      /**
       * Branches are keyed by definition identity, not completion order.
       */
      branches: {
        [k: string]: WorkflowBranch;
      };
      type: 'parallel';
    }
  | {
      /**
       * Closed, explicitly tagged value syntax; no evaluation of expression strings.
       */
      output:
        | {
            path: string[];
            type: 'reference';
          }
        | {
            value: unknown;
            type: 'literal';
          }
        | {
            fields: {
              [k: string]: WorkflowValue;
            };
            type: 'object';
          }
        | {
            items: WorkflowValue[];
            type: 'array';
          };
      type: 'return';
    };
/**
 * Closed, explicitly tagged value syntax; no evaluation of expression strings.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowValue".
 */
export type WorkflowValue =
  | {
      path: string[];
      type: 'reference';
    }
  | {
      value: unknown;
      type: 'literal';
    }
  | {
      fields: {
        [k: string]: WorkflowValue;
      };
      type: 'object';
    }
  | {
      items: WorkflowValue[];
      type: 'array';
    };
/**
 * The single authored output contract of a Workflow Model node.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowModelOutput".
 */
export type WorkflowModelOutput =
  | {
      type: 'text';
    }
  | {
      schema: unknown;
      type: 'json_schema';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowPort".
 */
export type WorkflowPort = boolean | ('true' | 'false' | 'satisfied' | 'exhausted' | 'next');
/**
 * Typed predicates. Equality requires matching scalar types; no coercion.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowPredicate".
 */
export type WorkflowPredicate =
  | {
      value: WorkflowValue;
      type: 'boolean';
    }
  | {
      left: WorkflowValue;
      right: WorkflowValue;
      type: 'equal';
    }
  | {
      left: WorkflowValue;
      right: WorkflowValue;
      type: 'not_equal';
    }
  | {
      predicate: WorkflowPredicate;
      type: 'not';
    }
  | {
      predicates: WorkflowPredicate[];
      type: 'and';
    }
  | {
      predicates: WorkflowPredicate[];
      type: 'or';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowReviewSubject".
 */
export type WorkflowReviewSubject = {
  value: WorkflowValue;
  type: 'plan';
};
/**
 * One exact executable identity, used by Workflow Tool leaves and allowlists.
 * Source-wide Agent capability selection cannot be authored in this type.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ExactToolSelector".
 */
export type ExactToolSelector =
  | {
      name: string;
      origin: 'builtin';
    }
  | {
      source_id: ToolSourceId;
      name: string;
      origin: 'source';
    };
/**
 * A configured MCP source or a canonical Managed Python package.
 * Namespace parsing happens only at the authoring boundary; runtime dispatch
 * matches these variants, never display strings.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolSourceId".
 */
export type ToolSourceId = string;
/**
 * Select exactly one native content part, never provider text or log heuristics.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowToolResult".
 */
export type WorkflowToolResult =
  | {
      part: number;
      schema: unknown;
      type: 'json';
    }
  | {
      part: number;
      type: 'text';
    };
/**
 * Exactly two trust granularities for one source. This is also the authoring
 * boundary: only the literal "all" or an exact array is accepted.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceToolSelection".
 */
export type SourceToolSelection = AllTools | string[];
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AllTools".
 */
export type AllTools = 'all';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceMutation".
 */
export type SourceMutation =
  | {
      document: string;
      kind: 'repair_config';
    }
  | {
      id: McpServerId;
      authored?: McpWrite | null;
      kind: 'mcp';
    }
  | {
      mutation: ConfigMutation;
      kind: 'config';
    }
  | {
      name: SubagentName;
      authored?: AgentProfileDocument | null;
      kind: 'agent';
    }
  | {
      id: WorkflowId;
      edit: WorkflowEdit;
      kind: 'workflow';
    };
/**
 * A validated reference, written as `$ENV_VAR` only in declared secret fields.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EnvironmentReference".
 */
export type EnvironmentReference = string;
/**
 * The transport an `mcpServers` entry selects explicitly.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "McpTransportType".
 */
export type McpTransportType = 'http' | 'stdio';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConfigMutation".
 */
export type ConfigMutation =
  | {
      id: string;
      authored?: ProviderWrite | null;
      unit: 'provider';
    }
  | {
      id: string;
      authored?: Model | null;
      unit: 'model';
    }
  | {
      authored?: ModelLayer | null;
      unit: 'root_model';
    }
  | {
      authored?: string[] | null;
      unit: 'native_tools';
    }
  | {
      id: ToolSourceId;
      authored?: SourceToolSelection | null;
      unit: 'source_tools';
    }
  | {
      authored?: AgentSkillSelection | null;
      unit: 'skills';
    }
  | {
      authored?: TodoExtensionDocument1 | null;
      unit: 'todo';
    }
  | {
      authored?: GoalExtensionDocument | null;
      unit: 'goal';
    }
  | {
      authored?: AgentStatusExtensionDocument1 | null;
      unit: 'agent_status';
    }
  | {
      authored?: SubagentName[] | null;
      unit: 'agents';
    }
  | {
      authored?: WorkflowId[] | null;
      unit: 'workflows';
    }
  | {
      authored?: AgentId | null;
      unit: 'agent_identity';
    }
  | {
      authored?: string | null;
      unit: 'description';
    }
  | {
      authored?: string | null;
      unit: 'instructions';
    }
  | {
      authored?: AgentProjectInstructionsDocument | null;
      unit: 'project_guidance';
    }
  | {
      authored?: ContextLayer | null;
      unit: 'context';
    }
  | {
      authored?: TimeoutLayer | null;
      unit: 'model_timeout';
    }
  | {
      authored?: ToolDeadlineLayer | null;
      unit: 'tool_deadline';
    }
  | {
      authored?: SubagentsLayer | null;
      unit: 'capacity';
    }
  | {
      authored?: ApprovalMode | null;
      unit: 'approval';
    }
  | {
      id: NativeTool;
      authored?: NativePolicyOverrideDocument | null;
      unit: 'native_policy';
    }
  | {
      id: McpServerId;
      authored?: InvocationPolicyDocument | null;
      unit: 'mcp_policy';
    }
  | {
      name: string;
      authored?: string | null;
      unit: 'environment';
    }
  | {
      authored?: AppServerPolicy | null;
      unit: 'app_server';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CredentialEdit".
 */
export type CredentialEdit =
  | {
      kind: 'retain';
    }
  | {
      variable: string;
      kind: 'environment';
    }
  | {
      value: string;
      kind: 'literal';
    };
/**
 * The model interaction protocol an adapter must speak.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelProtocol".
 */
export type ModelProtocol = 'openai_chat_completions' | 'openai_responses' | 'anthropic_messages';
/**
 * One semantic content modality of a model capability set.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "Modality".
 */
export type Modality = 'text' | 'image' | 'file';
/**
 * Which max-token field spelling a Chat Completions service accepts.
 *
 * This is a real structural translation difference between
 * OpenAI-compatible services, not a provider wire value: the two spellings
 * are mutually exclusive and both are runtime-protected.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ChatMaxTokensField".
 */
export type ChatMaxTokensField = 'max_completion_tokens' | 'max_tokens';
/**
 * Whether a Chat Completions service supports streaming usage options.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ChatStreamUsage".
 */
export type ChatStreamUsage = 'supported' | 'unsupported';
/**
 * Assistant-message field used to replay canonical reasoning through an
 * OpenAI-compatible Chat Completions dialect.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ChatReasoningReplay".
 */
export type ChatReasoningReplay = 'reasoning' | 'reasoning_content' | 'omit';
/**
 * The in-band tool protocol a Chat Completions model speaks.
 *
 * This is a real protocol difference between OpenAI-compatible services,
 * not a provider wire value. Most services emit tool calls only through the
 * structured `tool_calls` field. Some model families additionally have a
 * *reserved in-band* tool syntax that the serving stack is supposed to parse
 * out of the generated text; when that parse fails, the reserved markup
 * leaks into ordinary content or reasoning and the request terminates as if
 * the model had simply answered.
 *
 * Declaring the dialect is what allows the adapter to recognize such a leak
 * as malformed tool intent instead of guessing from arbitrary text. Nothing
 * is ever inferred from a provider name or a base URL hostname.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ChatToolProtocol".
 */
export type ChatToolProtocol = 'native' | 'qwen_xml';
/**
 * How the `OpenAI` Responses protocol operates with provider storage.
 *
 * This is continuation *structure*, not a wire value: Stored continues by
 * `previous_response_id`, Stateless continues by preserved output items and
 * requires the encrypted-reasoning `include` value.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ResponsesStorageMode".
 */
export type ResponsesStorageMode = 'stored' | 'stateless';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelOutput".
 */
export type ModelOutput =
  | {
      mode: 'catalog_default';
    }
  | {
      tokens: number;
      mode: 'limit';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AuxiliaryModelAuthoring".
 */
export type AuxiliaryModelAuthoring =
  | {
      mode: 'session';
    }
  | {
      model: ModelRef;
      profile?: ModelProfileId | null;
      request_params?: RequestParams;
      max_output_tokens?: ModelOutput | null;
      mode: 'explicit';
    };
/**
 * Prompt visibility within a frozen Skill catalog; never filesystem authority.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentSkillSelection".
 */
export type AgentSkillSelection = AllTools | string[];
/**
 * The canonical typed name of one admitted subagent definition.
 *
 * The keyspace is deliberately narrow: lowercase ASCII letters, digits,
 * `-`, and `_`, starting with a letter. A name is the model-facing routing
 * token, the durable ownership identity, and the Runtime Client projection
 * identity, so an ambiguous or shell-shaped spelling is rejected at the
 * configuration boundary rather than normalized later.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SubagentName".
 */
export type SubagentName = string;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SummaryOutput".
 */
export type SummaryOutput =
  | {
      mode: 'model_limit';
    }
  | {
      tokens: number;
      mode: 'limit';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "IdleLiveness".
 */
export type IdleLiveness =
  | {
      mode: 'disabled';
    }
  | {
      milliseconds: string;
      mode: 'window';
    };
/**
 * The runtime-wide control state for tool approval behavior.
 *
 * `Policy` consults each resolved Tool's
 * [`ToolApprovalPolicy`](crate::tools::types::ToolApprovalPolicy).
 * `FullAccess` changes only the effective approval result to `Never`; it
 * never changes availability, activation, execution ownership, concurrency,
 * or tool authority.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ApprovalMode".
 */
export type ApprovalMode = 'policy' | 'full_access';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "NativeTool".
 */
export type NativeTool = 'read' | 'write' | 'edit' | 'glob' | 'grep' | 'bash';
/**
 * One whole-file Workflow mutation of the exact file the mutation names.
 *
 * The surrounding source write carries the destination's expected
 * revision: the absent revision for `create` and `copy`, the exact current
 * revision for `replace` and `delete`.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowEdit".
 */
export type WorkflowEdit =
  | {
      description: string;
      operation: 'create';
    }
  | {
      document: WorkflowDefinition;
      operation: 'replace';
    }
  | {
      source: WorkflowId;
      source_revision: string;
      operation: 'copy';
    }
  | {
      operation: 'delete';
    };
/**
 * Success and failure are exclusive, including on deserialization.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "Response".
 */
export type Response = Success | Failure;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "MethodResult".
 */
export type MethodResult =
  | {
      recovery?: ScheduledWorkflowRecovery | null;
      type: 'scheduled_workflow_recovery';
    }
  | {
      schedules: ScheduleHeader[];
      next_ordinal?: string | null;
      type: 'schedules';
    }
  | {
      schedule?: ScheduleSnapshot | null;
      type: 'schedule';
    }
  | {
      result: ScheduleCommandResult;
      type: 'schedule_command';
    }
  | {
      generation: string;
      type: 'schedule_storage_recovered';
    }
  | {
      receipt: AuxiliarySubmissionReceipt;
      type: 'auxiliary_submitted';
    }
  | {
      status: AuxiliaryStatus;
      type: 'auxiliary_status';
    }
  | {
      result?: AuxiliaryResult | null;
      type: 'auxiliary_result';
    }
  | {
      lookup: AuxiliarySubmissionLookup;
      type: 'auxiliary_submission_lookup';
    }
  | {
      accepted: boolean;
      type: 'auxiliary_cancelled';
    }
  | {
      discarded: boolean;
      type: 'auxiliary_discarded';
    }
  | {
      file: SessionFileReference;
      data: string;
      type: 'session_file_bytes';
    }
  | {
      accepted: boolean;
      type: 'delivery_cancel';
    }
  | {
      file: SessionFileReference;
      /**
       * Absolute path in the server's filesystem namespace.
       */
      path: string;
      /**
       * Lossless decimal device/inode of the verified regular leaf.
       */
      device: string;
      inode: string;
      type: 'session_file_location';
    }
  | {
      download: ArchiveDownloadDescriptor;
      type: 'session_archive';
    }
  | {
      outcome: PendingMutationOutcome;
      type: 'inbound_mutation';
    }
  | {
      data: string;
      type: 'artifact_bytes';
    }
  | {
      transfer: UploadDescriptor;
      type: 'upload_prepared';
    }
  | {
      outcome: UploadOutcome;
      type: 'upload_status';
    }
  | {
      snapshot: ServerDiagnostics;
      type: 'diagnostics';
    }
  | {
      compaction?: RuntimeClientCompactionView | null;
      type: 'context_compacted';
    }
  | {
      page: ProjectionPage;
      type: 'projections';
    }
  | {
      detail: ProjectionDetail;
      type: 'projection_detail';
    }
  | {
      page: TracePage;
      type: 'trace';
    }
  | {
      location?: TraceToolLocation | null;
      type: 'trace_tool_location';
    }
  | {
      conversation_id: ConversationId;
      window: ConversationWindow;
      type: 'session_history';
    }
  | {
      conversation_id: ConversationId;
      read_cut: CheckpointReadCut;
      statistics: ConversationStatistics;
      occupancy?: RuntimeClientContextOccupancy | null;
      type: 'session_statistics';
    }
  | {
      conversation_id: ConversationId;
      page: TracePage;
      type: 'session_trace_history';
    }
  | {
      conversation_id: ConversationId;
      page: CheckpointPage;
      type: 'session_trace_history_detail';
    }
  | {
      window: ConversationWindow;
      type: 'transcript_window';
    }
  | {
      page: ObservedTurnPage;
      type: 'conversation_turns';
    }
  | {
      reference?: GoalRef | null;
      type: 'goal_mutation';
    }
  | {
      job_id: ToolExecutionId;
      type: 'job_cancellation_accepted';
    }
  | {
      agent_id: AgentId;
      activation_id?: SubagentId | null;
      type: 'agent_interruption_accepted';
    }
  | {
      job_id: ToolExecutionId;
      state: BackgroundLifecycle;
      type: 'job_outcome';
    }
  | {
      accepted: boolean;
      type: 'agent_observe_cancelled';
    }
  | {
      observation: AgentObservation;
      type: 'agent_observe';
    }
  | {
      metrics: AgentStatistics;
      type: 'agent_statistics';
    }
  | {
      agent_id: AgentId;
      activation_id: SubagentId;
      resumed: boolean;
      type: 'agent_message';
    }
  | {
      agent_id: AgentId;
      activation_id?: SubagentId | null;
      outcome?: SubagentState | null;
      type: 'agent_wait';
    }
  | {
      /**
       * Opaque process-owner identity, stable across transport reconnects. Not permission.
       */
      authority_id: string;
      protocol_version: number;
      capabilities: ServerCapabilities;
      type: 'initialized';
    }
  | {
      capabilities: ServerCapabilities;
      type: 'server_info';
    }
  | {
      session: SessionSnapshot;
      type: 'session';
    }
  | {
      page: EditorPage;
      type: 'editor_page';
    }
  | {
      session: SessionSnapshot;
      editor?: EditorLocator | null;
      durability_diagnostic?: string | null;
      type: 'session_transition';
    }
  | {
      summary: SessionSummary;
      type: 'session_summary';
    }
  | {
      sessions: SessionSummary[];
      next_offset?: number | null;
      type: 'sessions';
    }
  | {
      node: SessionNodeObservation;
      type: 'node';
    }
  | {
      chunk: SessionNodeValueChunk;
      type: 'node_value_chunk';
    }
  | {
      nodes: SessionNodeObservation[];
      next_offset?: number | null;
      type: 'tree';
    }
  | {
      /**
       * The identity of one exact historical Conversation Surface state.
       *
       * A revision is a monotonic counter in its own identity domain. The empty
       * Surface of a new conversation is [`SurfaceRevision::INITIAL`] (`0`), and
       * every accepted [`SurfaceOp`] advances it by exactly one, so revision `n`
       * is precisely "the Surface after the first `n` accepted operations".
       *
       * A revision is deliberately **not** a `MessageId`, an `AttemptId`, a
       * `RuntimeClientCursor`, an `InboundSequence`, an Event Journal sequence,
       * or a `CapabilityRevision`: none of those identify a Surface state, and
       * none of them may be substituted for one.
       */
      surface_revision: string;
      boundaries: SessionUserMessageBoundaryObservation[];
      next_offset?: number | null;
      type: 'boundaries';
    }
  | {
      result: RuntimeClientSessionDeletionResult;
      type: 'deletion';
    }
  | {
      target: AttachmentTarget;
      configuration_revision?: string | null;
      attachment_kind: AttachmentKind;
      observation_source: ObservationSource2;
      type: 'attached';
    }
  | {
      target?: AttachmentTarget | null;
      configuration_revision?: string | null;
      type: 'attachment_status';
    }
  | {
      chunk: ContentChunk;
      type: 'content_chunk';
    }
  | {
      page: CheckpointPage;
      type: 'domain_page';
    }
  | {
      chunk: CheckpointValueChunk;
      type: 'checkpoint_value_chunk';
    }
  | {
      page: CheckpointPage;
      type: 'checkpoint_page';
    }
  | {
      checkpoint: RuntimeClientCheckpoint;
      type: 'checkpoint';
    }
  | {
      type: 'detached';
    }
  | {
      registration_id: string;
      target: AttachmentTarget;
      after_cursor: RuntimeClientCursor;
      type: 'subscribed';
    }
  | {
      message_id: MessageId;
      inbound_sequence: InboundSequence;
      type: 'inbound_accepted';
    }
  | {
      attempt_id: AttemptId;
      type: 'cancellation_accepted';
    }
  | {
      interaction: InteractionRef;
      type: 'interaction_settled';
    }
  | {
      chunk: EffectiveValueChunk;
      type: 'configuration_value_chunk';
    }
  | {
      page: EffectivePage;
      type: 'configuration_page';
    }
  | {
      scope: string;
      version: string;
      type: 'configuration_receipt';
    }
  | {
      revision: string;
      configuration_revision?: string | null;
      type: 'sources_written';
    }
  | {
      settings_revision: string;
      type: 'model_set';
    }
  | {
      projection: BoundedEffectiveConfiguration;
      type: 'effective_configuration';
    }
  | {
      result: McpProbeResult;
      type: 'mcp_probe';
    }
  | {
      page: SourceFieldPage;
      type: 'source_page';
    }
  | {
      chunk: SourceValueChunk;
      type: 'source_value_chunk';
    }
  | {
      projection: BoundedSourceSettings;
      type: 'source_settings';
    }
  | {
      library: WorkflowLibrary;
      type: 'workflow_library';
    }
  | {
      source: WorkflowSource;
      type: 'workflow_source';
    }
  | {
      choices: WorkflowModelChoices;
      type: 'workflow_model_choices';
    }
  | {
      report: WorkflowCheckReport;
      type: 'workflow_check';
    }
  | {
      projection?: BoundedSessionApplication | null;
      type: 'session_configuration';
    }
  | {
      chunk: SessionApplicationChunk;
      type: 'session_application_value_chunk';
    }
  | {
      chunk: SessionSettingsChunk;
      type: 'settings_value_chunk';
    }
  | {
      projection: BoundedSessionSettings;
      type: 'settings';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduledWorkflowState".
 */
export type ScheduledWorkflowState =
  | {
      waiting?: string | null;
      retry_after?: string | null;
      type: 'accepted';
    }
  | {
      owner: string;
      claimed_at: string;
      effects_started: boolean;
      type: 'claimed';
    }
  | {
      outcome: ScheduledWorkflowOutcome;
      settled_at: string;
      type: 'settled';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduledWorkflowOutcome".
 */
export type ScheduledWorkflowOutcome =
  | {
      type: 'completed';
    }
  | {
      reason: string;
      type: 'failed';
    }
  | {
      type: 'cancelled';
    }
  | {
      type: 'timed_out';
    }
  | {
      reason: string;
      type: 'rejected_before_start';
    }
  | {
      reason: string;
      type: 'interrupted';
    }
  | {
      reason: string;
      type: 'outcome_unknown';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduledEffectKind".
 */
export type ScheduledEffectKind = 'tool' | 'agent' | 'model' | 'review';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduledEffectOwner".
 */
export type ScheduledEffectOwner =
  | {
      type: 'preparing';
    }
  | {
      invocation: ToolInvocationId;
      type: 'tool';
    }
  | {
      activation: SubagentId;
      type: 'agent';
    }
  | {
      call_id: AuxiliaryCallId;
      type: 'model';
    }
  | {
      type: 'review';
    };
/**
 * Caller correlation for one native invocation. Capability source is
 * independently represented by [`ToolOrigin`].
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolInvocationId".
 */
export type ToolInvocationId =
  | {
      call_id: ToolCallId;
      caller: 'agent';
    }
  | {
      node: WorkflowNodeInstance;
      caller: 'workflow';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleTargetHeader".
 */
export type ScheduleTargetHeader =
  | {
      type: 'conversation_inbound';
    }
  | {
      workflow_id: WorkflowId;
      type: 'workflow';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleSpec".
 */
export type ScheduleSpec =
  | {
      at_utc: string;
      type: 'at';
    }
  | {
      anchor_utc: string;
      every_seconds: number;
      type: 'interval';
    }
  | {
      expression: string;
      time_zone: string;
      type: 'cron';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleState".
 */
export type ScheduleState = 'enabled' | 'disabled' | 'completed';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduledTargetReceipt".
 */
export type ScheduledTargetReceipt =
  | {
      message_id: MessageId;
      sequence: InboundSequence;
      type: 'conversation_inbound';
    }
  | {
      start_id: ScheduledWorkflowStartId;
      run_id: WorkflowRunId;
      type: 'workflow';
    };
/**
 * A conversation-scoped inbound sequence number.
 *
 * The sequence identifies one item of the conversation's inbound ordering
 * domain. It is **not**
 * [`RuntimeEventEnvelope`](crate::events::types::RuntimeEventEnvelope)`::sequence`
 * and is never allocated from the Event Journal sequence; the durable
 * Pending Inbound Inbox owns allocation, and the first successful
 * acceptance of a conversation receives `1`.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InboundSequence".
 */
export type InboundSequence = string;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleCompletionReason".
 */
export type ScheduleCompletionReason =
  'one_shot_admitted' | 'calendar_exhausted' | 'time_range_exhausted';
/**
 * Exactly one admitted native target, never a generic callback.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleTarget".
 */
export type ScheduleTarget =
  | {
      text: string;
      type: 'conversation_inbound';
    }
  | {
      pinned: unknown;
      arguments: unknown;
      type: 'workflow';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleCommandResult".
 */
export type ScheduleCommandResult =
  | {
      receipt: ScheduleCommandReceipt;
      status: 'applied';
    }
  | {
      id: ScheduleId;
      revision: string;
      request_budget?: ScheduleRequestBudget | null;
      status: 'deleted';
    }
  | {
      current: ScheduleSnapshot;
      status: 'conflict';
    }
  | {
      status: 'not_found';
    }
  | {
      reason: string;
      status: 'invalid';
    }
  | {
      status: 'capacity';
    }
  | {
      status: 'mutation_budget_exhausted';
    };
/**
 * Terminal outcome of one auxiliary Call.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AuxiliaryCallOutcome".
 */
export type AuxiliaryCallOutcome = 'completed' | 'failed' | 'cancelled';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ResultAvailability".
 */
export type ResultAvailability = 'pending' | 'retained' | 'expired' | 'discarded' | 'unavailable';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConsumerResult".
 */
export type ConsumerResult =
  | {
      label: string;
      kind: 'classification';
    }
  | {
      text: string;
      kind: 'btw';
    }
  | {
      reason: ConsumerFailure;
      kind: 'rejected';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConsumerFailure".
 */
export type ConsumerFailure =
  | 'invalid_output'
  | 'refused'
  | 'output_too_large'
  | 'input_too_large'
  | 'execution_failed'
  | 'cancelled'
  | 'unavailable';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AuxiliarySubmissionLookup".
 */
export type AuxiliarySubmissionLookup =
  | {
      receipt: AuxiliarySubmissionReceipt;
      state: 'known';
    }
  | {
      scope: AuxiliaryAttribution;
      call_id?: AuxiliaryCallId | null;
      state: 'pending';
    }
  | {
      scope: AuxiliaryAttribution;
      state: 'confirmed_not_admitted';
    }
  | {
      state: 'unresolved';
    };
/**
 * Typed pending mutation disposition; uncertainty requires authoritative reread.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "PendingMutationOutcome".
 */
export type PendingMutationOutcome =
  | {
      status: 'durability_uncertain';
    }
  | {
      read_cut: ConversationReadCut1;
      status: 'applied';
    }
  | {
      status: 'not_pending';
    }
  | {
      status: 'conflict';
    }
  | {
      status: 'invalid_item';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UploadOutcome".
 */
export type UploadOutcome =
  | {
      state: 'absent';
    }
  | {
      state: 'unresolved';
    }
  | {
      state: 'failed';
    }
  | {
      files: UploadedFile[];
      state: 'ready';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ServerLifecycle".
 */
export type ServerLifecycle = 'Accepting' | 'Draining' | 'Terminated';
/**
 * Residency only; execution and interaction state remain runtime-owned.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ResidencyState".
 */
export type ResidencyState = 'Unloaded' | 'Loading' | 'Loaded' | 'Unloading';
/**
 * Inclusive source Journal position, never an activation or Ledger position.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "JournalFrontier".
 */
export type JournalFrontier = string;
/**
 * Immutable destination bootstrap installation, distinct from source activations.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "LineageInstallationId".
 */
export type LineageInstallationId = string;
/**
 * Input provenance is not execution ownership. Derived bodies have no canonical identity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceInputProvenance".
 */
export type TraceInputProvenance =
  | {
      source?: CanonicalMessageRef | null;
      conversation_id: ConversationId;
      content_digest: string;
      locator?: CanonicalContentLocator | null;
      message_id_source?: ContentBinding | null;
      type: 'original';
    }
  | {
      sources: CanonicalMessageRef[];
      checkpoint_sources: CheckpointSourceRef[];
      transformations: SemanticTransformation[];
      truncated: boolean;
      type: 'derived';
    }
  | {
      type: 'request_only';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SemanticTransformation".
 */
export type SemanticTransformation = 'summary' | 'image_downgrade' | 'upload_rendering';
/**
 * The original bounded reader that owns one deferred Trace field.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceFieldSource".
 */
export type TraceFieldSource =
  | {
      field: TraceFieldKind;
      domain: string;
      item_offset: number;
      path: string[];
      encoded_bytes: string;
      type: 'value';
    }
  | {
      field: TraceFieldKind;
      binding: ContentBinding;
      type: 'content';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceFieldKind".
 */
export type TraceFieldKind =
  | 'request_id'
  | 'provisional_message_id'
  | 'provenance'
  | 'model'
  | 'step_id'
  | 'call_id'
  | 'tool_id'
  | 'name'
  | 'arguments'
  | 'definition'
  | 'native_id'
  | 'message_id'
  | 'result'
  | 'failure';
/**
 * The role of one reconstructed request item.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceMessageRole".
 */
export type TraceMessageRole = ('user' | 'assistant' | 'tool') | 'request_only';
/**
 * The closed projected content vocabulary.
 *
 * Each variant is a semantic the browser can render with the matching
 * presentation primitive: prose as Markdown, structure as a JSON reader,
 * source as code, artifacts through the existing durable carrier.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceContentBlock".
 */
export type TraceContentBlock =
  | {
      text: TraceText;
      type: 'text';
    }
  | {
      text: TraceText;
      type: 'reasoning';
    }
  | {
      text: TraceText;
      type: 'refusal';
    }
  | {
      value: TraceJson;
      type: 'json';
    }
  | {
      call_id: ToolCallId;
      tool_id: ToolId;
      name: string;
      arguments: TraceJson;
      type: 'tool_call';
    }
  | {
      call_id: ToolCallId;
      tool_id: ToolId;
      outcome: TraceToolOutcome;
      blocks: TraceContentBlock[];
      truncated: boolean;
      type: 'tool_result';
    }
  | {
      artifact: TraceArtifact;
      alt?: string | null;
      type: 'image';
    }
  | {
      artifact: TraceArtifact;
      type: 'file';
    }
  | {
      name: string;
      type: 'upload';
    };
/**
 * The closed canonical Tool outcome vocabulary.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolOutcome".
 */
export type TraceToolOutcome =
  'success' | 'failed' | 'denied' | 'cancelled' | 'timed_out' | 'outcome_unknown';
/**
 * Immutable original field selection; caller source cut supplies read authority.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AttemptIdentitySource".
 */
export type AttemptIdentitySource =
  | {
      field: JournalFieldRef;
      domain: string;
      type: 'journal';
    }
  | {
      kind: InheritedAttemptIdentityKind;
      ordinal: number;
      domain: string;
      encoded_bytes: number;
      type: 'inherited';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InheritedAttemptIdentityKind".
 */
export type InheritedAttemptIdentityKind = 'turn' | 'response';
/**
 * The closed lifecycle vocabulary.
 *
 * `Incomplete` is the truthful answer whenever no terminal fact exists and
 * no current runtime projection positively proves activity. It is never
 * upgraded by the absence of evidence.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceState".
 */
export type TraceState =
  | 'incomplete'
  | 'running'
  | 'pending'
  | 'cancelling'
  | 'settling'
  | 'waiting'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'timed_out'
  | 'limited'
  | 'denied'
  | 'outcome_unknown'
  | 'interrupted';
/**
 * Error classes the runtime distinguishes for retry/termination decisions.
 * Provider SDK error structs never cross this boundary.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelErrorKind".
 */
export type ModelErrorKind =
  | 'invalid_request'
  | 'authentication'
  | 'rate_limit'
  | 'timeout'
  | 'transport'
  | 'provider_error'
  | 'context_window_exceeded'
  | 'cancelled'
  | 'unsupported'
  | 'malformed_tool_proposal'
  | 'generation_degenerated'
  | 'generation_budget_exceeded';
/**
 * The closed record vocabulary of the ledger.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceKind".
 */
export type TraceKind =
  | (
      | 'attempt'
      | 'step'
      | 'request'
      | 'assistant'
      | 'tool'
      | 'compaction'
      | 'background'
      | 'subagent'
      | 'workflow'
      | 'interaction'
    )
  | 'user'
  | 'context';
/**
 * How one actual request's frozen System Prompt relates to its predecessor.
 *
 * The relationship is request-relative and resolved from native durable
 * authority, never from the records a client happens to have loaded. A page
 * that begins in the middle of history therefore reports exactly what a page
 * containing the predecessor would.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceSystemPromptState".
 */
export type TraceSystemPromptState = 'initial' | 'changed' | 'unchanged' | 'previous_unavailable';
/**
 * The exact shared predecessor of the prompt and Tool catalog comparison.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceRequestPredecessor".
 */
export type TraceRequestPredecessor =
  | {
      availability: 'not_applicable';
    }
  | {
      request_id?: RequestId | null;
      request_id_source?: TraceFieldSource | null;
      availability: 'unavailable';
    }
  | {
      request_id?: RequestId | null;
      request_id_source?: TraceFieldSource | null;
      availability: 'available';
    };
/**
 * Complete frozen Tool definitions are compared before any display bounding.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolCatalogState".
 */
export type TraceToolCatalogState = 'initial' | 'changed' | 'unchanged' | 'previous_unavailable';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "NativeContextContributor".
 */
export type NativeContextContributor =
  | 'goal_status'
  | 'workspace_instructions'
  | 'skill_guidance'
  | 'agent_status'
  | 'core_system_identity'
  | 'agent_profile'
  | 'runtime_tool_observation';
/**
 * The stable logical key of one certified extension.
 *
 * Package/content attestation is intentionally not part of this type.  A
 * package may be upgraded while preserving its logical ordering identity;
 * the assembly generation records the attestation separately.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CertifiedExtensionIdentity".
 */
export type CertifiedExtensionIdentity = string;
/**
 * The closed presentation family of one admitted model-visible context fact.
 *
 * This is a presentation vocabulary, not the internal `ContextKind` payload:
 * a complete `GoalSnapshot` or Agent Status generation metadata never enters
 * a pageable summary through it.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceContextKind".
 */
export type TraceContextKind =
  | 'native_environment'
  | 'goal_status'
  | 'runtime_tool_observation'
  | 'extension_environment'
  | 'agent_status';
/**
 * Truthful admission source. Client controls have no model `ToolCall` identity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentActivationOrigin".
 */
export type AgentActivationOrigin =
  | {
      tool_call_id: ToolCallId;
      kind: 'creation_tool';
    }
  | {
      tool_call_id: ToolCallId;
      kind: 'message_tool';
    }
  | {
      kind: 'client_control';
    }
  | {
      node_id: WorkflowNodeInstance;
      kind: 'workflow';
    };
/**
 * The cursor domain of durable transcript paging.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientTranscriptCursor".
 */
export type RuntimeClientTranscriptCursor = string;
/**
 * Identifies one durable runtime event.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EventId".
 */
export type EventId = string;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TurnProcessOutcome".
 */
export type TurnProcessOutcome =
  | 'running'
  | 'incomplete_at_cut'
  | 'completed'
  | 'cancelled'
  | 'failed'
  | 'timed_out'
  | 'limit_exceeded';
/**
 * The normalized client-visible failure of one attempt.
 *
 * This is the external projection of the internal [`AttemptFailure`]
 * ([`crate::events::types::AttemptFailure`]): provider-specific fields
 * (such as the raw provider error code) never appear here.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientAttemptFailure".
 */
export type RuntimeClientAttemptFailure =
  | {
      /**
       * Error classes the runtime distinguishes for retry/termination decisions.
       * Provider SDK error structs never cross this boundary.
       */
      kind:
        | 'invalid_request'
        | 'authentication'
        | 'rate_limit'
        | 'timeout'
        | 'transport'
        | 'provider_error'
        | 'context_window_exceeded'
        | 'cancelled'
        | 'unsupported'
        | 'malformed_tool_proposal'
        | 'generation_degenerated'
        | 'generation_budget_exceeded';
      /**
       * The normalized human-readable message.
       */
      message: string;
      /**
       * The retry hint, when the provider reported one.
       */
      retry_after_ms?: string | null;
      type: 'model';
    }
  | {
      /**
       * The normalized runtime error.
       */
      error:
        | {
            /**
             * Human-readable diagnostic message.
             */
            message: string;
            type: 'internal';
          }
        | {
            /**
             * Human-readable diagnostic message.
             */
            message: string;
            type: 'invalid_state';
          }
        | {
            /**
             * Human-readable diagnostic message.
             */
            message: string;
            type: 'unsupported';
          }
        | {
            /**
             * The tool name the model called.
             */
            name: string;
            type: 'unknown_tool';
          }
        | {
            /**
             * Human-readable diagnostic message.
             */
            message: string;
            type: 'durable_store';
          }
        | {
            /**
             * Human-readable diagnostic message.
             */
            message: string;
            type: 'contract_violation';
          }
        | {
            /**
             * Human-readable diagnostic message.
             */
            message: string;
            type: 'context_preparation_failed';
          }
        | {
            /**
             * Human-readable diagnostic message.
             */
            message: string;
            type: 'context_compaction_failed';
          }
        | {
            /**
             * The policy's bounded rejection reason.
             */
            reason: string;
            type: 'pre_step_rejected';
          }
        | {
            /**
             * Human-readable diagnostic message.
             */
            message: string;
            type: 'pre_step_policy_failed';
          }
        | {
            /**
             * Human-readable diagnostic message.
             */
            message: string;
            type: 'tool_result_observation_failed';
          }
        | {
            /**
             * The bounded recovery diagnostic: which durable evidence settled
             * the attempt and what remained indeterminate.
             */
            message: string;
            type: 'restart_interrupted';
          }
        | {
            /**
             * Human-readable diagnostic message.
             */
            message: string;
            type: 'deferred_context_rejected';
          };
      type: 'runtime';
    };
/**
 * A content block inside a tool result.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolResultContent".
 */
export type ToolResultContent =
  | {
      /**
       * The text content.
       */
      text: string;
      type: 'text';
    }
  | {
      /**
       * The structured tool output value.
       */
      value: {
        [k: string]: unknown;
      };
      type: 'json';
    }
  | {
      /**
       * Identifies a durable artifact produced or referenced by the runtime.
       *
       * An artifact is identified by an opaque runtime-owned id, never by a
       * local filesystem path: paths are executor concerns and are not a
       * universal durable artifact identity.
       */
      artifact_id: string;
      /**
       * Optional display name.
       */
      name?: string | null;
      /**
       * Optional MIME type.
       */
      mime_type?: string | null;
      /**
       * Optional human-readable description.
       */
      description?: string | null;
      type: 'file';
    }
  | {
      /**
       * Identifies a durable artifact produced or referenced by the runtime.
       *
       * An artifact is identified by an opaque runtime-owned id, never by a
       * local filesystem path: paths are executor concerns and are not a
       * universal durable artifact identity.
       */
      artifact_id: string;
      /**
       * Optional short description or alt text.
       */
      alt?: string | null;
      type: 'image';
    };
/**
 * Runtime-owned managed textual-output continuation metadata of one tool
 * result (Issue #86): where the complete — or honestly partial — textual
 * output of the execution lives in the conversation's managed tool-output
 * store.
 *
 * This is rustX runtime metadata, explicitly typed and separate from
 * arbitrary tool-owned structured content (`ToolResultContent::Json`). It
 * is not a semantic artifact, not a `FileReference`, and not a File
 * modality: textual output stays textual. The locator is an advisory
 * model-facing absolute path inside the read-only managed tool-output
 * root — it is a locator, never filesystem authority.
 *
 * The two managed-output lifecycles both use this type: a foreground
 * result references its lazy result spill (`results/result_N.txt`) only
 * when the complete representation crossed the shared preview threshold,
 * while a background result references its dispatch-allocated live-output
 * file (`tasks/exec_N.output`). A size-only cutoff is never a semantic tool
 * failure; `Partial`/`Unavailable` make output-storage failure explicit.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ManagedOutputContinuation".
 */
export type ManagedOutputContinuation =
  | {
      /**
       * The absolute locator inside the managed tool-output root.
       */
      locator: string;
      type: 'complete';
    }
  | {
      /**
       * The absolute locator inside the managed tool-output root.
       */
      locator: string;
      /**
       * The output-storage failure diagnostic. Advisory only: it is
       * bounded whenever the continuation is rendered.
       */
      diagnostic: string;
      type: 'partial';
    }
  | {
      /**
       * The output-storage failure diagnostic. Advisory only: it is
       * bounded whenever the continuation is rendered.
       */
      diagnostic: string;
      type: 'unavailable';
    };
/**
 * A content block inside a `UserMessageBlock`.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UserContentBlock".
 */
export type UserContentBlock =
  | {
      batch_id: string;
      name: string;
      type: 'uploaded_file';
    }
  | {
      /**
       * The text content.
       */
      text: string;
      type: 'text';
    }
  | {
      /**
       * Identifies a durable artifact produced or referenced by the runtime.
       *
       * An artifact is identified by an opaque runtime-owned id, never by a
       * local filesystem path: paths are executor concerns and are not a
       * universal durable artifact identity.
       */
      artifact_id: string;
      /**
       * Optional short description or alt text.
       */
      alt?: string | null;
      type: 'image';
    }
  | {
      /**
       * Identifies a durable artifact produced or referenced by the runtime.
       *
       * An artifact is identified by an opaque runtime-owned id, never by a
       * local filesystem path: paths are executor concerns and are not a
       * universal durable artifact identity.
       */
      artifact_id: string;
      /**
       * Optional display name.
       */
      name?: string | null;
      /**
       * Optional MIME type.
       */
      mime_type?: string | null;
      /**
       * Optional human-readable description.
       */
      description?: string | null;
      type: 'file';
    };
/**
 * The semantic family of one admitted model-visible context fact.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ContextKind".
 */
export type ContextKind =
  | 'native_environment'
  | {
      goal_status: GoalSnapshot;
    }
  | 'runtime_tool_observation'
  | 'extension_environment'
  | {
      agent_status: AgentStatusGenerationMetadata;
    };
/**
 * The single durable Goal lifecycle authority.
 *
 * - `Active`: continuation is authorized when runtime admission is eligible.
 * - `Paused` / `Blocked`: continuation is not authorized.
 * - `Complete`: terminal.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "GoalPhase".
 */
export type GoalPhase = 'active' | 'paused' | 'blocked' | 'complete';
/**
 * Trusted origin supplied by runtime, never by model arguments.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "GoalOrigin".
 */
export type GoalOrigin =
  | {
      message_id: MessageId;
      attempt_id: AttemptId;
      kind: 'human_attempt';
    }
  | {
      kind: 'runtime_control';
    };
/**
 * The stable identity of one code-owned Agent Status module.
 *
 * This identity belongs to the canonical message layer because an active
 * Agent Status message must carry enough durable information for a later
 * Surface scan to identify the modules it contains. It is intentionally a
 * closed enum rather than extension metadata or a generic key/value field.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentStatusModuleId".
 */
export type AgentStatusModuleId = 'time' | 'background' | 'todo';
/**
 * A content block inside an `AssistantMessageBlock`.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AssistantContentBlock".
 */
export type AssistantContentBlock =
  | {
      /**
       * The text content.
       */
      text: string;
      type: 'text';
    }
  | (ReasoningBlock & {
      type: 'reasoning';
    })
  | (ToolCall & {
      type: 'tool_call';
    })
  | (RefusalBlock & {
      type: 'refusal';
    })
  | {
      /**
       * Identifies a durable artifact produced or referenced by the runtime.
       *
       * An artifact is identified by an opaque runtime-owned id, never by a
       * local filesystem path: paths are executor concerns and are not a
       * universal durable artifact identity.
       */
      artifact_id: string;
      /**
       * Optional short description or alt text.
       */
      alt?: string | null;
      type: 'image';
    };
/**
 * Provider-specific continuation state preserved by the runtime.
 *
 * `None` (absence) is the natural representation for protocols that carry no
 * continuation state, such as `OpenAI` Chat Completions, which resend the
 * full context instead of referencing a previous response.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ProviderContinuationState".
 */
export type ProviderContinuationState =
  | {
      openai_responses: OpenAiResponsesContinuation;
    }
  | {
      anthropic: AnthropicContinuation;
    };
/**
 * Continuation state for the `OpenAI` Responses protocol.
 *
 * The Responses protocol supports two continuation modes, and the canonical
 * boundary preserves either one without depending on a provider SDK:
 *
 * - [`OpenAiResponsesContinuation::Stored`]: stateful operation, continued
 *   by referencing the provider-stored previous response.
 * - [`OpenAiResponsesContinuation::Stateless`]: stateless operation
 *   (`store: false`, zero-data-retention), continued by preserving the
 *   previous response's output/reasoning items and passing them back on
 *   later requests. This includes opaque encrypted reasoning content.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "OpenAiResponsesContinuation".
 */
export type OpenAiResponsesContinuation =
  | {
      stored: {
        /**
         * The provider-assigned response id of the previous response to
         * continue.
         */
        previous_response_id: string;
      };
    }
  | {
      stateless: {
        /**
         * Ordered output/reasoning items from the previous response that
         * must be passed back on the next request, including opaque
         * encrypted reasoning content.
         */
        items: unknown[];
      };
    };
/**
 * One consolidated block of an immutable publication audit.
 *
 * Consolidation is what keeps the audit bounded: a stream that staged ten
 * thousand frames leaves one audit object whose size is the released output,
 * never O(number-of-frames) permanent staging rows.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "PublicationAuditBlock".
 */
export type PublicationAuditBlock =
  | {
      /**
       * A stable index identifying one content block within the ordered content
       * list of the canonical message being assembled.
       *
       * Streaming facts (text deltas, reasoning deltas, provider continuation
       * state, tool-call content) reference the block they belong to by this
       * index, so interleaved and multiple blocks remain unambiguous without
       * exposing any provider-specific block id type.
       */
      block_index: number;
      /**
       * The released text.
       */
      text: string;
      kind: 'text';
    }
  | {
      /**
       * A stable index identifying one content block within the ordered content
       * list of the canonical message being assembled.
       *
       * Streaming facts (text deltas, reasoning deltas, provider continuation
       * state, tool-call content) reference the block they belong to by this
       * index, so interleaved and multiple blocks remain unambiguous without
       * exposing any provider-specific block id type.
       */
      block_index: number;
      /**
       * The released reasoning text.
       */
      text: string;
      kind: 'reasoning';
    }
  | {
      /**
       * A stable index identifying one content block within the ordered content
       * list of the canonical message being assembled.
       *
       * Streaming facts (text deltas, reasoning deltas, provider continuation
       * state, tool-call content) reference the block they belong to by this
       * index, so interleaved and multiple blocks remain unambiguous without
       * exposing any provider-specific block id type.
       */
      block_index: number;
      /**
       * The released refusal text.
       */
      text: string;
      kind: 'refusal';
    }
  | {
      /**
       * A stable index identifying one content block within the ordered content
       * list of the canonical message being assembled.
       *
       * Streaming facts (text deltas, reasoning deltas, provider continuation
       * state, tool-call content) reference the block they belong to by this
       * index, so interleaved and multiple blocks remain unambiguous without
       * exposing any provider-specific block id type.
       */
      block_index: number;
      /**
       * Provider/model-issued opaque correlation string, scoped to a request/publication.
       * Not a rustX global identity: canonical ownership uses `ToolCallOccurrenceRef`.
       */
      call_id: string;
      /**
       * Identifies a tool definition in the capability set.
       */
      tool_id: string;
      /**
       * The tool name the model named.
       */
      name: string;
      /**
       * The released raw argument text, exactly as far as it was released.
       */
      arguments: string;
      /**
       * Whether the proposal finished assembling before the stream ended.
       * A partial proposal can never have been executed.
       */
      complete: boolean;
      kind: 'proposed_tool_call';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ReviewSubject".
 */
export type ReviewSubject = {
  content: unknown;
  type: 'plan';
};
/**
 * The bounded set of text shapes rustX can validate deterministically.
 *
 * Only formats rustX can prove are supported. A schema asking for a format
 * outside this set is refused by its producer rather than accepted and then
 * silently unvalidated.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TextFormat".
 */
export type TextFormat = 'date' | 'date_time' | 'uri';
/**
 * Canonical finite binary64 bits: lowercase hex, positive zero only; no NaN or infinity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "FiniteNumber".
 */
export type FiniteNumber = string;
export type ExactInteger = '0' | string;
export type ExactInteger1 = string;
/**
 * The generic liveness deadline that fired for one started execution.
 *
 * The kind is observational evidence: it records *which* intent fired, not
 * the settlement outcome. The canonical outcome is selected from the
 * executor's settlement evidence at the terminal result.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolDeadlineKind".
 */
export type ToolDeadlineKind = 'hard' | 'idle';
/**
 * The public lifecycle of one background execution.
 *
 * Terminal states are absorbing. The allowed public transitions are:
 *
 * ```text
 * Starting  → Running
 * Starting  → Cancelling
 * Starting  → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
 * Running   → Cancelling
 * Running   → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
 * Starting / Running / Cancelling → PublishingTerminal
 * Cancelling → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
 * PublishingTerminal → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
 * ```
 *
 * The terminal vocabulary claims only facts the executor proved: `Failed`
 * is a *known* failure, `Denied` is policy/approval refusal (not `Failed`),
 * `TimedOut` is a proven deadline settlement, and
 * `OutcomeUnknown` means the executor could not prove the external outcome
 * at all. A terminal lifecycle never claims a stronger external outcome
 * than the canonical [`ToolExecutionStatus`] of the stored result proves —
 * in particular an unknown outcome is never labelled `Failed`, and a
 * cancellation request that could not be confirmed physically never
 * becomes `Cancelled`.
 *
 * [`BackgroundLifecycle::PublishingTerminal`] is the honest non-terminal
 * state in which the executor has returned its terminal candidate and the
 * registry now owns durable terminal publication; it is never `Running`:
 * the executor has returned and the runner now owns the durable settlement
 * continuation, retaining the settlement candidate until publication
 * reaches a terminal outcome. An internal unpublished prepared
 * state implements dispatch atomicity but never leaks as an accepted
 * execution.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "BackgroundLifecycle".
 */
export type BackgroundLifecycle =
  | 'starting'
  | 'running'
  | 'cancelling'
  | 'publishing_terminal'
  | 'succeeded'
  | 'failed'
  | 'denied'
  | 'cancelled'
  | 'timed_out'
  | 'outcome_unknown';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentObservation".
 */
export type AgentObservation =
  | {
      agent_id: AgentId;
      conversation_id: ConversationId;
      activation_id: SubagentId;
      source: ObservationSource;
      cursor?: AgentObservationCursor | null;
      checkpoint: RuntimeClientCheckpoint;
      type: 'checkpoint';
    }
  | {
      after: AgentObservationCursor;
      cursor: AgentObservationCursor;
      events: RuntimeClientProtocolEvent[];
      type: 'events';
    }
  | {
      agent_id: AgentId;
      conversation_id: ConversationId;
      activation_id: SubagentId;
      source: ObservationSource;
      reason: string;
      type: 'resync_required';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ObservationSource".
 */
export type ObservationSource = 'live' | 'durable_only';
/**
 * Which native evidence is available for the canonical settings sections.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SettingsEvidence".
 */
export type SettingsEvidence = ('live_session' | 'frozen_child') | 'historical_partial';
/**
 * Identifies one immutable process-local runtime resource generation.
 *
 * This is deliberately separate from `ContextGeneration` (one context
 * assembly provenance set) and `CapabilityRevision` (one executable
 * capability set). A resource-only change may advance this revision while
 * retaining an identical capability revision.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeResourceRevision".
 */
export type RuntimeResourceRevision = string;
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowState".
 */
export type WorkflowState =
  | {
      type: 'pending';
    }
  | {
      type: 'running';
    }
  | {
      reason: WorkflowWait;
      type: 'waiting';
    }
  | {
      type: 'draining';
    }
  | {
      outcome: WorkflowExecutionOutcome;
      type: 'settled';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowWait".
 */
export type WorkflowWait =
  'tool' | 'agent' | 'model' | 'capacity' | 'questionnaire' | 'approval' | 'review' | 'settlement';
/**
 * Bounded block/node observation; live state stays in the executor.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowExecutionOutcome".
 */
export type WorkflowExecutionOutcome =
  'completed' | 'failed' | 'cancelled' | 'denied' | 'timed_out' | 'outcome_unknown';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowNodeKind".
 */
export type WorkflowNodeKind =
  'block' | 'agent' | 'model' | 'tool' | 'branch' | 'parallel' | 'review' | 'loop' | 'return';
/**
 * Normal finite Loop completion; neither case claims business verification passed.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowLoopExit".
 */
export type WorkflowLoopExit = 'satisfied' | 'exhausted';
/**
 * The canonical conversation message.
 *
 * The `role` discriminator is stable: `user`, `assistant`, `tool`.
 * No additional top-level role exists.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "MessageBlock".
 */
export type MessageBlock =
  | (UserMessageBlock & {
      role: 'user';
    })
  | (AssistantMessageBlock & {
      role: 'assistant';
    })
  | (ToolMessageBlock & {
      role: 'tool';
    });
/**
 * The audit settlement kinds — the two terminal settlements that are not
 * canonical conversation acceptance.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "PublicationAuditKind".
 */
export type PublicationAuditKind = 'unaccepted' | 'incomplete';
/**
 * One ordered block of an in-flight Assistant message.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InFlightBlock".
 */
export type InFlightBlock =
  | {
      /**
       * A stable index identifying one content block within the ordered content
       * list of the canonical message being assembled.
       *
       * Streaming facts (text deltas, reasoning deltas, provider continuation
       * state, tool-call content) reference the block they belong to by this
       * index, so interleaved and multiple blocks remain unambiguous without
       * exposing any provider-specific block id type.
       */
      block_index: number;
      /**
       * The accumulated text.
       */
      text: string;
      type: 'text';
    }
  | {
      /**
       * A stable index identifying one content block within the ordered content
       * list of the canonical message being assembled.
       *
       * Streaming facts (text deltas, reasoning deltas, provider continuation
       * state, tool-call content) reference the block they belong to by this
       * index, so interleaved and multiple blocks remain unambiguous without
       * exposing any provider-specific block id type.
       */
      block_index: number;
      /**
       * The accumulated reasoning text.
       */
      text: string;
      type: 'reasoning';
    }
  | {
      /**
       * A stable index identifying one content block within the ordered content
       * list of the canonical message being assembled.
       *
       * Streaming facts (text deltas, reasoning deltas, provider continuation
       * state, tool-call content) reference the block they belong to by this
       * index, so interleaved and multiple blocks remain unambiguous without
       * exposing any provider-specific block id type.
       */
      block_index: number;
      /**
       * The accumulated refusal text.
       */
      text: string;
      type: 'refusal';
    }
  | {
      /**
       * A stable index identifying one content block within the ordered content
       * list of the canonical message being assembled.
       *
       * Streaming facts (text deltas, reasoning deltas, provider continuation
       * state, tool-call content) reference the block they belong to by this
       * index, so interleaved and multiple blocks remain unambiguous without
       * exposing any provider-specific block id type.
       */
      block_index: number;
      /**
       * The tool-call identity.
       */
      call_id?: ToolCallId | null;
      metadata_ref?: ContentRef | null;
      /**
       * The canonical tool identity.
       */
      tool_id?: ToolId | null;
      /**
       * The model-facing tool name.
       */
      name: string;
      /**
       * The accumulated JSON argument fragments.
       */
      arguments: string;
      type: 'tool_call';
    };
/**
 * Presentation metadata for the root interaction surface.
 *
 * This is projection data only. It grants no authority to the root router,
 * and it is never copied into either conversation's canonical history.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InteractionSource".
 */
export type InteractionSource =
  | {
      type: 'primary';
    }
  | {
      /**
       * Identifies one finite activation owned by an Agent, or one finite
       * Workflow child execution.
       *
       * Repeated native Agent activations have distinct `SubagentId`s while
       * retaining the same durable `AgentId`. This is not an OS pid: process
       * identity or PID reuse can never establish durable ownership or proof
       * of physical settlement.
       */
      subagent_id: string;
      /**
       * The child conversation that owns the interaction.
       */
      child_conversation_id: string;
      /**
       * The canonical typed name of one admitted subagent definition.
       *
       * The keyspace is deliberately narrow: lowercase ASCII letters, digits,
       * `-`, and `_`, starting with a letter. A name is the model-facing routing
       * token, the durable ownership identity, and the Runtime Client projection
       * identity, so an ambiguous or shell-shaped spelling is rejected at the
       * configuration boundary rather than normalized later.
       */
      agent_name: string;
      type: 'subagent';
    };
/**
 * Identifies one process-owned native human interaction.
 *
 * Interaction identities are derived from the conversation runtime's
 * non-reused [`AttemptId`] domain plus an ordinal owned by the
 * interaction coordinator.  They are deliberately not allocated from
 * a process-local counter alone: a response retained by a client from a
 * crashed process can therefore never name a different interaction in a
 * restarted runtime.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InteractionId".
 */
export type InteractionId = string;
/**
 * One truthful native interaction owner. Scheduled work has no model turn.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InteractionExecution".
 */
export type InteractionExecution =
  | {
      attempt_id: AttemptId;
      turn: number;
      type: 'attempt';
    }
  | {
      run: WorkflowRunId;
      type: 'scheduled';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientInteractionKind".
 */
export type RuntimeClientInteractionKind = 'approval' | 'questionnaire' | 'review';
/**
 * The resolved execution ownership of one canonical invocation.
 *
 * The runtime resolves the effective mode from the tool's declared
 * [`ToolExecutionPolicy`] and (for `ModelSelectable`) the required
 * `execution_mode` invocation field, and delivers it to the executor as
 * canonical invocation data. The field is consumed before business-argument
 * validation and never reaches an executor. No policy resolution happens
 * inside executors.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolInvocationMode".
 */
export type ToolInvocationMode = 'foreground' | 'background';
/**
 * The public lifecycle vocabulary of one subagent snapshot.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SubagentState".
 */
export type SubagentState =
  | 'stopping'
  | 'running'
  | 'cancelling'
  | 'publishing_terminal'
  | 'succeeded'
  | 'failed'
  | 'cancelled'
  | 'interrupted';
/**
 * One structured Agent Status section of the external view.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientStatusSection".
 */
export type RuntimeClientStatusSection =
  | {
      /**
       * The runtime clock value sampled at composition time.
       */
      current_time: string;
      /**
       * The Time status timezone, when configured.
       */
      timezone?: string | null;
      type: 'temporal';
    }
  | {
      /**
       * The active background executions in allocation order.
       */
      executions: RuntimeClientStatusBackgroundExecution[];
      /**
       * Active executions omitted by the module-local bound.
       */
      omitted_count: number;
      type: 'background_executions';
    }
  | {
      /**
       * The first committed in-progress task, when any.
       */
      current?: RuntimeClientTodoStatusTask | null;
      /**
       * Remaining committed active tasks in creation order.
       */
      tasks?: RuntimeClientTodoStatusTask[];
      /**
       * Number of committed active tasks.
       */
      active_count: number;
      /**
       * Number of active tasks blocked by active dependencies.
       */
      blocked_count: number;
      /**
       * Number of committed completed tasks.
       */
      completed_count: number;
      /**
       * Number of committed deleted tasks.
       */
      deleted_count: number;
      /**
       * Number of active tasks omitted from the bounded view.
       */
      omitted_count: number;
      type: 'todo';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ResourceFamily".
 */
export type ResourceFamily = 'agent' | 'workflow' | 'managed_python' | 'mcp' | 'skill';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceScope".
 */
export type SourceScope = 'user' | 'workspace';
/**
 * What one resource diagnostic belongs to.
 *
 * Attribution is a native fact, decided where the identity is known: the
 * catalog entry keyed by that identity. It is never derived from a field path
 * or from a source file several identities share, so a client never has to
 * guess which resource a diagnostic is about.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ResourceDiagnosticSubject".
 */
export type ResourceDiagnosticSubject =
  | {
      family: ResourceFamily;
      name: string;
      kind: 'resource';
    }
  | {
      family: ResourceFamily;
      kind: 'collection';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentIdentity".
 */
export type AgentIdentity =
  | {
      kind: 'main';
    }
  | {
      kind: 'named';
      name: SubagentName;
    };
/**
 * Where a tool comes from.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolOrigin".
 */
export type ToolOrigin =
  | 'builtin'
  | {
      mcp: {
        server_id: McpServerId;
      };
    }
  | {
      managed_python: {
        package: string;
      };
    };
/**
 * Agent admission/projection intent, lowered from `ToolSelectionDocument`.
 * This is never a Workflow Tool leaf or a frozen child executable identity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentToolSelection".
 */
export type AgentToolSelection =
  | {
      name: string;
      origin: 'builtin';
    }
  | {
      source_id: ToolSourceId;
      name: string;
      origin: 'source';
    }
  | {
      source_id: ToolSourceId;
      origin: 'all';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "NativeExtension".
 */
export type NativeExtension = 'agent_status' | 'todo' | 'goal';
/**
 * Typed native facts retained with the generation, never emitted per model turn.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentProfileDiagnostic".
 */
export type AgentProfileDiagnostic =
  | {
      kind: 'host_tool_suppressed';
      detail: {
        id: ToolId;
        name: string;
        source?: ToolSourceId | null;
      };
    }
  | {
      kind: 'tool';
      detail: ToolSelectionError;
    }
  | {
      kind: 'skill_unavailable';
      detail: {
        name: string;
      };
    }
  | {
      kind: 'agent_unavailable';
      detail: {
        name: SubagentName;
      };
    }
  | {
      kind: 'workflow_unavailable';
      detail: {
        id: WorkflowId;
      };
    }
  | {
      kind: 'scope_unsupported';
      detail: {
        capability: ScopeCapability;
      };
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolSelectionError".
 */
export type ToolSelectionError =
  | {
      selector: string;
      source: ToolSourceId;
      reason: SourceResolutionFailure;
      kind: 'source_unavailable';
    }
  | {
      source: ToolSourceId;
      name: string;
      kind: 'exact_tool_absent';
    }
  | {
      selector: string;
      kind: 'unknown_capability';
    };
/**
 * Typed facts for admission owners; they choose their own failure policy.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceResolutionFailure".
 */
export type SourceResolutionFailure =
  | {
      kind: 'undefined';
    }
  | {
      kind: 'unprepared';
    }
  | {
      kind: 'unavailable';
      detail: {};
    };
/**
 * Closed scope-ineligible capability identities, requiring no string parsing.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScopeCapability".
 */
export type ScopeCapability =
  | {
      kind: 'agent';
      identity: SubagentName;
    }
  | {
      kind: 'workflow';
      identity: WorkflowId;
    }
  | {
      kind: 'goal';
    }
  | {
      kind: 'builtin_tool';
      identity: ToolId;
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowInspection".
 */
export type WorkflowInspection =
  | {
      status: 'enabled';
    }
  | {
      status: 'disabled';
      diagnostics: WorkflowAdmissionDiagnostic[];
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowDependencyFailure".
 */
export type WorkflowDependencyFailure =
  | {
      kind: 'not_admitted';
    }
  | {
      kind: 'model';
      detail: {
        reason: string;
      };
    }
  | {
      kind: 'materialization';
      detail: {};
    }
  | {
      kind: 'agent';
      detail: AgentProfileDiagnostic;
    }
  | {
      kind: 'tool';
      detail: ToolSelectionError;
    }
  | {
      kind: 'ineligible_tool';
      detail: ExactToolSelector;
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceInspection".
 */
export type SourceInspection =
  | {
      status: 'unprepared';
    }
  | {
      status: 'ready';
    }
  | {
      status: 'unavailable';
    };
/**
 * One typed generation-scoped Skill discovery fact.
 *
 * The variant declaration order, followed by the field order, **is** the
 * canonical diagnostic order: the derived [`Ord`] is the only sort key, so
 * no consumer can observe a filesystem-enumeration-dependent ordering.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SkillDiagnostic".
 */
export type SkillDiagnostic =
  | {
      /**
       * The source whose root is absent.
       */
      source: 'user' | 'workspace';
      /**
       * The resolved root path.
       */
      root: string;
      kind: 'source_root_missing';
    }
  | {
      /**
       * The source whose root is unusable.
       */
      source: 'user' | 'workspace';
      /**
       * The resolved root path.
       */
      root: string;
      kind: 'source_root_invalid';
    }
  | {
      /**
       * The source whose cumulative budget was exhausted.
       */
      source: 'user' | 'workspace';
      /**
       * The fixed collection root, represented in canonical order.
       */
      roots: string[];
      /**
       * The source's total candidate count.
       */
      candidates: number;
      /**
       * The source's cumulative candidate budget.
       */
      limit: number;
      kind: 'source_budget_exceeded';
    }
  | {
      /**
       * The source the candidate was found under.
       */
      source: 'user' | 'workspace';
      /**
       * The candidate package directory.
       */
      package: string;
      /**
       * The preserved typed validation cause.
       */
      cause:
        | {
            directory: string;
            cause: 'invalid_name';
          }
        | {
            directory: string;
            cause: 'name_directory_mismatch';
          }
        | {
            directory: string;
            cause: 'missing_skill_markdown';
          }
        | {
            directory: string;
            cause: 'skill_markdown_not_regular_file';
          }
        | {
            directory: string;
            cause: 'malformed_frontmatter';
          }
        | {
            directory: string;
            cause: 'invalid_description';
          }
        | {
            directory: string;
            cause: 'invalid_compatibility';
          }
        | {
            directory: string;
            cause: 'malformed_metadata';
          }
        | {
            directory: string;
            cause: 'invalid_dependency_declaration';
          }
        | {
            path: string;
            cause: 'unsupported_symlink';
          }
        | {
            path: string;
            cause: 'unrepresentable_root';
          }
        | {
            path: string;
            cause: 'io';
          };
      kind: 'package_invalid';
    }
  | {
      /**
       * The source that offered the candidate.
       */
      source: 'user' | 'workspace';
      /**
       * The candidate package directory as offered.
       */
      package: string;
      /**
       * The canonical source root the candidate left.
       */
      root: string;
      kind: 'package_escapes_source';
    }
  | {
      /**
       * The scope that defines the identity more than once.
       */
      source: 'user' | 'workspace';
      /**
       * The contested logical Skill identity.
       */
      name: string;
      /**
       * Every conflicting package root, in canonical order.
       */
      packages: string[];
      kind: 'duplicate_identity';
    }
  | {
      /**
       * The contested logical Skill identity.
       */
      name: string;
      /**
       * The source that owns the effective package.
       */
      effective_source: 'user' | 'workspace';
      /**
       * The effective package's `SKILL.md` location.
       */
      effective_location: string;
      /**
       * The source whose package was shadowed.
       */
      shadowed_source: 'user' | 'workspace';
      /**
       * The shadowed package's `SKILL.md` location.
       */
      shadowed_location: string;
      kind: 'shadowed';
    };
/**
 * The redacted client-facing projection of an auxiliary policy.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AuxiliaryModelView".
 */
export type AuxiliaryModelView =
  | {
      mode: 'session';
    }
  | {
      /**
       * An authored Model identity. Its spelling has no provider or wire semantics.
       */
      model: string;
      /**
       * The model interaction protocol an adapter must speak.
       */
      protocol: 'openai_chat_completions' | 'openai_responses' | 'anthropic_messages';
      /**
       * The model's context window in tokens.
       */
      contextWindow: number;
      /**
       * The model's configured maximum output tokens.
       */
      modelMaxOutputTokens: number;
      /**
       * The effective output budget.
       */
      maxOutputTokens: number;
      /**
       * The selected Model Profile, when the model declares any.
       */
      profile?: ModelProfileId | null;
      /**
       * Whether reasoning is semantically enabled.
       */
      reasoningEnabled: boolean;
      /**
       * The effective opaque provider request parameters.
       */
      requestParams?: {
        [k: string]: unknown;
      };
      capabilities: ModelCapabilities2;
      declaredCapabilities: ModelCapabilities3;
      mode: 'explicit';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientGoalOrigin".
 */
export type RuntimeClientGoalOrigin =
  | {
      message_id?: MessageId | null;
      attempt_id?: AttemptId | null;
      kind: 'human_attempt';
    }
  | {
      kind: 'runtime_control';
    };
/**
 * One externally visible Runtime Client observation.
 *
 * The stable `type` discriminator is the protocol contract; unknown
 * fields are rejected.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientEvent".
 */
export type RuntimeClientEvent =
  | {
      events: RuntimeClientEvent[];
      metadata: ProjectionMetadata;
      type: 'projection_transition';
    }
  | {
      transition: string;
      checkpoint: RuntimeClientCheckpoint;
      type: 'checkpoint_advanced';
    }
  | {
      position: number;
      attempt_id: AttemptId;
      message_id: MessageId;
      block_index: ContentBlockIndex;
      kind: string;
      base_length: string;
      end_length: string;
      content_revision: string;
      inline_delta?: string | null;
      content_ref: ContentRef;
      type: 'block_content_advanced';
    }
  | {
      projection_head?: RuntimeClientProjectionHeadView | null;
      transcript: RuntimeClientTranscriptPage;
      trace: TracePage;
      occupancy?: RuntimeClientContextOccupancy | null;
      todos?: TodoSnapshot | null;
      type: 'read_domains_updated';
    }
  | {
      type: 'trace_changed';
    }
  | {
      eligibility: AdoptionEligibility;
      type: 'configuration_adoption_eligibility_changed';
    }
  | {
      view: RuntimeClientGoalView;
      type: 'goal_changed';
    }
  | {
      workflows: WorkflowSnapshot1;
      type: 'workflows_updated';
    }
  | {
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      /**
       * The immutable model snapshot the attempt froze at admission.
       */
      model?: AttemptModelView | null;
      /**
       * Native frozen admission evidence, absent when unavailable.
       */
      execution_settings?: AdmittedSettings | null;
      type: 'attempt_started';
    }
  | {
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      /**
       * The platform-level settlement.
       */
      outcome:
        | {
            /**
             * The normalized finish reason.
             */
            finish_reason:
              | {
                  type: 'stop';
                }
              | {
                  type: 'tool_calls';
                }
              | {
                  type: 'length';
                }
              | {
                  type: 'content_filter';
                }
              | {
                  type: 'refusal';
                }
              | {
                  /**
                   * The original provider reason, preserved for diagnostics.
                   */
                  reason: string;
                  type: 'other';
                };
            /**
             * The original reason exceeds its finite inline preview.
             */
            finish_reason_deferred?: boolean;
            /**
             * Exact allowlisted reason bytes at the published native cut.
             */
            finish_reason_source?: JournalFieldRef | null;
            type: 'completed';
          }
        | {
            /**
             * Why the attempt was cancelled.
             */
            reason:
              | 'user_requested'
              | 'runtime_shutdown'
              | 'parent_cancelled'
              | 'subagent_execution_deadline_exceeded';
            type: 'cancelled';
          }
        | {
            type: 'timed_out';
          }
        | {
            /**
             * Which limit was exceeded.
             */
            limit: 'max_turns' | 'max_tool_calls' | 'max_runtime_seconds';
            type: 'limit_exceeded';
          }
        | {
            /**
             * The normalized client-visible failure of one attempt.
             *
             * This is the external projection of the internal [`AttemptFailure`]
             * ([`crate::events::types::AttemptFailure`]): provider-specific fields
             * (such as the raw provider error code) never appear here.
             */
            error:
              | {
                  /**
                   * Error classes the runtime distinguishes for retry/termination decisions.
                   * Provider SDK error structs never cross this boundary.
                   */
                  kind:
                    | 'invalid_request'
                    | 'authentication'
                    | 'rate_limit'
                    | 'timeout'
                    | 'transport'
                    | 'provider_error'
                    | 'context_window_exceeded'
                    | 'cancelled'
                    | 'unsupported'
                    | 'malformed_tool_proposal'
                    | 'generation_degenerated'
                    | 'generation_budget_exceeded';
                  /**
                   * The normalized human-readable message.
                   */
                  message: string;
                  /**
                   * The retry hint, when the provider reported one.
                   */
                  retry_after_ms?: string | null;
                  type: 'model';
                }
              | {
                  /**
                   * The normalized runtime error.
                   */
                  error:
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'internal';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'invalid_state';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'unsupported';
                      }
                    | {
                        /**
                         * The tool name the model called.
                         */
                        name: string;
                        type: 'unknown_tool';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'durable_store';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'contract_violation';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'context_preparation_failed';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'context_compaction_failed';
                      }
                    | {
                        /**
                         * The policy's bounded rejection reason.
                         */
                        reason: string;
                        type: 'pre_step_rejected';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'pre_step_policy_failed';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'tool_result_observation_failed';
                      }
                    | {
                        /**
                         * The bounded recovery diagnostic: which durable evidence settled
                         * the attempt and what remained indeterminate.
                         */
                        message: string;
                        type: 'restart_interrupted';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'deferred_context_rejected';
                      };
                  type: 'runtime';
                };
            /**
             * The normalized diagnostic exceeds its finite inline preview.
             */
            error_deferred?: boolean;
            /**
             * Normalized diagnostic source; never a raw provider error.
             */
            error_source?: JournalFieldRef | null;
            type: 'failed';
          };
      type: 'attempt_settled';
    }
  | {
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      /**
       * The exact folded number of completed turns.
       */
      turn: number;
      type: 'attempt_turn_updated';
    }
  | {
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      usage: ModelUsage1;
      type: 'attempt_usage_updated';
    }
  | {
      owner: string;
      cut: ObservationCollectionCut;
      head: RuntimeClientInteractionHeader[];
      type: 'pending_head_changed';
    }
  | {
      interaction: RuntimeClientInteractionHeader1;
      type: 'interaction_pending';
    }
  | {
      interaction: InteractionRef1;
      /**
       * The exact terminal rendezvous outcome.
       */
      outcome:
        | {
            kind: ToolDeadlineKind;
            type: 'deadline_expired';
          }
        | {
            /**
             * A typed response to one native interaction.
             */
            response:
              | {
                  response: ReviewResponse;
                  type: 'review';
                }
              | {
                  /**
                   * The finite approval decision.  It has no tool arguments.
                   */
                  decision:
                    | {
                        type: 'allow';
                      }
                    | {
                        /**
                         * A bounded client-facing reason.
                         */
                        reason: string;
                        type: 'deny';
                      };
                  type: 'approval';
                }
              | {
                  /**
                   * A submitted answer set or explicit decline.
                   */
                  response:
                    | {
                        type: 'submitted';
                        value: QuestionnaireSubmission;
                      }
                    | {
                        type: 'declined';
                      };
                  type: 'questionnaire';
                };
            type: 'responded';
          }
        | {
            /**
             * The first-winner cancellation cause from the owning attempt.
             */
            reason:
              | 'user_requested'
              | 'runtime_shutdown'
              | 'parent_cancelled'
              | 'subagent_execution_deadline_exceeded';
            type: 'cancelled';
          };
      type: 'interaction_settled';
    }
  | {
      interaction: InteractionRef2;
      type: 'interaction_removed';
    }
  | {
      audit: RuntimeClientTranscriptInteractionRequested;
      /**
       * The cursor domain of durable transcript paging.
       */
      transcript_cursor: string;
      type: 'interaction_audit_requested';
    }
  | {
      audit: RuntimeClientTranscriptInteractionSettled;
      /**
       * The cursor domain of durable transcript paging.
       */
      transcript_cursor: string;
      type: 'interaction_audit_settled';
    }
  | {
      context: RuntimeClientContextView1;
      /**
       * The owning attempt for automatic compaction; absent for manual
       * idle maintenance.
       */
      attempt_id?: AttemptId | null;
      type: 'context_compaction_started';
    }
  | {
      context: RuntimeClientContextView2;
      /**
       * The owning attempt for automatic compaction; absent for manual
       * idle maintenance.
       */
      attempt_id?: AttemptId | null;
      /**
       * The runtime-owned failure diagnostic.
       */
      error: string;
      type: 'context_compaction_failed';
    }
  | {
      /**
       * The owning attempt for automatic compaction; absent for manual
       * idle maintenance.
       */
      attempt_id?: AttemptId | null;
      context: RuntimeClientContextView3;
      type: 'context_compacted';
    }
  | {
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      /**
       * Identifies a committed canonical message block.
       */
      message_id: string;
      type: 'assistant_message_started';
    }
  | {
      position: number;
      foreground_position: number;
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      /**
       * Identifies a committed canonical message block.
       */
      message_id: string;
      /**
       * A stable index identifying one content block within the ordered content
       * list of the canonical message being assembled.
       *
       * Streaming facts (text deltas, reasoning deltas, provider continuation
       * state, tool-call content) reference the block they belong to by this
       * index, so interleaved and multiple blocks remain unambiguous without
       * exposing any provider-specific block id type.
       */
      block_index: number;
      call: RuntimeClientToolCall;
      type: 'tool_call_started';
    }
  | {
      position: number;
      foreground_position: number;
      /**
       * Exact native assembled representation; clients must not reserialize JSON numbers.
       */
      arguments_json: string;
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      /**
       * Identifies a committed canonical message block.
       */
      message_id: string;
      /**
       * A stable index identifying one content block within the ordered content
       * list of the canonical message being assembled.
       *
       * Streaming facts (text deltas, reasoning deltas, provider continuation
       * state, tool-call content) reference the block they belong to by this
       * index, so interleaved and multiple blocks remain unambiguous without
       * exposing any provider-specific block id type.
       */
      block_index: number;
      call: RuntimeClientToolCall1;
      arguments_ref?: ContentRef | null;
      type: 'tool_call_assembled';
    }
  | {
      settlement_head: RuntimeClientPublicationSettlementRecord[];
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      audit: PublicationAudit1;
      /**
       * The cursor domain of durable transcript paging.
       */
      transcript_cursor: string;
      type: 'assistant_publication_settled';
    }
  | {
      settlement_head: RuntimeClientPublicationSettlementRecord[];
      attempt_id: AttemptId;
      settlement: RuntimeClientPublicationSettlement;
      /**
       * Bounded retained prefix, never a claim that deferred content is empty.
       */
      audit?: PublicationAudit2 | null;
      transcript_cursor: RuntimeClientTranscriptCursor;
      read_cut: ObservationReadCut1;
      content: ContentBinding[];
      collections: CollectionFrontier[];
      type: 'assistant_publication_deferred_settled';
    }
  | {
      attempt_id: AttemptId;
      previous_locator: CanonicalContentLocator;
      occurrence: RuntimeClientToolOccurrence;
      metadata_ref?: ContentRef | null;
      type: 'tool_occurrence_rebound';
    }
  | {
      attempt_id: AttemptId;
      occurrence: RuntimeClientToolOccurrence;
      result: ToolExecutionResult;
      result_deferred: boolean;
      result_metadata_ref?: ContentRef | null;
      type: 'tool_result_committed';
    }
  | {
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      occurrence?: RuntimeClientToolOccurrence | null;
      metadata_ref?: ContentRef | null;
      /**
       * The logical tool-call identity.
       */
      tool_call_id?: ToolCallId | null;
      /**
       * The canonical tool identity.
       */
      tool_id?: ToolId | null;
      type: 'tool_execution_started';
    }
  | {
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      occurrence?: RuntimeClientToolOccurrence | null;
      metadata_ref?: ContentRef | null;
      /**
       * The logical tool-call identity.
       */
      tool_call_id?: ToolCallId | null;
      /**
       * The canonical tool identity.
       */
      tool_id?: ToolId | null;
      /**
       * The detached runtime execution instance for background work;
       * `None` for foreground executions (no fake id is invented).
       */
      execution_id?: ToolExecutionId | null;
      progress: ToolProgress1;
      type: 'tool_execution_progress';
    }
  | {
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      occurrence?: RuntimeClientToolOccurrence | null;
      metadata_ref?: ContentRef | null;
      /**
       * The logical tool-call identity.
       */
      tool_call_id?: ToolCallId | null;
      /**
       * The canonical tool identity.
       */
      tool_id?: ToolId | null;
      result: ToolExecutionResult3;
      result_source?: JournalFieldRef | null;
      result_metadata_ref?: ContentRef | null;
      type: 'tool_execution_settled';
    }
  | {
      attempt_id?: AttemptId | null;
      descriptor: RuntimeClientMessageDescriptor;
      head: RuntimeClientMessageDescriptor[];
      transcript_cursor?: RuntimeClientTranscriptCursor | null;
      evicted_message_id?: MessageId | null;
      type: 'message_descriptor_committed';
    }
  | {
      /**
       * Exact display-head eviction; canonical history remains authoritative and readable.
       */
      evicted_message_id?: MessageId | null;
      /**
       * The committing attempt, when one is active.
       */
      attempt_id?: AttemptId | null;
      /**
       * The canonical conversation message.
       *
       * The `role` discriminator is stable: `user`, `assistant`, `tool`.
       * No additional top-level role exists.
       */
      message:
        | (UserMessageBlock & {
            role: 'user';
          })
        | (AssistantMessageBlock & {
            role: 'assistant';
          })
        | (ToolMessageBlock & {
            role: 'tool';
          });
      /**
       * The durable transcript position, absent for hidden Context facts.
       */
      transcript_cursor?: RuntimeClientTranscriptCursor | null;
      type: 'message_committed';
    }
  | {
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      /**
       * The turn number of the request preparation.
       */
      turn: number;
      status: AgentStatusView1;
      /**
       * The composition this admission pushed out of the bounded window,
       * when the window was already full.
       *
       * `None` when nothing left the window — including for a replayed
       * observation of a composition the window already holds, which
       * admits nothing and therefore evicts nothing. A client removes
       * exactly this identity before applying the identity-keyed
       * admission, so replay stays idempotent and can never displace an
       * unrelated composition.
       */
      evicted_status_message_id?: MessageId | null;
      evicted_status_locator?: CanonicalContentLocator | null;
      type: 'agent_status_composed';
    }
  | {
      /**
       * Complete committed pending projection, in durable sequence order.
       */
      pending: InboundItemView[];
      type: 'pending_inbound_changed';
    }
  | {
      /**
       * A conversation-scoped inbound sequence number.
       *
       * The sequence identifies one item of the conversation's inbound ordering
       * domain. It is **not**
       * [`RuntimeEventEnvelope`](crate::events::types::RuntimeEventEnvelope)`::sequence`
       * and is never allocated from the Event Journal sequence; the durable
       * Pending Inbound Inbox owns allocation, and the first successful
       * acceptance of a conversation receives `1`.
       */
      sequence: string;
      message: UserMessageBlock2;
      /**
       * The durable transcript position allocated at acceptance, absent
       * for hidden Context-kind inbound.
       */
      transcript_cursor?: RuntimeClientTranscriptCursor | null;
      type: 'inbound_enqueued';
    }
  | {
      /**
       * A conversation-scoped inbound sequence number.
       *
       * The sequence identifies one item of the conversation's inbound ordering
       * domain. It is **not**
       * [`RuntimeEventEnvelope`](crate::events::types::RuntimeEventEnvelope)`::sequence`
       * and is never allocated from the Event Journal sequence; the durable
       * Pending Inbound Inbox owns allocation, and the first successful
       * acceptance of a conversation receives `1`.
       */
      watermark: string;
      /**
       * The number of drained items.
       */
      count: number;
      /**
       * The drained message identities in inbound sequence order.
       */
      message_ids: MessageId[];
      type: 'inbound_drained';
    }
  | {
      domain: string;
      cut: ObservationCollectionCut;
      type: 'collection_changed';
    }
  | {
      position: number;
      evicted_job_id?: ToolExecutionId | null;
      job: RuntimeClientJob1;
      type: 'job_updated';
    }
  | {
      position: number;
      evicted_agent_id?: AgentId | null;
      agent: RuntimeClientAgent1;
      type: 'agent_updated';
    }
  | {
      capabilities: CapabilityView1;
      type: 'capability_updated';
    }
  | {
      plugins?: EffectivePlugins | null;
      model?: SessionModelView | null;
      model_status: RuntimeClientModelStatus;
      approval_mode: ApprovalMode;
      capabilities: CapabilityView2;
      resources: RuntimeClientResourcesView1;
      type: 'resource_generation_updated';
    }
  | {
      /**
       * The redacted session model state after the update.
       */
      model?: SessionModelView | null;
      model_status: RuntimeClientModelStatus;
      type: 'session_model_changed';
    }
  | {
      type: 'runtime_shutdown';
    }
  | {
      /**
       * The operation that failed persistently.
       */
      operation: string;
      /**
       * The human-readable failure diagnostic.
       */
      diagnostic: string;
      type: 'runtime_durability_failed';
    };
/**
 * Whether a live runtime could adopt a prepared configuration now.
 *
 * Runtime-domain advisory state, published by the runtime through its Runtime
 * Client projection; it is never part of a `ConfigurationApplication`.
 * Adoption always revalidates the native admission gate.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AdoptionEligibility".
 */
export type AdoptionEligibility =
  | {
      status: 'eligible';
    }
  | {
      status: 'busy';
    }
  | {
      status: 'unavailable';
    };
/**
 * A copied label never claims that generation ran in the destination.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionTitleProvenance".
 */
export type SessionTitleProvenance =
  | {
      conversation_id: ConversationId;
      message_id: MessageId;
      call_id: AuxiliaryCallId;
      type: 'local';
    }
  | {
      source_session: SessionId;
      type: 'inherited';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionNodeOriginObservation".
 */
export type SessionNodeOriginObservation =
  | {
      type: 'new';
    }
  | {
      source_session: SessionId;
      source_node: SessionNodeId;
      source_surface_revision: SurfaceRevision;
      type: 'clone';
    }
  | {
      source_session: SessionId;
      source_node: SessionNodeId;
      source_surface_revision: SurfaceRevision;
      source_message?: MessageId | null;
      source_message_ref?: SessionNodeValueRef | null;
      side: LineageSide;
      type: 'fork';
    };
/**
 * Bounded external outcomes shared by App Server and local presentation.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientSessionDeletionResult".
 */
export type RuntimeClientSessionDeletionResult =
  | {
      preview: RuntimeClientSessionDeletePreview;
      status: 'preview';
    }
  | {
      session_id: SessionId;
      status: 'deleted';
    }
  | {
      session_id: SessionId;
      status: 'stale';
    }
  | {
      session_id: SessionId;
      reason: RuntimeClientSessionDeletionBlocker;
      status: 'blocked';
    }
  | {
      session_id: SessionId;
      status: 'committed_cleanup_pending';
    }
  | {
      session_id: SessionId;
      status: 'committed_durability_uncertain';
    }
  | {
      session_id: SessionId;
      status: 'not_found';
    };
/**
 * Bounded safety summary. Resource identities and storage diagnostics stay native.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientSessionDeletionBlocker".
 */
export type RuntimeClientSessionDeletionBlocker =
  | {
      kind: 'resource_conflict';
    }
  | {
      kind: 'invalid_ownership';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AttachmentKind".
 */
export type AttachmentKind = 'controller';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ObservationSource2".
 */
export type ObservationSource2 = 'live';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CheckpointValueRepresentation".
 */
export type CheckpointValueRepresentation = 'utf8_text' | 'canonical_json_v1';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UnitApplication".
 */
export type UnitApplication =
  | {
      status: 'applied';
    }
  | {
      status: 'preparing';
    }
  | {
      impact: CacheImpact;
      status: 'ready';
    }
  | {
      diagnostic: string;
      status: 'failed';
    }
  | {
      status: 'process_restart';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CacheImpact".
 */
export type CacheImpact = 'preserved' | 'prefix_changed' | 'cache_namespace_changed' | 'unproven';
/**
 * The redacted client-facing description of a credential source.
 *
 * This is safe to place in Runtime Client results: it carries the source
 * *kind* and, for an environment reference, the variable *name*. It never
 * carries a credential value.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CredentialSourceView".
 */
export type CredentialSourceView =
  | {
      type: 'literal';
    }
  | {
      /**
       * The environment variable name (never its value).
       */
      variable: string;
      type: 'environment';
    };
/**
 * Values are never included: provenance cannot expose credentials or environment values.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "Origin".
 */
export type Origin =
  | {
      kind: 'builtin';
    }
  | {
      document: string;
      base: string;
      kind: 'user';
    }
  | {
      document: string;
      base: string;
      kind: 'workspace';
    }
  | {
      base: string;
      kind: 'process';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "McpProbeOutcome".
 */
export type McpProbeOutcome =
  | 'reachable'
  | 'connection_failed'
  | 'list_failed'
  | 'timed_out'
  | 'cancelled'
  | 'settlement_failed';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ProcessPolicyImpact".
 */
export type ProcessPolicyImpact = 'hot' | 'restart';
/**
 * Whether native can bind a Session in a Workspace, and its model catalog.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionModelsView".
 */
export type SessionModelsView =
  | {
      catalog: ModelCatalogView;
      default_model: SessionModelConfig;
      kind: 'available';
    }
  | {
      diagnostic: string;
      kind: 'unavailable';
    };
/**
 * The outcome of one automatic repair observation. A source that never
 * needed repair has no receipt — nor has one whose receipt was withdrawn or
 * forgotten, so having none never proves a source valid.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConfigRepairStatus".
 */
export type ConfigRepairStatus = 'repaired' | 'unresolved' | 'commit_uncertain';
/**
 * One deterministic edit of an authored source, in native vocabulary only:
 * never an authored key, identity or value.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConfigRepairChange".
 */
export type ConfigRepairChange =
  | {
      /**
       * The closed schema object the field was removed from: schema
       * property names, `*` for an open-map identity and `[n]` for a
       * zero-based array position; empty for the document root.
       */
      object_path: string;
      kind: 'removed_field';
    }
  | {
      /**
       * The element's zero-based position in the authored selection.
       */
      position: number;
      /**
       * The native Tool identity the element names; null when `reason`
       * is `unknown`, since a name no native Tool has is authored text.
       */
      name?: string | null;
      reason: NativeToolRemoval;
      kind: 'removed_native_tool';
    }
  | {
      from: number;
      to: number;
      kind: 'updated_schema_version';
    };
/**
 * Why a Native Tool selection entry was removed.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "NativeToolRemoval".
 */
export type NativeToolRemoval = 'unknown' | 'duplicate' | 'agent_control' | 'plugin_owned';
/**
 * At most 32 identities of at most 256 UTF-8 bytes in one exact source cut.
 * Continuation checks `revision`; a replaced source never silently retargets.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowModelChoices".
 */
export type WorkflowModelChoices =
  | {
      revision: string;
      models: ModelRef[];
      next_offset?: number | null;
      type: 'models';
    }
  | {
      revision: string;
      model: ModelRef;
      profiles: ModelProfileId[];
      next_offset?: number | null;
      type: 'profiles';
    };
/**
 * Whether the native compiler admits the candidate's structure.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowStructure".
 */
export type WorkflowStructure =
  | {
      outputs: {
        [k: string]: unknown;
      };
      revision: string;
      status: 'valid';
    }
  | {
      diagnostic: WorkflowDiagnostic;
      status: 'invalid';
    };
/**
 * Dependency admission of a structurally valid candidate in one explicit
 * Workspace composition.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowDependencies".
 */
export type WorkflowDependencies =
  | {
      status: 'not_checked';
    }
  | {
      context: string;
      status: 'admitted';
    }
  | {
      context: string;
      model_nodes: string[];
      status: 'requires_attempt';
    }
  | {
      context: string;
      diagnostics: WorkflowAdmissionDiagnostic[];
      status: 'blocked';
    }
  | {
      context: string;
      reason: string;
      status: 'unavailable';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelPolicyInspection".
 */
export type ModelPolicyInspection =
  | {
      mode: 'session';
    }
  | {
      model: ModelRef;
      profile?: ModelProfileId | null;
      max_output_tokens?: number | null;
      mode: 'explicit';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ErrorData".
 */
export type ErrorData =
  | {
      reason: AuxiliaryFailure;
      kind: 'auxiliary';
    }
  | {
      reason: SessionFileReadFailure;
      kind: 'session_file_read';
    }
  | {
      rejection: AdoptionError;
      kind: 'configuration_adoption';
    }
  | {
      diagnostic: string;
      kind: 'model_unavailable';
    }
  | {
      reason: SessionArchivePrepareError;
      kind: 'archive_preparation_failed';
    }
  | {
      job_id: ToolExecutionId;
      kind: 'job_publication_abandoned';
    }
  | {
      agent_id: AgentId;
      kind: 'agent_not_delivered';
    }
  | {
      agent_id: AgentId;
      kind: 'agent_delivery_unknown';
    }
  | {
      agent_id: AgentId;
      kind: 'agent_stopping';
    }
  | {
      agent_id: AgentId;
      kind: 'agent_settlement';
    }
  | {
      agent_id: AgentId;
      kind: 'unknown_agent';
    }
  | {
      agent_id: AgentId;
      kind: 'agent_history_unavailable';
    }
  | {
      subagent_id: SubagentId;
      kind: 'unknown_subagent';
    }
  | {
      subagent_id: SubagentId;
      kind: 'subagent_history_unavailable';
    }
  | {
      kind: 'request_capacity';
    }
  | {
      kind: 'duplicate_request_id';
    }
  | {
      diagnostic: string;
      kind: 'observation_unavailable';
    }
  | {
      kind: 'content_source_unavailable';
    }
  | {
      kind: 'invalid_range';
    }
  | {
      kind: 'read_cut_expired';
    }
  | {
      kind: 'observation_capacity_exceeded';
    }
  | {
      kind: 'observation_in_use';
    }
  | {
      kind: 'response_too_large';
    }
  | {
      kind: 'response_serialization_failed';
    }
  | {
      kind: 'outcome_unknown';
    }
  | {
      kind: 'residency_capacity';
    }
  | {
      kind: 'attachment_capacity';
    }
  | {
      kind: 'server_draining';
    }
  | {
      session_id: SessionId;
      kind: 'unknown_session';
    }
  | {
      session_id: SessionId;
      node_id: SessionNodeId;
      kind: 'unknown_node';
    }
  | {
      scope: SourceScope;
      expected: string;
      actual: string;
      kind: 'source_conflict';
    }
  | {
      expected: string;
      actual: string;
      kind: 'stale_settings';
    }
  | {
      diagnostic: WorkflowDiagnostic;
      kind: 'workflow_invalid';
    }
  | {
      interaction: InteractionRef;
      kind: 'interaction_not_pending';
    }
  | {
      interaction: InteractionRef;
      kind: 'interaction_audit_failed';
    }
  | {
      kind: 'committed_durability_uncertain';
    }
  | {
      supported: number;
      requested: number;
      kind: 'unsupported_version';
    }
  | {
      kind: 'not_initialized';
    }
  | {
      kind: 'already_initialized';
    }
  | {
      kind: 'delivery_cancelled';
    }
  | {
      kind: 'observation_cancelled';
    }
  | {
      kind: 'subscription_superseded';
    }
  | {
      kind: 'stale_attachment';
    }
  | {
      kind: 'stale_runtime';
    }
  | {
      kind: 'controller_in_use';
    }
  | {
      kind: 'invalid_state';
    }
  | {
      kind: 'invalid_params';
    }
  | {
      kind: 'resync_required';
    }
  | {
      kind: 'operation_failed';
    };
/**
 * Finite admission/read failures, independent of model and consumer outcomes.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AuxiliaryFailure".
 */
export type AuxiliaryFailure =
  'busy' | 'conflict' | 'scope_closed' | 'unavailable' | 'input_too_large';
/**
 * Closed file-access failures; diagnostics never become path authority.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionFileReadFailure".
 */
export type SessionFileReadFailure =
  | 'missing'
  | 'unauthorized'
  | 'unavailable'
  | 'not_regular'
  | 'replaced'
  | 'too_large'
  | 'capacity'
  | 'read_failed';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AdoptionError".
 */
export type AdoptionError =
  | {
      status: 'busy';
    }
  | {
      status: 'not_ready';
    }
  | {
      status: 'conflict';
    }
  | {
      diagnostic: string;
      status: 'failed';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionArchivePrepareError".
 */
export type SessionArchivePrepareError =
  | 'unknown_session'
  | 'busy'
  | 'descendant_unavailable'
  | 'conversation_unavailable'
  | 'artifact_unavailable'
  | 'corrupt_authority'
  | 'storage'
  | 'cancelled';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "Notification".
 */
export type Notification = {
  jsonrpc: JsonRpcVersion;
} & Notification1;
export type Notification1 =
  | {
      method: 'auxiliary/segment';
      params: {
        target: AttachmentTarget;
        registration_id: string;
        segment: AuxiliarySegment;
      };
    }
  | {
      method: 'session/ownershipRetired';
      params: {
        session_id: SessionId;
        /**
         * Exact decimal integer; independent of notification delivery sequence.
         */
        retired_through: string;
      };
    }
  | {
      method: 'configuration/changed';
      params: {
        application: BoundedSessionApplication;
      };
    }
  | {
      method: 'configuration/sourceRepairChanged';
      params: {
        target: SourceTarget;
        receipt?: ConfigRepairReceipt | null;
        /**
         * The native repair-ledger sequence this level is current at. Within
         * one App Server authority it orders this observation against every
         * other observation of the same source, including the
         * `repair_sequence` of a `configuration/sourcesRead`: the higher one
         * is the newer, and two at one sequence agree.
         */
        sequence: string;
      };
    }
  | {
      method: 'session/event';
      params: {
        registration_id: string;
        target: AttachmentTarget;
        cursor: RuntimeClientCursor;
        event: RuntimeClientEvent;
      };
    }
  | {
      method: 'session/observationUnavailable';
      params: {
        registration_id: string;
        target: AttachmentTarget;
        message: string;
      };
    }
  | {
      method: 'session/resyncRequired';
      params: {
        registration_id: string;
        target: AttachmentTarget;
        reason: ResyncReason;
        checkpoint_required: boolean;
        failed_at_cursor?: RuntimeClientCursor | null;
        after_cursor: RuntimeClientCursor;
        earliest_serviceable: RuntimeClientCursor;
      };
    }
  | {
      method: 'session/closed';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'session/summaryInvalidated';
      params: {
        session_id: SessionId;
        /**
         * Membership may have changed; reread the catalog and invalidate the named summary.
         * False invalidates only the named Session's display metadata.
         */
        catalog_changed: boolean;
      };
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AuxiliarySegment".
 */
export type AuxiliarySegment =
  | {
      call_id: AuxiliaryCallId;
      request_id: RequestId;
      attempt: number;
      kind: 'reset';
    }
  | {
      call_id: AuxiliaryCallId;
      request_id: RequestId;
      attempt: number;
      start: string;
      end: string;
      text: string;
      kind: 'text';
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ResyncReason".
 */
export type ResyncReason = 'replay_unavailable' | 'record_too_large';
/**
 * The rustX-owned logical identity of a context contributor.
 *
 * The identity is independent from registration order, process-local object
 * identity, and package/content generation.  The Context Assembly boundary
 * derives trusted native provenance itself; contributors never provide it.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ContextContributorIdentity".
 */
export type ContextContributorIdentity =
  | {
      Native: NativeContextContributor;
    }
  | {
      CertifiedExtension: CertifiedExtensionIdentity;
    };
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceContributionPresentation".
 */
export type TraceContributionPresentation = {
  AgentStatus: AgentStatusView;
};
/**
 * How far one Tool call progressed, by its own native evidence.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolLifecycle".
 */
export type TraceToolLifecycle = 'proposed' | 'started' | 'settled';
/**
 * The lifecycle status of one task.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TodoStatus".
 */
export type TodoStatus = 'pending' | 'in_progress' | 'completed' | 'deleted';
/**
 * The closed native inspection families. Request bodies use their existing indexed layout.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceInspectionContent".
 */
export type TraceInspectionContent =
  | {
      type: 'lifecycle';
    }
  | {
      definition: TraceToolDefinitionSelection;
      type: 'tool';
    }
  | {
      page: TraceInspectionMessages;
      type: 'messages';
    };
/**
 * Absence is proved by the original catalog; unavailable means its relationship cannot be read.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolDefinitionSelection".
 */
export type TraceToolDefinitionSelection =
  | {
      field: TraceFieldSource;
      type: 'available';
    }
  | {
      type: 'absent';
    }
  | {
      type: 'unavailable';
    };
/**
 * Provenance of inbound information.
 *
 * Provenance is metadata; it never changes the message role.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UserSource".
 */
export type UserSource =
  | 'human'
  | {
      agent: {
        /**
         * Identifies an Agent independently of its finite activations.
         * For a continuable child, this is the durable Agent-domain identity of
         * its child Conversation and remains unchanged across resume.
         */
        agent_id: string;
      };
    }
  | 'fleet'
  | 'external_system'
  | 'runtime'
  | {
      extension: {
        /**
         * The stable logical key of one certified extension.
         *
         * Package/content attestation is intentionally not part of this type.  A
         * package may be upgraded while preserving its logical ordering identity;
         * the assembly generation records the attestation separately.
         */
        contributor: string;
      };
    };
/**
 * The exact native producer of one admitted request Context fact.
 *
 * Request-scoped canonical Context has exactly one admission path — Context
 * Assembly — and that path derives exactly two provenances, so this
 * vocabulary is closed at two cases. The remaining `UserSource`
 * namespaces belong to other core-owned inbound paths and cannot reach
 * request Context; a snapshot identity that claimed one would be an
 * invariant violation, reported as such rather than widening this type.
 *
 * A certified extension keeps its **exact** logical identity. A coarse
 * namespace would make two different extensions contributing the same
 * context family indistinguishable in Trace, which is precisely the
 * question a reader asks about extension-owned context. The identity is
 * assigned by rustX during admission; a contributor never supplies it.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceContextSource".
 */
export type TraceContextSource =
  | {
      type: 'runtime';
    }
  | {
      contributor: CertifiedExtensionIdentity;
      type: 'certified_extension';
    };
/**
 * A real native execution origin. Scheduled work has no Human Attempt.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowRunOrigin".
 */
export type WorkflowRunOrigin =
  | {
      attempt_id: AttemptId;
      type: 'tool';
    }
  | {
      start_id: ScheduledWorkflowStartId;
      type: 'scheduled';
    };
/**
 * The bounded native interaction vocabulary of the 0.1 protocol.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InteractionKind".
 */
export type InteractionKind =
  | {
      review: ReviewSpecification;
      subject_digest: string;
      type: 'review';
    }
  | {
      /**
       * Caller correlation for one native invocation. Capability source is
       * independently represented by [`ToolOrigin`].
       */
      invocation_id:
        | {
            call_id: ToolCallId;
            caller: 'agent';
          }
        | {
            node: WorkflowNodeInstance;
            caller: 'workflow';
          };
      /**
       * Identifies a tool definition in the capability set.
       */
      tool_id: string;
      /**
       * The safe model-facing tool name.
       */
      tool_name: string;
      /**
       * The registry-resolved tool origin.
       */
      origin:
        | 'builtin'
        | {
            mcp: {
              server_id: McpServerId;
            };
          }
        | {
            managed_python: {
              package: string;
            };
          };
      /**
       * The resolved execution ownership of one canonical invocation.
       *
       * The runtime resolves the effective mode from the tool's declared
       * [`ToolExecutionPolicy`] and (for `ModelSelectable`) the required
       * `execution_mode` invocation field, and delivers it to the executor as
       * canonical invocation data. The field is consumed before business-argument
       * validation and never reaches an executor. No policy resolution happens
       * inside executors.
       */
      mode: 'foreground' | 'background';
      /**
       * The already validated business arguments.  This is descriptive
       * data only; it is never accepted back as replacement input.
       */
      arguments: {
        [k: string]: unknown;
      };
      /**
       * The native policy's bounded explanation for asking.
       */
      reason: string;
      type: 'approval';
    }
  | {
      invocation_id: ToolInvocationId;
      requester: InteractionRequester1;
      questionnaire: QuestionnaireSpecification1;
      type: 'questionnaire';
    };
/**
 * The exact shape of a legal answer to one question.
 *
 * This is the whole provider-independent vocabulary. It is deliberately a
 * closed set of concrete cases rather than a JSON Schema interpreter: every
 * producer must map into one of these or refuse, and every consumer can
 * render and validate all of them.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AnswerSpecification".
 */
export type AnswerSpecification =
  | {
      /**
       * The inclusive minimum Unicode scalar count, when the producer declares one.
       */
      min_length?: number | null;
      /**
       * The inclusive maximum Unicode scalar count, when the producer declares one.
       */
      max_length?: number | null;
      /**
       * The declared text shape, when the producer declares one.
       */
      format?: TextFormat | null;
      type: 'text';
    }
  | {
      /**
       * The inclusive minimum, when the producer declares one.
       */
      minimum?: FiniteNumber | null;
      /**
       * The inclusive maximum, when the producer declares one.
       */
      maximum?: FiniteNumber | null;
      type: 'number';
    }
  | {
      /**
       * The inclusive minimum, when the producer declares one.
       */
      minimum?: (ExactInteger & ExactInteger1) | null;
      /**
       * The inclusive maximum, when the producer declares one.
       */
      maximum?: (ExactInteger & ExactInteger1) | null;
      type: 'integer';
    }
  | {
      type: 'boolean';
    }
  | {
      /**
       * The finite declared options, addressed by index.
       */
      options: OptionSpecification[];
      /**
       * Whether a free-text answer outside the declared options is legal.
       */
      allow_custom: boolean;
      type: 'single_choice';
    }
  | {
      /**
       * The finite declared options, addressed by index.
       */
      options: OptionSpecification[];
      /**
       * The inclusive lower bound on how many options a submitted answer selects.
       */
      min_selected: number;
      /**
       * The inclusive upper bound on how many options a submitted answer selects.
       */
      max_selected: number;
      /**
       * Whether a free-text answer outside the declared options is legal.
       */
      allow_custom: boolean;
      type: 'multi_choice';
    };
/**
 * The finite approval decision accepted from a client.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ApprovalDecision".
 */
export type ApprovalDecision =
  | {
      type: 'allow';
    }
  | {
      /**
       * A bounded client-facing reason.
       */
      reason: string;
      type: 'deny';
    };
/**
 * The two valid terminal responses to a questionnaire.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "QuestionnaireResponse".
 */
export type QuestionnaireResponse =
  | {
      type: 'submitted';
      value: QuestionnaireSubmission;
    }
  | {
      type: 'declined';
    };
/**
 * A typed answer decision accepted from a Runtime Client.
 *
 * Which variants are legal for a given question is decided by that question's
 * [`AnswerSpecification`], not by the client.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "QuestionnaireAnswer".
 */
export type QuestionnaireAnswer =
  | {
      type: 'text';
      value: TextAnswer;
    }
  | {
      type: 'number';
      value: NumberAnswer;
    }
  | {
      type: 'integer';
      value: IntegerAnswer;
    }
  | {
      type: 'boolean';
      value: BooleanAnswer;
    }
  | {
      type: 'option';
      value: OptionAnswer;
    }
  | {
      type: 'options';
      value: OptionsAnswer;
    }
  | {
      type: 'custom';
      value: CustomAnswer;
    };
/**
 * The configurable execution-ownership policy of one tool.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ExecutionPolicyDocument".
 */
export type ExecutionPolicyDocument = 'foreground_only' | 'background_only' | 'model_selectable';
/**
 * The configurable in-batch scheduling policy of one tool.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConcurrencyPolicyDocument".
 */
export type ConcurrencyPolicyDocument = 'sequential' | 'parallel';
/**
 * The configurable HITL approval policy of one Tool.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ApprovalPolicyDocument".
 */
export type ApprovalPolicyDocument = 'never' | 'always';
/**
 * Where a [`TokenMeasurement`] came from.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TokenMeasurementSource".
 */
export type TokenMeasurementSource = 'provider_reported' | 'provider_anchored' | 'estimated';
/**
 * A normalized runtime-owned execution error.
 *
 * `RuntimeError` describes failures of the runtime itself, distinct from
 * normalized model errors (`ModelError`) and tool execution statuses.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeError".
 */
export type RuntimeError =
  | {
      /**
       * Human-readable diagnostic message.
       */
      message: string;
      type: 'internal';
    }
  | {
      /**
       * Human-readable diagnostic message.
       */
      message: string;
      type: 'invalid_state';
    }
  | {
      /**
       * Human-readable diagnostic message.
       */
      message: string;
      type: 'unsupported';
    }
  | {
      /**
       * The tool name the model called.
       */
      name: string;
      type: 'unknown_tool';
    }
  | {
      /**
       * Human-readable diagnostic message.
       */
      message: string;
      type: 'durable_store';
    }
  | {
      /**
       * Human-readable diagnostic message.
       */
      message: string;
      type: 'contract_violation';
    }
  | {
      /**
       * Human-readable diagnostic message.
       */
      message: string;
      type: 'context_preparation_failed';
    }
  | {
      /**
       * Human-readable diagnostic message.
       */
      message: string;
      type: 'context_compaction_failed';
    }
  | {
      /**
       * The policy's bounded rejection reason.
       */
      reason: string;
      type: 'pre_step_rejected';
    }
  | {
      /**
       * Human-readable diagnostic message.
       */
      message: string;
      type: 'pre_step_policy_failed';
    }
  | {
      /**
       * Human-readable diagnostic message.
       */
      message: string;
      type: 'tool_result_observation_failed';
    }
  | {
      /**
       * The bounded recovery diagnostic: which durable evidence settled
       * the attempt and what remained indeterminate.
       */
      message: string;
      type: 'restart_interrupted';
    }
  | {
      /**
       * Human-readable diagnostic message.
       */
      message: string;
      type: 'deferred_context_rejected';
    };
/**
 * Typed execution status of a tool call.
 *
 * The terminal vocabulary is explicit about **outcome certainty**: a
 * terminal status may claim only facts the owning runtime/executor can
 * prove. `Success`, `Failed`, `Denied`, `Cancelled`, and `TimedOut` are
 * confirmed terminal outcomes; `OutcomeUnknown` means an admitted call
 * crossed the external-effect frontier but rustX cannot prove its final
 * external result. An unknown outcome is never silently collapsed into
 * success, generic failure, confirmed timeout, or confirmed cancellation,
 * and every non-success status produces bounded model-facing feedback
 * through [`Self::feedback_text`].
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolExecutionStatus".
 */
export type ToolExecutionStatus =
  | {
      type: 'success';
    }
  | {
      /**
       * Human-readable error message.
       */
      error: string;
      type: 'failed';
    }
  | {
      /**
       * The policy or human-readable approval reason.
       */
      reason: string;
      type: 'denied';
    }
  | {
      /**
       * Why the execution was cancelled.
       */
      reason:
        | 'user_requested'
        | 'runtime_shutdown'
        | 'parent_cancelled'
        | 'subagent_execution_deadline_exceeded';
      /**
       * Whether cancellation won before executor start or while execution
       * was already in flight.
       */
      phase: 'before_start' | 'during_execution';
      type: 'cancelled';
    }
  | {
      type: 'timed_out';
    }
  | {
      /**
       * A producer-owned diagnostic describing why certainty is
       * unavailable. It is rendered into the bounded model-facing
       * projection and is never parsed to decide semantics; the typed
       * variant itself is the certainty claim.
       */
      detail: string;
      type: 'outcome_unknown';
    };
/**
 * Why an operation was cancelled.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CancellationReason".
 */
export type CancellationReason =
  | 'user_requested'
  | 'runtime_shutdown'
  | 'parent_cancelled'
  | 'subagent_execution_deadline_exceeded';
/**
 * The semantic phase at which a tool call was cancelled.
 *
 * This is a closed, provider-independent fact owned by the canonical tool
 * result contract and shared by foreground and background runtime-owned
 * results. The foreground Agent Loop selects it from its per-call executor
 * start frontier; the background registry selects it from its detached-runner
 * frontier. Executors may report a provisional physical cancellation status,
 * but they do not own the canonical phase classification. Clients only
 * consume or project the canonical typed fact; provider adapters likewise
 * translate that fact without inferring its phase.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolCancellationPhase".
 */
export type ToolCancellationPhase = 'before_start' | 'during_execution';
/**
 * The externally meaningful state of one foreground tool execution.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ForegroundToolState".
 */
export type ForegroundToolState =
  | {
      /**
       * The assembled JSON arguments.
       */
      arguments: string;
      type: 'assembled';
    }
  | {
      /**
       * The assembled JSON arguments.
       */
      arguments: string;
      /**
       * The latest bounded progress, when any.
       */
      progress?: ToolProgress | null;
      type: 'running';
    }
  | {
      /**
       * The assembled JSON arguments.
       */
      arguments: string;
      result: ToolExecutionResult1;
      type: 'settled';
    };
/**
 * The explicit Runtime Client transcript vocabulary.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientTranscriptItem".
 */
export type RuntimeClientTranscriptItem =
  | {
      turn: TurnProcessView;
      type: 'attempt_terminal';
    }
  | {
      /**
       * The canonical or durably accepted message.
       */
      message:
        | (UserMessageBlock & {
            role: 'user';
          })
        | (AssistantMessageBlock & {
            role: 'assistant';
          })
        | (ToolMessageBlock & {
            role: 'tool';
          });
      type: 'message';
    }
  | {
      audit: PublicationAudit;
      type: 'publication_audit';
    }
  | {
      /**
       * Identifies one durable runtime event.
       */
      event_id: string;
      /**
       * Event timestamp.
       */
      timestamp: string;
      /**
       * Owning Attempt; absent for native Scheduled work.
       */
      attempt_id?: AttemptId | null;
      /**
       * Owning turn; absent for native Scheduled work.
       */
      turn_id?: TurnId | null;
      /**
       * Interaction identity.
       */
      interaction_id: string;
      /**
       * Bounded durable subject.
       */
      subject:
        | {
            review: ReviewSpecification;
            type: 'review';
          }
        | {
            /**
             * Caller correlation for one native invocation. Capability source is
             * independently represented by [`ToolOrigin`].
             */
            invocation_id:
              | {
                  call_id: ToolCallId;
                  caller: 'agent';
                }
              | {
                  node: WorkflowNodeInstance;
                  caller: 'workflow';
                };
            /**
             * Identifies a tool definition in the capability set.
             */
            tool_id: string;
            /**
             * The model-facing tool name.
             */
            tool_name: string;
            /**
             * The digest of the canonical model-issued arguments.
             */
            arguments_digest: string;
            /**
             * The bounded policy explanation shown to the client.
             */
            reason: string;
            type: 'approval';
          }
        | {
            invocation_id: ToolInvocationId;
            requester: InteractionRequester;
            questionnaire: QuestionnaireSpecification;
            type: 'questionnaire';
          };
      type: 'interaction_requested';
    }
  | {
      /**
       * Identifies one durable runtime event.
       */
      event_id: string;
      /**
       * Event timestamp.
       */
      timestamp: string;
      /**
       * Owning Attempt; absent for native Scheduled work.
       */
      attempt_id?: AttemptId | null;
      /**
       * Owning turn; absent for native Scheduled work.
       */
      turn_id?: TurnId | null;
      /**
       * Interaction identity.
       */
      interaction_id: string;
      /**
       * Bounded durable settlement.
       */
      settlement:
        | {
            response: ReviewResponse;
            type: 'reviewed';
          }
        | {
            kind: ToolDeadlineKind;
            type: 'deadline_expired';
          }
        | {
            type: 'approved';
          }
        | {
            /**
             * The bounded client-facing denial reason.
             */
            reason: string;
            type: 'denied';
          }
        | {
            submission: QuestionnaireSubmission1;
            type: 'questionnaire_submitted';
          }
        | {
            type: 'questionnaire_declined';
          }
        | {
            /**
             * The first-winner cancellation cause.
             */
            reason:
              | 'user_requested'
              | 'runtime_shutdown'
              | 'parent_cancelled'
              | 'subagent_execution_deadline_exceeded';
            type: 'cancelled';
          };
      type: 'interaction_settled';
    };
/**
 * Typed kind of inbound information.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InboundKind".
 */
export type InboundKind =
  | {
      goal_continuation: GoalRef;
    }
  | 'message'
  | {
      context: ContextKind;
    };
/**
 * The bounded terminal answer facts used by all interaction layers.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InteractionSubject".
 */
export type InteractionSubject =
  | {
      review: ReviewSpecification;
      type: 'review';
    }
  | {
      /**
       * Caller correlation for one native invocation. Capability source is
       * independently represented by [`ToolOrigin`].
       */
      invocation_id:
        | {
            call_id: ToolCallId;
            caller: 'agent';
          }
        | {
            node: WorkflowNodeInstance;
            caller: 'workflow';
          };
      /**
       * Identifies a tool definition in the capability set.
       */
      tool_id: string;
      /**
       * The model-facing tool name.
       */
      tool_name: string;
      /**
       * The digest of the canonical model-issued arguments.
       */
      arguments_digest: string;
      /**
       * The bounded policy explanation shown to the client.
       */
      reason: string;
      type: 'approval';
    }
  | {
      invocation_id: ToolInvocationId;
      requester: InteractionRequester;
      questionnaire: QuestionnaireSpecification;
      type: 'questionnaire';
    };
/**
 * The distinct durable terminal settlements of one interaction.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InteractionSettlement".
 */
export type InteractionSettlement =
  | {
      response: ReviewResponse;
      type: 'reviewed';
    }
  | {
      kind: ToolDeadlineKind;
      type: 'deadline_expired';
    }
  | {
      type: 'approved';
    }
  | {
      /**
       * The bounded client-facing denial reason.
       */
      reason: string;
      type: 'denied';
    }
  | {
      submission: QuestionnaireSubmission1;
      type: 'questionnaire_submitted';
    }
  | {
      type: 'questionnaire_declined';
    }
  | {
      /**
       * The first-winner cancellation cause.
       */
      reason:
        | 'user_requested'
        | 'runtime_shutdown'
        | 'parent_cancelled'
        | 'subagent_execution_deadline_exceeded';
      type: 'cancelled';
    };
/**
 * The externally meaningful phase of one attempt.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientAttemptPhase".
 */
export type RuntimeClientAttemptPhase =
  | {
      type: 'incomplete';
    }
  | {
      type: 'admitted';
    }
  | {
      type: 'running';
    }
  | {
      /**
       * The platform-level terminal settlement.
       */
      outcome:
        | {
            /**
             * The normalized finish reason.
             */
            finish_reason:
              | {
                  type: 'stop';
                }
              | {
                  type: 'tool_calls';
                }
              | {
                  type: 'length';
                }
              | {
                  type: 'content_filter';
                }
              | {
                  type: 'refusal';
                }
              | {
                  /**
                   * The original provider reason, preserved for diagnostics.
                   */
                  reason: string;
                  type: 'other';
                };
            /**
             * The original reason exceeds its finite inline preview.
             */
            finish_reason_deferred?: boolean;
            /**
             * Exact allowlisted reason bytes at the published native cut.
             */
            finish_reason_source?: JournalFieldRef | null;
            type: 'completed';
          }
        | {
            /**
             * Why the attempt was cancelled.
             */
            reason:
              | 'user_requested'
              | 'runtime_shutdown'
              | 'parent_cancelled'
              | 'subagent_execution_deadline_exceeded';
            type: 'cancelled';
          }
        | {
            type: 'timed_out';
          }
        | {
            /**
             * Which limit was exceeded.
             */
            limit: 'max_turns' | 'max_tool_calls' | 'max_runtime_seconds';
            type: 'limit_exceeded';
          }
        | {
            /**
             * The normalized client-visible failure of one attempt.
             *
             * This is the external projection of the internal [`AttemptFailure`]
             * ([`crate::events::types::AttemptFailure`]): provider-specific fields
             * (such as the raw provider error code) never appear here.
             */
            error:
              | {
                  /**
                   * Error classes the runtime distinguishes for retry/termination decisions.
                   * Provider SDK error structs never cross this boundary.
                   */
                  kind:
                    | 'invalid_request'
                    | 'authentication'
                    | 'rate_limit'
                    | 'timeout'
                    | 'transport'
                    | 'provider_error'
                    | 'context_window_exceeded'
                    | 'cancelled'
                    | 'unsupported'
                    | 'malformed_tool_proposal'
                    | 'generation_degenerated'
                    | 'generation_budget_exceeded';
                  /**
                   * The normalized human-readable message.
                   */
                  message: string;
                  /**
                   * The retry hint, when the provider reported one.
                   */
                  retry_after_ms?: string | null;
                  type: 'model';
                }
              | {
                  /**
                   * The normalized runtime error.
                   */
                  error:
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'internal';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'invalid_state';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'unsupported';
                      }
                    | {
                        /**
                         * The tool name the model called.
                         */
                        name: string;
                        type: 'unknown_tool';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'durable_store';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'contract_violation';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'context_preparation_failed';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'context_compaction_failed';
                      }
                    | {
                        /**
                         * The policy's bounded rejection reason.
                         */
                        reason: string;
                        type: 'pre_step_rejected';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'pre_step_policy_failed';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'tool_result_observation_failed';
                      }
                    | {
                        /**
                         * The bounded recovery diagnostic: which durable evidence settled
                         * the attempt and what remained indeterminate.
                         */
                        message: string;
                        type: 'restart_interrupted';
                      }
                    | {
                        /**
                         * Human-readable diagnostic message.
                         */
                        message: string;
                        type: 'deferred_context_rejected';
                      };
                  type: 'runtime';
                };
            /**
             * The normalized diagnostic exceeds its finite inline preview.
             */
            error_deferred?: boolean;
            /**
             * Normalized diagnostic source; never a raw provider error.
             */
            error_source?: JournalFieldRef | null;
            type: 'failed';
          };
      type: 'settled';
    };
/**
 * The platform-level settlement outcome of one attempt.
 *
 * Provider finish reasons, runtime cancellation, timeout, limit
 * exhaustion, and runtime failure are distinct and never collapsed into
 * one string. A normalized model failure exposes its kind, message, and
 * retry hint — never the provider-specific error code.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientOutcome".
 */
export type RuntimeClientOutcome =
  | {
      /**
       * The normalized finish reason.
       */
      finish_reason:
        | {
            type: 'stop';
          }
        | {
            type: 'tool_calls';
          }
        | {
            type: 'length';
          }
        | {
            type: 'content_filter';
          }
        | {
            type: 'refusal';
          }
        | {
            /**
             * The original provider reason, preserved for diagnostics.
             */
            reason: string;
            type: 'other';
          };
      /**
       * The original reason exceeds its finite inline preview.
       */
      finish_reason_deferred?: boolean;
      /**
       * Exact allowlisted reason bytes at the published native cut.
       */
      finish_reason_source?: JournalFieldRef | null;
      type: 'completed';
    }
  | {
      /**
       * Why the attempt was cancelled.
       */
      reason:
        | 'user_requested'
        | 'runtime_shutdown'
        | 'parent_cancelled'
        | 'subagent_execution_deadline_exceeded';
      type: 'cancelled';
    }
  | {
      type: 'timed_out';
    }
  | {
      /**
       * Which limit was exceeded.
       */
      limit: 'max_turns' | 'max_tool_calls' | 'max_runtime_seconds';
      type: 'limit_exceeded';
    }
  | {
      /**
       * The normalized client-visible failure of one attempt.
       *
       * This is the external projection of the internal [`AttemptFailure`]
       * ([`crate::events::types::AttemptFailure`]): provider-specific fields
       * (such as the raw provider error code) never appear here.
       */
      error:
        | {
            /**
             * Error classes the runtime distinguishes for retry/termination decisions.
             * Provider SDK error structs never cross this boundary.
             */
            kind:
              | 'invalid_request'
              | 'authentication'
              | 'rate_limit'
              | 'timeout'
              | 'transport'
              | 'provider_error'
              | 'context_window_exceeded'
              | 'cancelled'
              | 'unsupported'
              | 'malformed_tool_proposal'
              | 'generation_degenerated'
              | 'generation_budget_exceeded';
            /**
             * The normalized human-readable message.
             */
            message: string;
            /**
             * The retry hint, when the provider reported one.
             */
            retry_after_ms?: string | null;
            type: 'model';
          }
        | {
            /**
             * The normalized runtime error.
             */
            error:
              | {
                  /**
                   * Human-readable diagnostic message.
                   */
                  message: string;
                  type: 'internal';
                }
              | {
                  /**
                   * Human-readable diagnostic message.
                   */
                  message: string;
                  type: 'invalid_state';
                }
              | {
                  /**
                   * Human-readable diagnostic message.
                   */
                  message: string;
                  type: 'unsupported';
                }
              | {
                  /**
                   * The tool name the model called.
                   */
                  name: string;
                  type: 'unknown_tool';
                }
              | {
                  /**
                   * Human-readable diagnostic message.
                   */
                  message: string;
                  type: 'durable_store';
                }
              | {
                  /**
                   * Human-readable diagnostic message.
                   */
                  message: string;
                  type: 'contract_violation';
                }
              | {
                  /**
                   * Human-readable diagnostic message.
                   */
                  message: string;
                  type: 'context_preparation_failed';
                }
              | {
                  /**
                   * Human-readable diagnostic message.
                   */
                  message: string;
                  type: 'context_compaction_failed';
                }
              | {
                  /**
                   * The policy's bounded rejection reason.
                   */
                  reason: string;
                  type: 'pre_step_rejected';
                }
              | {
                  /**
                   * Human-readable diagnostic message.
                   */
                  message: string;
                  type: 'pre_step_policy_failed';
                }
              | {
                  /**
                   * Human-readable diagnostic message.
                   */
                  message: string;
                  type: 'tool_result_observation_failed';
                }
              | {
                  /**
                   * The bounded recovery diagnostic: which durable evidence settled
                   * the attempt and what remained indeterminate.
                   */
                  message: string;
                  type: 'restart_interrupted';
                }
              | {
                  /**
                   * Human-readable diagnostic message.
                   */
                  message: string;
                  type: 'deferred_context_rejected';
                };
            type: 'runtime';
          };
      /**
       * The normalized diagnostic exceeds its finite inline preview.
       */
      error_deferred?: boolean;
      /**
       * Normalized diagnostic source; never a raw provider error.
       */
      error_source?: JournalFieldRef | null;
      type: 'failed';
    };
/**
 * Why a model generation finished.
 *
 * Finish reasons are semantic outcomes, not raw provider strings. When a
 * provider returns a reason the runtime cannot normalize, it is preserved in
 * [`ModelFinishReason::Other`] rather than collapsing the entire model into
 * arbitrary strings.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelFinishReason".
 */
export type ModelFinishReason =
  | {
      type: 'stop';
    }
  | {
      type: 'tool_calls';
    }
  | {
      type: 'length';
    }
  | {
      type: 'content_filter';
    }
  | {
      type: 'refusal';
    }
  | {
      /**
       * The original provider reason, preserved for diagnostics.
       */
      reason: string;
      type: 'other';
    };
/**
 * Which attempt execution limit was exceeded.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AttemptLimit".
 */
export type AttemptLimit = 'max_turns' | 'max_tool_calls' | 'max_runtime_seconds';
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentState".
 */
export type AgentState = ('admitting' | 'active' | 'stopping' | 'inactive') | 'unavailable';
/**
 * What the child is observably doing right now.
 *
 * Every variant derives from an objective child runtime transition; there
 * is deliberately no idle/quiescent variant beyond the neutral
 * `AwaitingActivity` (see the module documentation).
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SubagentActivity".
 */
export type SubagentActivity =
  | {
      type: 'awaiting_activity';
    }
  | {
      /**
       * Identifies one actual provider-neutral model request.
       *
       * A request identity is distinct from an attempt, turn, retry ordinal,
       * and Event Journal sequence. Primary requests derive the `request:`
       * namespace from immutable [`RequestIdentity`](crate::model::snapshot::RequestIdentity).
       * Auxiliary requests allocate the separate `req_` `UUIDv7` namespace with
       * [`RequestId::generate_auxiliary`]. Each physical retry is a distinct
       * request. Identity alone grants no execution permission.
       */
      request_id: string;
      /**
       * The retry ordinal of THIS request: 0 for a first attempt, `n`
       * for the request the nth scheduled retry armed.
       */
      retry: number;
      type: 'model';
    }
  | {
      /**
       * The scheduled retry ordinal.
       */
      retry: number;
      type: 'retrying_model';
    }
  | {
      active: number;
      representative?: SubagentToolRepresentative | null;
      type: 'tool_group';
    }
  | {
      type: 'compacting';
    }
  | {
      /**
       * What the child waits on.
       */
      on:
        | {
            type: 'review';
          }
        | {
            /**
             * The tool awaiting approval.
             */
            tool_id?: ToolId | null;
            type: 'approval';
          }
        | {
            type: 'questionnaire';
          };
      type: 'waiting';
    };
/**
 * Why the child is blocked on a native interaction.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SubagentWaitReason".
 */
export type SubagentWaitReason =
  | {
      type: 'review';
    }
  | {
      /**
       * The tool awaiting approval.
       */
      tool_id?: ToolId | null;
      type: 'approval';
    }
  | {
      type: 'questionnaire';
    };
/**
 * Who owns one execution of a tool.
 *
 * This axis decides ownership and settlement, never scheduling:
 *
 * - `ForegroundOnly` — every invocation settles before the current agent
 *   attempt may continue past that tool result. The execution is
 *   attempt-owned and observable attempt cancellation physically reaches
 *   it.
 * - `BackgroundOnly` — every invocation is detached from the current
 *   attempt after successful background dispatch; the conversation owns the
 *   execution.
 * - `ModelSelectable` — the model must explicitly choose foreground or
 *   background execution for each invocation through the required
 *   `execution_mode` invocation field. That field is reserved by the
 *   runtime only under this policy; registration rejects a `ModelSelectable`
 *   tool whose canonical schema already defines it.
 *
 * Foreground/background is orthogonal to sequential/parallel scheduling
 * ([`ToolConcurrencyPolicy`]).
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolExecutionPolicy".
 */
export type ToolExecutionPolicy = 'foreground_only' | 'background_only' | 'model_selectable';
/**
 * How calls of one tool within one batch are scheduled.
 *
 * A `Sequential` invocation is an exclusive scheduling barrier; adjacent
 * `Parallel` invocations may execute concurrently as one group. This axis
 * describes concurrency only, never foreground/background ownership
 * ([`ToolExecutionPolicy`]).
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolConcurrencyPolicy".
 */
export type ToolConcurrencyPolicy = 'sequential' | 'parallel';
/**
 * Whether a resolved tool invocation requires a native approval interaction.
 *
 * Denial is intentionally not part of this policy. A tool that is not
 * eligible belongs to availability/authorization and is rejected before
 * approval; HITL only decides whether an otherwise eligible invocation needs
 * a human decision.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolApprovalPolicy".
 */
export type ToolApprovalPolicy = 'never' | 'always';
/**
 * Whether the runtime may automatically re-execute a tool after a crash.
 *
 * `Never` is the safe default. After a crash the runtime must never blindly
 * replay a tool whose external completion state is unknown: it records an
 * `OutcomeUnknown` result and lets the model decide the next action.
 * Automatic replay is allowed only for tools that explicitly declare
 * themselves idempotent. `Idempotent` is metadata for the future #12
 * recovery policy, not permission to invent replay behavior in M8.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolReplayPolicy".
 */
export type ToolReplayPolicy = 'never' | 'idempotent';
/**
 * A monotonic revision counter for the capability set observed by an attempt.
 *
 * A running attempt snapshots one immutable `CapabilityRevision` when it
 * starts and keeps it for its entire lifetime. The revision is a counter,
 * not a provider-specific string: every capability mutation atomically swaps
 * the whole capability set and increments the revision.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CapabilityRevision".
 */
export type CapabilityRevision = string;
/**
 * Identifies a skill bound to the runtime.
 *
 * M6 makes the standard Agent Skills `name` the logical skill identity:
 * a `SkillId` is the validated standard skill name, not an externally
 * assigned opaque string.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SkillId".
 */
export type SkillId = string;
/**
 * Identifies an immutable skill version.
 *
 * M6 derives `SkillVersionId` deterministically from the complete
 * accepted Skill package content (SHA-256 over the stable textual form
 * `sha256:<64 lowercase hex characters>`). Any package-content change
 * yields a new version id.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SkillVersionId".
 */
export type SkillVersionId = string;
/**
 * The client-visible identity of one optional capability source (Issue
 * #81).
 *
 * Native tools never appear here: their construction is part of the core
 * runtime and remains fatal at composition.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CapabilitySourceDescriptor".
 */
export type CapabilitySourceDescriptor =
  | {
      package: string;
      type: 'managed_python';
    }
  | {
      /**
       * Identifies an MCP server bound to the runtime.
       */
      server_id: string;
      type: 'mcp';
    };
/**
 * The client-visible availability of one optional capability source.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CapabilitySourceStateView".
 */
export type CapabilitySourceStateView =
  | {
      type: 'unprepared';
    }
  | {
      type: 'ready';
    }
  | {
      type: 'unavailable';
    };
/**
 * One Skill source authority.
 *
 * The declaration order **is** the precedence order: a later variant wins
 * the same logical Skill identity against an earlier one.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SkillSource".
 */
export type SkillSource = 'user' | 'workspace';
/**
 * A parsing/validation failure of **one** Skill package.
 *
 * Every variant identifies the responsible candidate. A failure here
 * excludes exactly that candidate and is preserved verbatim inside
 * [`SkillDiagnostic::PackageInvalid`]; it never fails discovery, and it
 * never suppresses an unrelated valid package.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SkillPackageError".
 */
export type SkillPackageError =
  | {
      directory: string;
      cause: 'invalid_name';
    }
  | {
      directory: string;
      cause: 'name_directory_mismatch';
    }
  | {
      directory: string;
      cause: 'missing_skill_markdown';
    }
  | {
      directory: string;
      cause: 'skill_markdown_not_regular_file';
    }
  | {
      directory: string;
      cause: 'malformed_frontmatter';
    }
  | {
      directory: string;
      cause: 'invalid_description';
    }
  | {
      directory: string;
      cause: 'invalid_compatibility';
    }
  | {
      directory: string;
      cause: 'malformed_metadata';
    }
  | {
      directory: string;
      cause: 'invalid_dependency_declaration';
    }
  | {
      path: string;
      cause: 'unsupported_symlink';
    }
  | {
      path: string;
      cause: 'unrepresentable_root';
    }
  | {
      path: string;
      cause: 'io';
    };
/**
 * The terminal outcome delivered to the semantic waiter.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InteractionOutcome".
 */
export type InteractionOutcome =
  | {
      kind: ToolDeadlineKind;
      type: 'deadline_expired';
    }
  | {
      /**
       * A typed response to one native interaction.
       */
      response:
        | {
            response: ReviewResponse;
            type: 'review';
          }
        | {
            /**
             * The finite approval decision.  It has no tool arguments.
             */
            decision:
              | {
                  type: 'allow';
                }
              | {
                  /**
                   * A bounded client-facing reason.
                   */
                  reason: string;
                  type: 'deny';
                };
            type: 'approval';
          }
        | {
            /**
             * A submitted answer set or explicit decline.
             */
            response:
              | {
                  type: 'submitted';
                  value: QuestionnaireSubmission;
                }
              | {
                  type: 'declined';
                };
            type: 'questionnaire';
          };
      type: 'responded';
    }
  | {
      /**
       * The first-winner cancellation cause from the owning attempt.
       */
      reason:
        | 'user_requested'
        | 'runtime_shutdown'
        | 'parent_cancelled'
        | 'subagent_execution_deadline_exceeded';
      type: 'cancelled';
    };

export interface ProtocolTypes {
  message: ProtocolMessage;
}
/**
 * Every attached operation addresses all routing domains explicitly.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AttachmentTarget".
 */
export interface AttachmentTarget {
  session_id: SessionId;
  conversation_id: ConversationId;
  runtime_incarnation: RuntimeIncarnationId;
  attachment_id: AttachmentId;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ClassificationSubmission".
 */
export interface ClassificationSubmission {
  correlation: CorrelationId;
  input: ClassificationInput;
  labels: string[];
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "BtwSubmission".
 */
export interface BtwSubmission {
  correlation: CorrelationId;
  question: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UploadMetadata".
 */
export interface UploadMetadata {
  name: string;
  size: number;
}
/**
 * Native occurrence identity owns navigation; optional inline IDs are checks.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolLocator".
 */
export interface TraceToolLocator {
  occurrence: RuntimeClientToolOccurrence;
  call_id?: ToolCallId | null;
  tool_id?: ToolId | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientToolOccurrence".
 */
export interface RuntimeClientToolOccurrence {
  locator: CanonicalContentLocator;
  block_index: ContentBlockIndex;
}
/**
 * Inclusive Journal/transcript bounds a read was captured at, plus its semantic mutation epoch.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConversationReadCut".
 */
export interface ConversationReadCut {
  conversation_id: ConversationId;
  journal: string;
  transcript: string;
  /**
   * Advances on every edit/removal/adoption of a mutable pending transcript body.
   */
  mutation_revision: string;
  /**
   * Immutable canonical Ledger frontier at this same native transaction cut.
   */
  ledger_through: string;
  /**
   * Native model-context structure revision, separate from canonical history.
   */
  surface_revision: string;
  /**
   * Actual retained transcript count, distinct from its positional frontier.
   */
  transcript_total: string;
  /**
   * Native acceptance frontier, including pending Context without transcript positions.
   */
  pending_through: string;
  /**
   * Exact pending count at this cut, separate from transcript collection size.
   */
  pending_total: string;
}
/**
 * Origin survives lineage copying; ordinal never participates in identity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConversationTurnId".
 */
export interface ConversationTurnId {
  conversation_id: ConversationId;
  attempt_id: AttemptId;
}
/**
 * The authoritative mutable model configuration of one conversation
 * session.
 *
 * This one type is the session's state, the `model_get` result, and the
 * `model_set` parameter: an update is a whole-state replacement, never an
 * ambiguous JSON patch.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionModelConfig".
 */
export interface SessionModelConfig {
  /**
   * The selected catalog model.
   */
  model: string;
  /**
   * The selected Model Profile; the model's `default_profile` is used
   * when absent.
   */
  profile?: ModelProfileId | null;
  /**
   * The session request-parameter overrides.
   */
  requestParams?: {
    [k: string]: unknown;
  };
  /**
   * The session output-budget override; the selected profile's default,
   * else the model's hard maximum, is used when absent.
   */
  maxOutputTokens?: number | null;
  /**
   * The compaction summary model policy.
   */
  summaryModel?:
    | {
        mode: 'session';
      }
    | {
        /**
         * The catalog model reference.
         */
        model: string;
        /**
         * The selected Model Profile; the model's `default_profile` is used
         * when absent.
         */
        profile?: ModelProfileId | null;
        /**
         * The explicit auxiliary request-parameter overrides.
         */
        request_params?: {
          [k: string]: unknown;
        };
        /**
         * The explicit auxiliary output-budget override.
         */
        max_output_tokens?: number | null;
        mode: 'explicit';
      };
  /**
   * The independent automatic title model policy.
   */
  titleModel?:
    | {
        mode: 'session';
      }
    | {
        /**
         * The catalog model reference.
         */
        model: string;
        /**
         * The selected Model Profile; the model's `default_profile` is used
         * when absent.
         */
        profile?: ModelProfileId | null;
        /**
         * The explicit auxiliary request-parameter overrides.
         */
        request_params?: {
          [k: string]: unknown;
        };
        /**
         * The explicit auxiliary output-budget override.
         */
        max_output_tokens?: number | null;
        mode: 'explicit';
      };
  classificationModel?: AuxiliaryModelPolicy;
  btwModel?: AuxiliaryModelPolicy;
}
/**
 * A native catalog choice at the exact resource generation the client read.
 * Choosing defaults replaces the Session configuration; it never patches old overrides.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CatalogModelSelection".
 */
export interface CatalogModelSelection {
  binding_revision: string;
  model_position: number;
  profile_position?: number | null;
}
/**
 * A conversation-scoped identity whose ordinal is never reused.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleId".
 */
export interface ScheduleId {
  conversation_id: ConversationId;
  ordinal: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleCommand".
 */
export interface ScheduleCommand {
  command_id: string;
  mutation: ScheduleMutation;
}
/**
 * Exact observation required by every mutation of an existing Goal.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "GoalRef".
 */
export interface GoalRef {
  id: string;
  revision: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentObservationCursor".
 */
export interface AgentObservationCursor {
  agent_id: AgentId;
  conversation_id: ConversationId;
  activation_id: SubagentId;
  cursor: RuntimeClientCursor;
}
/**
 * A server-issued capability, scoped to exactly one Session.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UploadReceipt".
 */
export interface UploadReceipt {
  session_id: SessionId;
  batch_id: string;
  token: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InitializeParams".
 */
export interface InitializeParams {
  protocol_version: number;
  client: ClientIdentity;
  presentation: PresentationCapabilities;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ClientIdentity".
 */
export interface ClientIdentity {
  name: string;
  version: string;
}
/**
 * Rendering hints never confer execution or interaction authority.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "PresentationCapabilities".
 */
export interface PresentationCapabilities {
  images: boolean;
  questionnaires: boolean;
  reviews: boolean;
}
/**
 * Deliberate Session-owned intent. Effective configuration is never persisted.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionPersistentState".
 */
export interface SessionPersistentState {
  cwd: string;
  /**
   * Explicit initial selection. Omission delegates to the native configuration
   * owner at creation; clients must not copy a projected default into this field.
   */
  model?: SessionModelConfig | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CheckpointReadCut".
 */
export interface CheckpointReadCut {
  canonical_foreground?: CanonicalForegroundReadCut | null;
  live_publication?: LivePublicationReadCut | null;
  conversation_id: ConversationId;
  activation_id?: SubagentId | null;
  revision: string;
  through: string;
  /**
   * Immutable durable descriptor frontier captured with this display cut.
   */
  source_cut?: ObservationReadCut | null;
  domain_revisions: {
    [k: string]: string;
  };
  collection_cuts: {
    [k: string]: ObservationCollectionCut;
  };
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CanonicalForegroundReadCut".
 */
export interface CanonicalForegroundReadCut {
  locator: CanonicalContentLocator;
  total: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "LivePublicationReadCut".
 */
export interface LivePublicationReadCut {
  source: ContentSource;
  revision: number;
  block_total: number;
  tool_total: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ObservationReadCut".
 */
export interface ObservationReadCut {
  read: ConversationReadCut;
  ledger_through: string;
  transcript_total: string;
  surface_revision: SurfaceRevision;
  inherited_through: TranscriptCursor;
}
/**
 * One native collection owner's synchronized observation frontier.
 * It carries no execution authority and is independent of client cursors.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ObservationCollectionCut".
 */
export interface ObservationCollectionCut {
  generation: string;
  total: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ContentRef".
 */
export interface ContentRef {
  conversation_id: ConversationId;
  source: ContentSource;
  representation: ContentRepresentation;
  version: number;
  /**
   * Immutable sources use revision 1. For `LivePublicationPrefix` this is
   * the committed frame sequence plus one, with exact H verified against
   * the retained staging frontier index and projection release cut.
   */
  content_revision: number;
  readable_end: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionNodeValueRef".
 */
export interface SessionNodeValueRef {
  session_id: SessionId;
  node_id: SessionNodeId;
  ordinal: string;
  field: SessionNodeValueField;
  readable_end: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EditorLocator".
 */
export interface EditorLocator {
  session_id: SessionId;
  node_id: SessionNodeId;
  source_session: SessionId;
  source_node: SessionNodeId;
  source_surface_revision: SurfaceRevision;
}
/**
 * Exact durable pending occurrence and compare-and-set revision.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "PendingInboundRef".
 */
export interface PendingInboundRef {
  /**
   * Durable sequence; never reused.
   */
  sequence: string;
  /**
   * Expected native content revision.
   */
  revision: string;
}
/**
 * The root-facing address of a conversation-local interaction.
 *
 * `InteractionId` is allocated inside one conversation/attempt domain and is
 * intentionally not globally unique. The pair is the only identity that
 * crosses a Runtime Client or parent/child routing boundary.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InteractionRef".
 */
export interface InteractionRef {
  /**
   * The conversation-owned semantic interaction domain.
   */
  conversation_id: string;
  /**
   * The interaction identity allocated by that conversation's coordinator.
   */
  interaction_id: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ReviewResponse".
 */
export interface ReviewResponse {
  instance: WorkflowNodeInstance;
  subject_digest: string;
  decision: ReviewDecision;
}
/**
 * One concrete node visit in an owning block instance.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowNodeInstance".
 */
export interface WorkflowNodeInstance {
  block: WorkflowBlockInstance;
  node: string;
  visit: number;
}
/**
 * A concrete block instance. Root/Parallel append zero; Loop appends its one-based iteration.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowBlockInstance".
 */
export interface WorkflowBlockInstance {
  run: WorkflowRunId;
  definition: WorkflowDefinitionPath;
  invocations: number[];
}
/**
 * Runtime-owned identity, independent of model `ToolCall` text.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowRunId".
 */
export interface WorkflowRunId {
  /**
   * Owning conversation.
   */
  conversation_id: string;
  /**
   * Real Tool or durable Scheduled origin, unique across recovery.
   */
  origin:
    | {
        attempt_id: AttemptId;
        type: 'tool';
      }
    | {
        start_id: ScheduledWorkflowStartId;
        type: 'scheduled';
      };
  /**
   * WorkflowRuntime-owned invocation ordinal, allocated at run admission.
   */
  invocation: string;
}
/**
 * Static source location; never a concrete execution authority.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowDefinitionPath".
 */
export interface WorkflowDefinitionPath {
  workflow_id: WorkflowId;
  /**
   * Pairs of owner node and child key (Parallel branch or Loop `body`); empty for root.
   */
  blocks: string[];
}
/**
 * The submitted decisions for one questionnaire. Omitted questions are
 * intentionally allowed so a user may submit a partial questionnaire.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "QuestionnaireSubmission".
 */
export interface QuestionnaireSubmission {
  /**
   * At most one entry per answered question.
   */
  answers: QuestionnaireAnswerEntry[];
}
/**
 * One decision for one question. It carries only an index and a decision,
 * never a client-echoed copy of the request facts.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "QuestionnaireAnswerEntry".
 */
export interface QuestionnaireAnswerEntry {
  /**
   * Zero-based index into the immutable questionnaire.
   */
  question_index: number;
  /**
   * The typed decision for that question.
   */
  answer:
    | {
        type: 'text';
        value: TextAnswer;
      }
    | {
        type: 'number';
        value: NumberAnswer;
      }
    | {
        type: 'integer';
        value: IntegerAnswer;
      }
    | {
        type: 'boolean';
        value: BooleanAnswer;
      }
    | {
        type: 'option';
        value: OptionAnswer;
      }
    | {
        type: 'options';
        value: OptionsAnswer;
      }
    | {
        type: 'custom';
        value: CustomAnswer;
      };
}
/**
 * A bounded free-form text answer.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TextAnswer".
 */
export interface TextAnswer {
  /**
   * The bounded user-entered text.
   */
  value: string;
}
/**
 * A finite numeric answer, carried as canonical binary64 **text**.
 *
 * The value is a [`FiniteNumber`], the same domain the question's bounds and
 * the emitted MCP content use, so what the runtime validates is bit-identical
 * to what it sends — and the wire encoding is the value's own bits, so it is
 * bit-identical to what the client selected too. See [`FiniteNumber`] for why
 * a JSON number could not carry that identity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "NumberAnswer".
 */
export interface NumberAnswer {
  /**
   * The typed value, exactly representable as a finite binary64.
   */
  value: string;
}
/**
 * A whole-number answer.
 *
 * The value is an [`ExactInteger`], which crosses the Runtime Client protocol
 * as canonical decimal text, so an answer above the binary64 integer frontier
 * round-trips exactly instead of being rounded by a client.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "IntegerAnswer".
 */
export interface IntegerAnswer {
  /**
   * The typed value.
   */
  value: ('0' | string) & string;
}
/**
 * A typed boolean answer.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "BooleanAnswer".
 */
export interface BooleanAnswer {
  /**
   * The canonical business value, never a `Yes`/`No` display string.
   */
  value: boolean;
}
/**
 * One declared option selected for a single-choice question.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "OptionAnswer".
 */
export interface OptionAnswer {
  /**
   * The zero-based index into the question's declared options.
   */
  option_index: number;
}
/**
 * Several declared options selected for a multi-choice question.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "OptionsAnswer".
 */
export interface OptionsAnswer {
  /**
   * Zero-based option indices in ascending canonical order.
   */
  option_indices: number[];
}
/**
 * A custom answer entered by the user.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CustomAnswer".
 */
export interface CustomAnswer {
  /**
   * The bounded user-entered answer.
   */
  answer: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectiveFieldRef".
 */
export interface EffectiveFieldRef {
  revision: string;
  field: EffectiveFieldName;
  item_index?: number | null;
  byte_length: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceFieldRef".
 */
export interface SourceFieldRef {
  revision: string;
  field: SourceFieldName;
  item_index?: number | null;
  byte_length: number;
}
/**
 * The canonical YAML/domain representation of one workflow.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowDefinition".
 */
export interface WorkflowDefinition {
  /**
   * The model-facing description of the workflow Tool.
   */
  description: string;
  /**
   * Trusted finite total foreground lifetime, including descendant waits.
   */
  timeout_ms?: string;
  block: WorkflowBlock;
}
/**
 * The root lexical execution scope.
 */
export interface WorkflowBlock {
  /**
   * The workflow input JSON Schema.
   */
  input: {
    [k: string]: unknown;
  };
  /**
   * The workflow output JSON Schema.
   */
  output: {
    [k: string]: unknown;
  };
  /**
   * The one explicit entry node.
   */
  entry: string;
  /**
   * Stable node definitions keyed by explicit node id.
   */
  nodes: {
    [k: string]: WorkflowNodeDefinition;
  };
  /**
   * Ordinary sequential control-flow edges.
   */
  edges?: WorkflowEdgeDefinition[];
}
/**
 * A fixed lexical graph shared by root, Parallel branches and Loop bodies.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowBlock".
 */
export interface WorkflowBlock1 {
  /**
   * The workflow input JSON Schema.
   */
  input: {
    [k: string]: unknown;
  };
  /**
   * The workflow output JSON Schema.
   */
  output: {
    [k: string]: unknown;
  };
  /**
   * The one explicit entry node.
   */
  entry: string;
  /**
   * Stable node definitions keyed by explicit node id.
   */
  nodes: {
    [k: string]: WorkflowNodeDefinition;
  };
  /**
   * Ordinary sequential control-flow edges.
   */
  edges?: WorkflowEdgeDefinition[];
}
/**
 * One explicit edge in the workflow graph.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowEdgeDefinition".
 */
export interface WorkflowEdgeDefinition {
  /**
   * Source node id.
   */
  from: string;
  /**
   * Destination node id.
   */
  to: string;
  /**
   * Required Branch/Loop outcome port. Ordinary nodes must omit it.
   */
  port?: WorkflowPort | null;
}
/**
 * The optional invocation-scoped capability override of this node
 * (Issue #258).
 *
 * This is **trusted static program data**: it is part of the
 * compiled Workflow, is validated against the Workflow's own
 * admitted generation, and is not reachable from model output, node
 * input values, or task text. It may therefore legitimately replace
 * the role's defaults with capabilities the invoking main model does
 * not itself hold — never with capabilities the generation does not
 * authorize.
 */
export interface SubagentInvocationOverride {
  tools?: ToolSelectionDocument;
  /**
   * The complete replacement Skill selection, when this dimension is
   * overridden. Absent uses the definition's selection.
   */
  skills?: string[];
  plugins?: NativeAgentExtensionSelection;
}
/**
 * The complete replacement Tool selection, when this dimension is
 * overridden. Absent uses the definition's selection.
 */
export interface ToolSelectionDocument {
  builtin?: string[];
  sources?: {
    [k: string]: SourceToolSelection;
  };
}
/**
 * The complete replacement Plugin composition, when this
 * dimension is overridden. Absent uses the definition's composition.
 */
export interface NativeAgentExtensionSelection {
  agentStatus?: AgentStatusExtensionDocument;
  todo?: TodoExtensionDocument;
  goal?: GoalExtensionDocument;
}
/**
 * The requested Agent Status extension, when this selection composes it.
 *
 * `null` is not accepted: the field is either absent (not composed) or a
 * complete authored configuration. Collapsing an explicit `null` into
 * "absent" would give one wire spelling two meanings.
 */
export interface AgentStatusExtensionDocument {
  /**
   * Whether this composition includes the Agent Status extension at all.
   *
   * With `false` the runtime composes no status engine: the Agent Loop
   * emits no Agent Status and is otherwise a completely ordinary
   * `ConversationRuntime`.
   */
  enabled?: boolean;
  time?: TimeStatusConfig;
  background?: BackgroundStatusConfig;
}
/**
 * The Time contributor configuration.
 */
export interface TimeStatusConfig {
  /**
   * Whether Time participates in an available Agent Status opportunity.
   */
  enabled?: boolean;
  /**
   * The optional IANA timezone used only by Time presentation. Omitting it
   * renders UTC.
   */
  timezone?: string | null;
}
/**
 * The Background contributor configuration.
 */
export interface BackgroundStatusConfig {
  /**
   * Whether Background participates in an available Agent Status opportunity.
   */
  enabled?: boolean;
}
/**
 * The requested Todo extension, when this selection composes it
 * (Issue #259).
 *
 * Absent means the child composes no Todo at all — no list authority, no
 * `todo` Tool, no status contribution — never "whatever the role
 * authored". `null` is refused for the same reason `agentStatus` refuses
 * it: one wire spelling may not carry two meanings.
 */
export interface TodoExtensionDocument {
  /**
   * Whether this composition includes the Todo extension at all.
   */
  enabled?: boolean;
}
/**
 * Authored opt-in Goal composition.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "GoalExtensionDocument".
 */
export interface GoalExtensionDocument {
  enabled?: boolean;
}
/**
 * One explicit input projection and fixed private Parallel block.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowBranch".
 */
export interface WorkflowBranch {
  /**
   * Closed, explicitly tagged value syntax; no evaluation of expression strings.
   */
  input:
    | {
        path: string[];
        type: 'reference';
      }
    | {
        value: unknown;
        type: 'literal';
      }
    | {
        fields: {
          [k: string]: WorkflowValue;
        };
        type: 'object';
      }
    | {
        items: WorkflowValue[];
        type: 'array';
      };
  block: WorkflowBlock2;
}
/**
 * The child's private graph and contracts.
 */
export interface WorkflowBlock2 {
  /**
   * The workflow input JSON Schema.
   */
  input: {
    [k: string]: unknown;
  };
  /**
   * The workflow output JSON Schema.
   */
  output: {
    [k: string]: unknown;
  };
  /**
   * The one explicit entry node.
   */
  entry: string;
  /**
   * Stable node definitions keyed by explicit node id.
   */
  nodes: {
    [k: string]: WorkflowNodeDefinition;
  };
  /**
   * Ordinary sequential control-flow edges.
   */
  edges?: WorkflowEdgeDefinition[];
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "McpWrite".
 */
export interface McpWrite {
  definition: McpAuthoring;
  retained_env?: string[];
  retained_headers?: string[];
}
/**
 * Secret-field presence is retained even for empty tables, so project authority
 * cannot be widened by replacing an authored empty table with a default map.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "McpAuthoring".
 */
export interface McpAuthoring {
  sensitive_env?: {
    [k: string]: EnvironmentReference;
  } | null;
  sensitive_headers?: {
    [k: string]: EnvironmentReference;
  } | null;
  type?: McpTransportType | null;
  url?: string | null;
  headers?: {
    [k: string]: string;
  };
  command?: string | null;
  args?: string[];
  env?: {
    [k: string]: string;
  };
  cwd?: string | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ProviderWrite".
 */
export interface ProviderWrite {
  base_url: string;
  credential: CredentialEdit;
}
/**
 * One authored Model. A Model declares either model-level `request_params`
 * or a nonempty `profiles` collection with a `default_profile`, never both.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "Model".
 */
export interface Model {
  provider: string;
  id: string;
  protocol: ModelProtocol;
  context_window: string;
  /**
   * The hard maximum output budget of every invocation of this Model.
   */
  max_output_tokens: number;
  capabilities: Capabilities;
  /**
   * Native request parameters of a Model without profiles. Presence is
   * retained: a Model with profiles must not declare it, even as `{}`.
   */
  request_params?: RequestParams | null;
  default_profile?: ModelProfileId | null;
  profiles?: {
    [k: string]: Profile;
  } | null;
  compat?: Compat;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "Capabilities".
 */
export interface Capabilities {
  input_modalities: Modality[];
  output_modalities: Modality[];
  tool_calls: boolean;
  reasoning: boolean;
  /**
   * Verified native JSON Schema enforcement; never inferred from a model name.
   */
  json_schema_output?: boolean;
}
/**
 * Opaque provider-native request parameters as a structured JSON object. Nested objects, arrays and null are preserved. Every number is exactly an IEEE 754 binary64 value (I-JSON); another, such as 9007199254740993, is refused. Runtime-protected wire keys are checked during model validation.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RequestParams".
 */
export interface RequestParams {
  [k: string]: unknown;
}
/**
 * One authored Model Profile: a complete, independent invocation preset.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "Profile".
 */
export interface Profile {
  /**
   * Required for every profile of a reasoning-capable Model; omission
   * means `false` for a non-reasoning Model.
   */
  reasoning_enabled?: boolean | null;
  /**
   * Default output budget; at most the Model's `max_output_tokens`.
   */
  max_output_tokens?: number | null;
  request_params?: RequestParams;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "Compat".
 */
export interface Compat {
  chat_max_tokens_field?: ChatMaxTokensField | null;
  chat_stream_usage?: ChatStreamUsage | null;
  chat_reasoning_replay?: ChatReasoningReplay | null;
  chat_tool_protocol?: ChatToolProtocol | null;
  responses_storage?: ResponsesStorageMode | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelLayer".
 */
export interface ModelLayer {
  model?: ModelRef | null;
  /**
   * The selected Model Profile; the Model's `default_profile` when omitted.
   */
  profile?: ModelProfileId | null;
  request_params?: RequestParams | null;
  max_output_tokens?: ModelOutput | null;
  summary_model?: AuxiliaryModelAuthoring | null;
  title_model?: AuxiliaryModelAuthoring | null;
  classification_model?: AuxiliaryModelAuthoring | null;
  btw_model?: AuxiliaryModelAuthoring | null;
}
/**
 * The authored Todo extension (Issue #259).
 *
 * Todo is *one* capability with several faces, and `enabled` composes all
 * of them together or none of them:
 *
 * ```text
 * enabled = true    conversation-owned ConversationTodoList authority
 *                   the model-facing `todo` Tool
 *                   the bounded read-only Todo status presentation
 *                   the Runtime Client / TUI Todo projection
 *
 * enabled = false   none of the above for this runtime; canonical history
 *                   keeps every Todo ToolCall/ToolResult it already holds
 * ```
 *
 * It carries no contributor settings today. That is a statement about the
 * extension, not a placeholder: the list's bounds, transitions, and
 * dependency rules are owned by
 * [`ConversationTodoList`](crate::tools::todo::ConversationTodoList) and are
 * not launch configuration.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TodoExtensionDocument".
 */
export interface TodoExtensionDocument1 {
  /**
   * Whether this composition includes the Todo extension at all.
   */
  enabled?: boolean;
}
/**
 * The authored Agent Status extension.
 *
 * `enabled` composes the extension in or out of the runtime as a whole;
 * `time` and `background` remain the two bounded status contributors, with
 * exactly the semantics they had before the migration.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentStatusExtensionDocument".
 */
export interface AgentStatusExtensionDocument1 {
  /**
   * Whether this composition includes the Agent Status extension at all.
   *
   * With `false` the runtime composes no status engine: the Agent Loop
   * emits no Agent Status and is otherwise a completely ordinary
   * `ConversationRuntime`.
   */
  enabled?: boolean;
  time?: TimeStatusConfig;
  background?: BackgroundStatusConfig;
}
/**
 * Project-instruction selection from admitted workspace resources.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentProjectInstructionsDocument".
 */
export interface AgentProjectInstructionsDocument {
  /**
   * Whether the invoking generation's normal project instruction chain is
   * prepended to the explicit files.
   */
  inherit?: boolean;
  /**
   * Explicit agent-owned project instruction files, in deterministic
   * configured order. Relative paths resolve against the owning
   * configuration document's directory at launch resolution.
   */
  files?: string[];
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ContextLayer".
 */
export interface ContextLayer {
  reserve_tokens?: string | null;
  keep_recent_tokens?: string | null;
  summary_output_cap?: SummaryOutput | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TimeoutLayer".
 */
export interface TimeoutLayer {
  response_start_timeout_ms?: string | null;
  stream_idle_timeout_ms?: string | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolDeadlineLayer".
 */
export interface ToolDeadlineLayer {
  hard_deadline_ms?: string | null;
  idle_liveness_ms?: IdleLiveness | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SubagentsLayer".
 */
export interface SubagentsLayer {
  max_concurrent?: number | null;
}
/**
 * Explicit native policy axes, resolved over that tool's product default.
 * Absence never applies the generic external-tool policy to a native tool.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "NativePolicyOverrideDocument".
 */
export interface NativePolicyOverrideDocument {
  /**
   * Foreground/background ownership override.
   */
  execution?: 'foreground_only' | 'background_only' | 'model_selectable';
  /**
   * In-batch scheduling override.
   */
  concurrency?: 'sequential' | 'parallel';
  /**
   * Tool approval override.
   */
  approval?: 'never' | 'always';
}
/**
 * One tool invocation policy document.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InvocationPolicyDocument".
 */
export interface InvocationPolicyDocument {
  /**
   * Foreground/background ownership policy.
   */
  execution?: 'foreground_only' | 'background_only' | 'model_selectable';
  /**
   * In-batch scheduling policy.
   */
  concurrency?: 'sequential' | 'parallel';
  /**
   * Human approval behavior for otherwise eligible calls.
   */
  approval?: 'never' | 'always';
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AppServerPolicy".
 */
export interface AppServerPolicy {
  max_resident_runtimes?: number;
  max_connections?: number;
  max_external_attachments?: number;
  idle_grace_ms?: number;
  shutdown_deadline_ms?: number;
}
/**
 * Strict Agent Profile authoring shared by root and named Agents.
 *
 * Complete selected intent resolves against admitted resources. Execution
 * scope controls child lifecycle applicability. Dynamic child
 * invocations may replace only Tools, Skills and Extensions within their
 * frozen delegation ceiling; the other dimensions remain authored defaults.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentProfileDocument".
 */
export interface AgentProfileDocument {
  agents?: SubagentName[];
  workflows?: WorkflowId[];
  /**
   * The bounded model-facing routing description. Empty defaults stay omitted
   * in native source projections and complete-resource serialization.
   */
  description?: string;
  /**
   * Explicit primary Agent instructions authored as TOML data.
   */
  instructions?: string;
  /**
   * The explicit model this agent runs on. Omit to inherit the invoking
   * attempt's frozen effective model configuration.
   */
  model?: ModelLayer | null;
  /**
   * The optional maximum wall-clock duration of the complete child
   * lifecycle, in milliseconds. The model cannot override or extend it.
   */
  timeout_ms?: string | null;
  tools?: ToolSelectionDocument1;
  /**
   * Skill descriptions advertised in the prompt: "all", exact names, or [].
   */
  skills?: AgentSkillSelection | null;
  agents_md?: AgentProjectInstructionsDocument1;
  plugins?: NativeAgentExtensionsDocument;
}
/**
 * The exact source-qualified capability selection.
 */
export interface ToolSelectionDocument1 {
  builtin?: string[];
  sources?: {
    [k: string]: SourceToolSelection;
  };
}
/**
 * Project-instruction selection from admitted workspace resources.
 */
export interface AgentProjectInstructionsDocument1 {
  /**
   * Whether the invoking generation's normal project instruction chain is
   * prepended to the explicit files.
   */
  inherit?: boolean;
  /**
   * Explicit agent-owned project instruction files, in deterministic
   * configured order. Relative paths resolve against the owning
   * configuration document's directory at launch resolution.
   */
  files?: string[];
}
/**
 * Closed native Extension composition. Omission selects none. Root
 * product defaults are an explicit lower-priority authoring layer,
 * independent of this complete document's semantics.
 */
export interface NativeAgentExtensionsDocument {
  agent_status?: AgentStatusExtensionDocument2;
  todo?: TodoExtensionDocument2;
  goal?: GoalExtensionDocument1;
}
/**
 * The authored Agent Status extension.
 *
 * `enabled` composes the extension in or out of the runtime as a whole;
 * `time` and `background` remain the two bounded status contributors, with
 * exactly the semantics they had before the migration.
 */
export interface AgentStatusExtensionDocument2 {
  /**
   * Whether this composition includes the Agent Status extension at all.
   *
   * With `false` the runtime composes no status engine: the Agent Loop
   * emits no Agent Status and is otherwise a completely ordinary
   * `ConversationRuntime`.
   */
  enabled?: boolean;
  time?: TimeStatusConfig;
  background?: BackgroundStatusConfig;
}
/**
 * The authored Todo extension (Issue #259).
 *
 * Todo is *one* capability with several faces, and `enabled` composes all
 * of them together or none of them:
 *
 * ```text
 * enabled = true    conversation-owned ConversationTodoList authority
 *                   the model-facing `todo` Tool
 *                   the bounded read-only Todo status presentation
 *                   the Runtime Client / TUI Todo projection
 *
 * enabled = false   none of the above for this runtime; canonical history
 *                   keeps every Todo ToolCall/ToolResult it already holds
 * ```
 *
 * It carries no contributor settings today. That is a statement about the
 * extension, not a placeholder: the list's bounds, transitions, and
 * dependency rules are owned by
 * [`ConversationTodoList`](crate::tools::todo::ConversationTodoList) and are
 * not launch configuration.
 */
export interface TodoExtensionDocument2 {
  /**
   * Whether this composition includes the Todo extension at all.
   */
  enabled?: boolean;
}
/**
 * Authored opt-in Goal composition.
 */
export interface GoalExtensionDocument1 {
  enabled?: boolean;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionSettingsRef".
 */
export interface SessionSettingsRef {
  revision: string;
  byte_length: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionApplicationRef".
 */
export interface SessionApplicationRef {
  version: string;
  revision: string;
  byte_length: number;
}
/**
 * Source attempt identity is independent of both content and execution version.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ApplicationIdentity".
 */
export interface ApplicationIdentity {
  /**
   * None means immutable input capture failed; no source revision is fabricated.
   */
  input_revision?: string | null;
  attempt: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "Success".
 */
export interface Success {
  jsonrpc: JsonRpcVersion;
  id: AppServerRequestId;
  result: MethodResult;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduledWorkflowRecovery".
 */
export interface ScheduledWorkflowRecovery {
  start: ScheduledWorkflowHead;
  effects: ScheduledWorkflowEffect[];
  result?: unknown;
  next_effect_ordinal?: string | null;
  capacity_blocked: boolean;
  deletion_blocked: boolean;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduledWorkflowHead".
 */
export interface ScheduledWorkflowHead {
  id: ScheduledWorkflowStartId;
  run_id: WorkflowRunId;
  trigger_id: ScheduleTriggerId;
  binding: ScheduleBinding;
  accepted_at: string;
  state: ScheduledWorkflowState;
  cancellation_requested_at?: string | null;
  recovery_blocked?: string | null;
  native_quiescence_proven: boolean;
  result_digest?: string | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleTriggerId".
 */
export interface ScheduleTriggerId {
  schedule_id: ScheduleId;
  admission_revision: string;
  occurrence_utc: string;
}
/**
 * Native destination; changing it requires a new schedule.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleBinding".
 */
export interface ScheduleBinding {
  session_id: SessionId;
  node_id: SessionNodeId;
  conversation_id: ConversationId;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduledWorkflowEffect".
 */
export interface ScheduledWorkflowEffect {
  ordinal: string;
  node: WorkflowNodeInstance;
  kind: ScheduledEffectKind;
  owner: ScheduledEffectOwner;
  physical_settlement_proven: boolean;
  diagnostic?: string | null;
}
/**
 * Bounded product list projection. Reminder text is read through exact Get.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleHeader".
 */
export interface ScheduleHeader {
  id: ScheduleId;
  binding: ScheduleBinding;
  revision: string;
  title: string;
  target: ScheduleTargetHeader;
  spec: ScheduleSpec;
  state: ScheduleState;
  next_due_utc?: string | null;
  last_trigger?: ScheduleTriggerReceipt | null;
  completed_at?: string | null;
  completion_reason?: ScheduleCompletionReason | null;
  last_failure?: string | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleTriggerReceipt".
 */
export interface ScheduleTriggerReceipt {
  trigger_id: ScheduleTriggerId;
  accepted_at: string;
  target: ScheduledTargetReceipt;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleSnapshot".
 */
export interface ScheduleSnapshot {
  id: ScheduleId;
  binding: ScheduleBinding;
  revision: string;
  title: string;
  target: ScheduleTarget;
  spec: ScheduleSpec;
  state: ScheduleState;
  next_due_utc?: string | null;
  last_trigger?: ScheduleTriggerReceipt | null;
  completed_at?: string | null;
  completion_reason?: ScheduleCompletionReason | null;
  last_failure?: string | null;
}
/**
 * Compact committed command result, retained independently of display history.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleCommandReceipt".
 */
export interface ScheduleCommandReceipt {
  request_budget?: ScheduleRequestBudget | null;
  id: ScheduleId;
  revision: string;
  spec: ScheduleSpec;
  state: ScheduleState;
  next_due_utc?: string | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ScheduleRequestBudget".
 */
export interface ScheduleRequestBudget {
  human_message_id: MessageId;
  original_attempt_id: AttemptId;
  mutations_used: number;
  mutation_limit: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AuxiliarySubmissionReceipt".
 */
export interface AuxiliarySubmissionReceipt {
  call_id: AuxiliaryCallId;
  correlation: CorrelationId;
  scope: AuxiliaryAttribution;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AuxiliaryStatus".
 */
export interface AuxiliaryStatus {
  call_id: AuxiliaryCallId;
  executing: boolean;
  inference_stopped: boolean;
  outcome?: AuxiliaryCallOutcome | null;
  settlement_confirmed: boolean;
  result: ResultAvailability;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AuxiliaryResult".
 */
export interface AuxiliaryResult {
  call_id: AuxiliaryCallId;
  final_request_id?: RequestId | null;
  execution?: AuxiliaryCallOutcome | null;
  value: ConsumerResult;
}
/**
 * An explicit delivery fact. Bytes and the leaf inode remain mutable.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionFileReference".
 */
export interface SessionFileReference {
  scope: SessionFileScope;
  /**
   * Normalized relative path under that root, never an `ArtifactId`.
   */
  path: string;
  /**
   * Original filename, including spaces and Unicode.
   */
  name: string;
  /**
   * Bounded optional user-facing description.
   */
  description?: string | null;
  /**
   * Inert viewer classification. It confers no filesystem authority.
   */
  mime_type: string;
}
/**
 * Immutable historical resolution scope.
 */
export interface SessionFileScope {
  /**
   * Original Conversation. Copies retain this identity verbatim.
   */
  conversation_id: string;
  /**
   * Device/inode identity of the authorized root, encoded losslessly.
   */
  device: string;
  /**
   * Root inode; never silently rebound after root replacement.
   */
  inode: string;
}
/**
 * Resolve path against the selected App Server's HTTP(S) origin. Only an
 * owned stdio child supplies a loopback port; never a server filesystem path.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ArchiveDownloadDescriptor".
 */
export interface ArchiveDownloadDescriptor {
  path: string;
  filename: string;
  expires_in_seconds: number;
  loopback_port?: number | null;
}
/**
 * Inclusive Journal/transcript bounds a read was captured at, plus its semantic mutation epoch.
 */
export interface ConversationReadCut1 {
  conversation_id: ConversationId;
  journal: string;
  transcript: string;
  /**
   * Advances on every edit/removal/adoption of a mutable pending transcript body.
   */
  mutation_revision: string;
  /**
   * Immutable canonical Ledger frontier at this same native transaction cut.
   */
  ledger_through: string;
  /**
   * Native model-context structure revision, separate from canonical history.
   */
  surface_revision: string;
  /**
   * Actual retained transcript count, distinct from its positional frontier.
   */
  transcript_total: string;
  /**
   * Native acceptance frontier, including pending Context without transcript positions.
   */
  pending_through: string;
  /**
   * Exact pending count at this cut, separate from transcript collection size.
   */
  pending_total: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UploadDescriptor".
 */
export interface UploadDescriptor {
  path: string;
  loopback_port?: number | null;
  expires_in_seconds: number;
}
/**
 * A successful ordered file allocation. Paths are a presentation of ownership,
 * never input authority or canonical message identity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UploadedFile".
 */
export interface UploadedFile {
  receipt: UploadReceipt;
  file: UploadedFileRef;
  path: string;
}
/**
 * Runtime-authored identity of a Session-owned mutable workspace file.
 * The owning Session supplies allocation roots; history never stores host paths.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UploadedFileRef".
 */
export interface UploadedFileRef {
  batch_id: string;
  name: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ServerDiagnostics".
 */
export interface ServerDiagnostics {
  lifecycle: ServerLifecycle;
  policy: AppServerPolicy;
  loaded: number;
  loading: number;
  unloading: number;
  active_roots: number;
  external_attachments: number;
  sessions: SessionResidencyDiagnostic[];
  admission_refusals: {
    [k: string]: string;
  };
  shutdown_failures: string;
  shutdown_timeouts: string;
  unload_failures: string;
  transport: TransportDiagnostics;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionResidencyDiagnostic".
 */
export interface SessionResidencyDiagnostic {
  session_id: SessionId;
  conversation_id: ConversationId;
  residency: ResidencyState;
  incarnation?: RuntimeIncarnationId | null;
  external_attachments: number;
  operations: number;
  active_root: boolean;
  idle_for_ms?: string | null;
  idle_remaining_ms?: string | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TransportDiagnostics".
 */
export interface TransportDiagnostics {
  websocket_connections: number;
  stdio_connections: number;
  connection_refusals: string;
  delivery_failures: string;
  max_message_bytes: number;
  outbound_queue_messages: number;
  outbound_queue_bytes: number;
  in_flight_requests: number;
  write_deadline_ms: string;
}
/**
 * Public metadata for one committed compaction.
 *
 * Every field is derived from an actual local checkpoint commit. The
 * independent projection disclosure carries summary content; canonical
 * execution rows never acquire a fabricated summary identity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientCompactionView".
 */
export interface RuntimeClientCompactionView {
  /**
   * The local committed projection generation.
   */
  generation: string;
  /**
   * Immutable historical context content, independent of canonical message identity.
   */
  checkpoint_id: string;
  /**
   * Checkpoint activation order, independent of the Ledger and Journal domains.
   */
  activation_id: string;
  tokens_before: TokenMeasurement;
  /**
   * The deterministic estimate of the rebuilt request context.
   */
  estimated_tokens_after: string;
}
/**
 * The pre-compaction input measurement and its provenance.
 */
export interface TokenMeasurement {
  /**
   * The measured or estimated input token count.
   */
  input_tokens: string;
  /**
   * How the measurement was obtained.
   */
  source: 'provider_reported' | 'provider_anchored' | 'estimated';
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ProjectionPage".
 */
export interface ProjectionPage {
  records: RuntimeClientProjectionHeadView[];
  next_activation?: ProjectionActivationId | null;
}
/**
 * Projection ownership is independent of the local execution trajectory.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientProjectionHeadView".
 */
export interface RuntimeClientProjectionHeadView {
  checkpoint_id: CheckpointId;
  activation_id: ProjectionActivationId;
  /**
   * Insert the historical marker after this native transcript position.
   * Captured at activation; independent of Ledger and Journal coordinates.
   */
  transcript_anchor: string;
  base_frontier: LedgerFrontier;
  activation_ledger: LedgerFrontier;
  origin: OriginActivation;
  destination_installation?: LineageInstallationId | null;
  original_items: number;
  derived_items: number;
  summary_preview?: TraceText | null;
}
/**
 * Immutable source activation. Destination installation is a different coordinate.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "OriginActivation".
 */
export interface OriginActivation {
  conversation_id: ConversationId;
  activation: ProjectionActivationId;
  completed_journal: JournalFrontier;
}
/**
 * Bounded text with an explicit completeness statement.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceText".
 */
export interface TraceText {
  text: string;
  truncated: boolean;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ProjectionDetail".
 */
export interface ProjectionDetail {
  read_cut: CheckpointReadCut;
  checkpoint: RuntimeClientProjectionHeadView;
  items: ProjectionItemDetail[];
  next_item?: number | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ProjectionItemDetail".
 */
export interface ProjectionItemDetail {
  occurrence: CheckpointMessageOccurrence;
  content: TraceRequestMessage;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CheckpointMessageOccurrence".
 */
export interface CheckpointMessageOccurrence {
  checkpoint_id: CheckpointId;
  item_index: number;
}
/**
 * One reconstructed provider-neutral request item.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceRequestMessage".
 */
export interface TraceRequestMessage {
  /**
   * Present only for one actual frozen request; projection detail has its own coordinate.
   */
  request_occurrence?: RequestMessageOccurrence | null;
  provenance: TraceInputProvenance;
  provenance_source?: TraceFieldSource | null;
  role: TraceMessageRole;
  /**
   * Canonical Ledger identity; absent for Derived and request-only input.
   */
  message_id?: MessageId | null;
  /**
   * User provenance or the request-local Derived transformation label.
   */
  source?: string | null;
  blocks: TraceContentBlock[];
  truncated: boolean;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RequestMessageOccurrence".
 */
export interface RequestMessageOccurrence {
  request_id: RequestId;
  item_index: number;
}
/**
 * Immutable source scope, distinct from a projected occurrence.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CanonicalMessageRef".
 */
export interface CanonicalMessageRef {
  conversation_id: ConversationId;
  message_id: MessageId;
  /**
   * Integrity of the complete immutable body, independent of semantic request hashing.
   */
  content_digest: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ContentBinding".
 */
export interface ContentBinding {
  /**
   * Projection-owned structural selection inside `CanonicalJsonV1` bytes.
   */
  source_path?: string[] | null;
  path: string[];
  content_ref: ContentRef;
  deferred: boolean;
}
/**
 * Scoped immutable projection provenance, used when a summary consumes an earlier summary.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CheckpointSourceRef".
 */
export interface CheckpointSourceRef {
  conversation_id: ConversationId;
  checkpoint_id: CheckpointId;
  content_digest: string;
}
/**
 * Bounded structured data with an explicit completeness statement.
 *
 * The value stays real JSON rather than a rendered string, so the browser
 * can present it with a structured reader instead of a preformatted dump.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceJson".
 */
export interface TraceJson {
  value: unknown;
  truncated: boolean;
}
/**
 * Safe reference to the existing native artifact carrier, never a path.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceArtifact".
 */
export interface TraceArtifact {
  artifact_id: ArtifactId;
  image: boolean;
  name?: string | null;
  mime_type?: string | null;
}
/**
 * One finite page of bounded summary records, oldest first.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TracePage".
 */
export interface TracePage {
  read_cut?: CheckpointReadCut | null;
  /**
   * Current-cut repairs for explicitly requested loaded identities.
   */
  updates?: TraceLifecycle[];
  records: TraceRecord[];
  next_cursor?: TraceCursor | null;
}
/**
 * Refresh of an already loaded record, resolved at the server's snapshot cut.
 *
 * Mutable lifecycle and resolved native location travel here. Immutable historical input is
 * never repeated, because it cannot have changed.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceLifecycle".
 */
export interface TraceLifecycle {
  fields: TraceFieldSource[];
  collections: CollectionFrontier[];
  id: string;
  location: TraceLocation;
  state: TraceState;
  timing: TraceTiming;
  request?: TraceRequestOutcome | null;
  tool?: TraceToolOutcomeUpdate | null;
  message_id?: MessageId | null;
  attachments: TraceArtifact[];
  truncated: boolean;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CollectionFrontier".
 */
export interface CollectionFrontier {
  domain: string;
  revision: string;
  total: string;
  loaded: string;
  next_offset?: string | null;
}
/**
 * Current native ownership, including an idle input's answering Attempt.
 */
export interface TraceLocation {
  attempt_id?: AttemptId | null;
  attempt_id_source?: AttemptIdentitySource | null;
  step_id?: TurnId | null;
  step_id_source?: TraceFieldSource | null;
}
/**
 * An exact immutable native Journal field authorized by the checkpoint read cut.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "JournalFieldRef".
 */
export interface JournalFieldRef {
  sequence: number;
  path: string[];
  encoded_bytes: number;
}
/**
 * Two authoritative instants, or fewer.
 *
 * `duration_ms` exists exactly when both endpoints do. Receipt time, render
 * time and reconnect time are never endpoints.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceTiming".
 */
export interface TraceTiming {
  started_at: string;
  ended_at?: string | null;
  duration_ms?: string | null;
}
/**
 * Mutable request outcome only.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceRequestOutcome".
 */
export interface TraceRequestOutcome {
  failure_kind?: ModelErrorKind | null;
  usage?: ModelUsage | null;
  generation?: TraceGeneration | null;
}
/**
 * Normalized token accounting for one generation.
 *
 * Providers do not expose identical token metrics; this is the stable
 * common core. Provider SDK usage objects never appear here.
 * Missing provider metrics remain absent; a reported zero is `Some(0)`.
 * These are native u64 quantities, not byte offsets. Observation transports
 * use the existing exact decimal codec without narrowing provider evidence.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelUsage".
 */
export interface ModelUsage {
  /**
   * Input tokens consumed by the request.
   */
  input_tokens?: string | null;
  /**
   * Output tokens produced by the response.
   */
  output_tokens?: string | null;
  /**
   * Total tokens, where the provider reports or can derive them.
   */
  total_tokens?: string | null;
  /**
   * Optional normalized usage details.
   */
  details?: UsageDetails | null;
}
/**
 * Optional normalized token details where providers expose them.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UsageDetails".
 */
export interface UsageDetails {
  /**
   * Tokens consumed by reasoning.
   */
  reasoning_tokens?: string | null;
  /**
   * Input tokens served from cache.
   */
  cached_input_tokens?: string | null;
}
/**
 * Derived generation metrics for one actual request.
 *
 * Every field is `None` unless its authoritative endpoints exist. A missing
 * metric states that rustX did not observe what the metric measures.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceGeneration".
 */
export interface TraceGeneration {
  /**
   * Request-relative monotonic phase positions, only with a native clock bridge.
   */
  timeline?: TraceGenerationTimeline | null;
  /**
   * Milliseconds from the request's dispatch frontier to its first output.
   */
  ttft_ms?: string | null;
  /**
   * Milliseconds from the first output to the provider terminal.
   */
  generation_ms?: string | null;
  /**
   * Milliseconds from the dispatch frontier to the provider terminal.
   */
  terminal_ms: string;
  /**
   * Output tokens per second, present only when usage and both decode
   * endpoints exist and the decode span is measurable.
   */
  output_tokens_per_second?: number | null;
}
/**
 * All offsets share the paired durable request-start origin. The browser must
 * not stretch them to fit the independently recorded Journal wall duration.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceGenerationTimeline".
 */
export interface TraceGenerationTimeline {
  dispatch_ms: string;
  first_output_ms?: string | null;
  last_output_ms?: string | null;
  terminal_ms: string;
}
/**
 * Mutable Tool outcome only.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolOutcomeUpdate".
 */
export interface TraceToolOutcomeUpdate {
  started: boolean;
  outcome?: TraceToolOutcome | null;
  detail?: TracePreview | null;
}
/**
 * A one-line summary preview of longer content.
 *
 * Whitespace is collapsed so a multi-paragraph message occupies one ledger
 * row. This is presentation shaping of already-permitted content, not a
 * second copy of it: the complete value is reached through detail.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TracePreview".
 */
export interface TracePreview {
  text: string;
  truncated: boolean;
}
/**
 * One bounded pageable ledger record.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceRecord".
 */
export interface TraceRecord {
  fields: TraceFieldSource[];
  collections: CollectionFrontier[];
  /**
   * Stable native Trace identity. Selection and detail reads use it, and
   * it survives paging, prepending and reconnect for the same record.
   */
  id: string;
  /**
   * Opaque Trace-only exclusive boundary, valid only in its conversation.
   *
   * It is not a live cursor, a transcript cursor, an Event Journal sequence,
   * a Request ID, or an artifact ID. Clients order by it and page with it;
   * they never parse it.
   */
  position: string;
  location: TraceLocation1;
  kind: TraceKind;
  state: TraceState;
  timing: TraceTiming;
  /**
   * One-line content preview, so a ledger row carries meaning rather than
   * an identity alone.
   */
  preview?: TracePreview | null;
  request?: TraceRequestSummary | null;
  tool?: TraceToolSummary | null;
  /**
   * Canonical `ToolCall` proposals of an Assistant record, in block order.
   */
  calls: TraceToolCall[];
  /**
   * Exact native detached execution / Subagent / Workflow / interaction ID.
   */
  native_id?: string | null;
  agent_id?: AgentId | null;
  activation_id?: SubagentId | null;
  /**
   * Frozen admission source; client controls never invent a model Tool call.
   */
  activation_origin?: AgentActivationOrigin | null;
  /**
   * The exact outer `ToolCall` this Tool-owned domain record belongs to,
   * copied from the native start fact. It is presentation and navigation
   * correlation only: it confers no lifecycle, ownership, settlement or
   * cancellation authority, and it is never resolved from a Tool name, a
   * timestamp, row adjacency or the loaded page.
   */
  originating_tool_call_id?: ToolCallId | null;
  /**
   * Canonical accepted message; publication without acceptance is absent.
   */
  message_id?: MessageId | null;
  attachments: TraceArtifact[];
  /**
   * Whether this record has heavy detail available for inspection.
   */
  has_detail: boolean;
  truncated: boolean;
}
/**
 * Server-resolved native grouping of one record.
 *
 * `TurnId` is the logical model step inside an Attempt. An actual request
 * retry never allocates a new step, so retries of one step group together.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceLocation".
 */
export interface TraceLocation1 {
  attempt_id?: AttemptId | null;
  attempt_id_source?: AttemptIdentitySource | null;
  step_id?: TurnId | null;
  step_id_source?: TraceFieldSource | null;
}
/**
 * Bounded request facts carried by a pageable summary row.
 *
 * The historical request input — system prompt, reconstructed context, Tool
 * definitions — is deliberately absent. It lives in detail so a page of 32
 * requests does not carry 32 complete model contexts.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceRequestSummary".
 */
export interface TraceRequestSummary {
  accepted_message_locator?: CanonicalContentLocator | null;
  request_id?: RequestId | null;
  request_id_source?: TraceFieldSource | null;
  /**
   * Native actual-request ordinal within the logical step. Zero is the
   * initial request; retry and recovery requests continue the sequence.
   * It is read from the frozen snapshot, never inferred from timestamps.
   */
  retry_number: number;
  assistant_message_id?: MessageId | null;
  assistant_message_id_source?: TraceFieldSource | null;
  /**
   * Historical model, from the request's own immutable snapshot.
   */
  model?: string | null;
  model_source?: TraceFieldSource | null;
  /**
   * The exact preceding actual request's failure class, when recorded.
   */
  previous_failure_kind?: ModelErrorKind | null;
  failure_kind?: ModelErrorKind | null;
  usage?: ModelUsage | null;
  generation?: TraceGeneration | null;
  system_prompt: TraceSystemPromptPresentation;
  predecessor: TraceRequestPredecessor;
  tool_catalog: TraceToolCatalogState;
  /**
   * Canonical request Context this exact request introduced, in the order
   * frozen by `RequestSnapshot.request_context_ids`. A retry or recovery
   * request reuses admitted context and therefore introduces none.
   */
  context_additions: TraceContextPresentation[];
  /**
   * Whether the Context list was shortened by the Trace summary bound.
   */
  context_truncated: boolean;
  context_total: string;
  context_domain: string;
  context_next_offset?: number | null;
}
/**
 * Request-relative System Prompt presentation, resolved natively so the
 * browser never compares request details to discover a prompt change.
 */
export interface TraceSystemPromptPresentation {
  state: TraceSystemPromptState;
  /**
   * One-line preview of the prompt this request introduced. Absent for
   * `Unchanged`, where the preceding request's row already carries it.
   * A present but empty preview records an empty historical prompt.
   */
  preview?: TracePreview | null;
}
/**
 * One canonical request Context fact introduced by one actual request.
 *
 * Identity and order come from the immutable `RequestSnapshot`; content
 * comes from keyed Message Ledger reads. Neither the browser nor Trace
 * itself decides which request introduced a Context fact: the request that
 * committed the identity atomically with its own start did.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceContextPresentation".
 */
export interface TraceContextPresentation {
  message_id?: MessageId | null;
  locator: CanonicalContentLocator;
  fields: TraceFieldSource[];
  /**
   * Exact producer from the committed accepted-contribution record.
   */
  producer:
    | {
        Native: NativeContextContributor;
      }
    | {
        CertifiedExtension: CertifiedExtensionIdentity;
      };
  context_kind: TraceContextKind;
  /**
   * The exact native producer, copied from the canonical message's own
   * `UserSource`. It is never inferred from the context family, the
   * contributor list, the assembly generation, message order, text, or
   * the current extension registry.
   */
  source:
    | {
        type: 'runtime';
      }
    | {
        contributor: CertifiedExtensionIdentity;
        type: 'certified_extension';
      };
  preview?: TracePreview | null;
  attachments: TraceArtifact[];
  truncated: boolean;
}
/**
 * Bounded Tool facts carried by a pageable summary row.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolSummary".
 */
export interface TraceToolSummary {
  call_id?: ToolCallId | null;
  tool_id?: ToolId | null;
  occurrence?: RuntimeClientToolOccurrence | null;
  fields: TraceFieldSource[];
  /**
   * Recorded model-facing name, when the canonical proposal is loadable.
   */
  name?: string | null;
  /**
   * Bounded canonical arguments; absent only when the exact proposal is unavailable.
   */
  arguments?: TracePreview | null;
  /**
   * Whether execution started, as proven by its own durable start fact.
   */
  started: boolean;
  /**
   * Typed outcome class, present only once the execution settled.
   */
  outcome?: TraceToolOutcome | null;
  /**
   * Bounded typed error or status detail from the canonical result.
   */
  detail?: TracePreview | null;
}
/**
 * One canonical `ToolCall` the model proposed.
 *
 * A proposal proves that the generation assembled a call. It never proves
 * that execution started: a started Tool record is separate evidence.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolCall".
 */
export interface TraceToolCall {
  call_id?: ToolCallId | null;
  tool_id?: ToolId | null;
  name?: string | null;
  occurrence: RuntimeClientToolOccurrence;
  fields: TraceFieldSource[];
}
/**
 * A contiguous native history window ending at the selected execution.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolLocation".
 */
export interface TraceToolLocation {
  record_id: string;
  page: TracePage;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConversationWindow".
 */
export interface ConversationWindow {
  cut: ConversationReadCut;
  page: RuntimeClientTranscriptPage;
  newer_cursor?: RuntimeClientTranscriptCursor | null;
  target?: ObservedTurnIdentity | null;
  target_cursor?: RuntimeClientTranscriptCursor | null;
}
/**
 * One bounded newest-or-older page of derived transcript history.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientTranscriptPage".
 */
export interface RuntimeClientTranscriptPage {
  /**
   * The exact native source cut from which bodies and evidence were read.
   */
  source_cut?: ObservationReadCut | null;
  /**
   * Deferred bytes remain explicitly recoverable on older historical pages.
   */
  content?: ContentBinding[];
  collections?: CollectionFrontier[];
  /**
   * Native items whose structural display requires bounded further reading.
   */
  deferred_entries?: CheckpointPageItem[];
  /**
   * Last native transcript position inherited by this lineage; zero for an empty prefix.
   */
  inherited_through?: RuntimeClientTranscriptCursor | null;
  /**
   * Native whole-conversation totals, independent of this page.
   */
  statistics?: ConversationStatistics | null;
  /**
   * Items in chronological order within this page.
   */
  entries?: RuntimeClientTranscriptEntry[];
  /**
   * The exclusive cursor for the next older page.
   */
  next_cursor?: RuntimeClientTranscriptCursor | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CheckpointPageItem".
 */
export interface CheckpointPageItem {
  /**
   * Whether a finite actionable summary includes its complete request body.
   */
  body_complete?: boolean | null;
  key?: CheckpointKey | null;
  offset: string;
  identity: string;
  value?: unknown;
  content: ContentBinding[];
  deferred: boolean;
}
/**
 * Original JSON key evidence for a field selected by a bounded ordinal locator.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CheckpointKey".
 */
export interface CheckpointKey {
  path: string[];
  preview: string;
  encoded_bytes: number;
}
/**
 * Whole-conversation execution totals, independent of any transcript window.
 * A lineage child includes its inherited turns' recorded execution, as a
 * `DeepSeek` Harness fork folds its copied prefix; it owns none of it.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConversationStatistics".
 */
export interface ConversationStatistics {
  /**
   * Harness-style Turn and Step counts: native Attempt starts and Loop turns.
   */
  turns: string;
  steps: string;
  /**
   * Latest native Turn clock; never a browser receipt timestamp.
   */
  latest_turn?: ConversationTurnClock | null;
  /**
   * Summed measured work time, separate from usage coverage.
   */
  timing?: ConversationTiming | null;
  completed_responses: string;
  model_requests: string;
  /**
   * Known reported usage. Coverage is explicit; missing reports are not zero.
   */
  requests_with_usage: string;
  reported_usage?: ModelUsage | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConversationTurnClock".
 */
export interface ConversationTurnClock {
  attempt_id?: AttemptId | null;
  attempt_id_deferred: boolean;
  attempt_id_source?: JournalFieldRef | null;
  started_at: string;
  ended_at?: string | null;
}
/**
 * Whole-conversation work time. Unlike a completed response's exact
 * aggregate, each figure sums only the requests or Tool executions that
 * measured it, and is absent until the first one did.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConversationTiming".
 */
export interface ConversationTiming {
  /**
   * Summed dispatch-to-provider-terminal time of measured requests.
   */
  model_ms?: number | null;
  /**
   * Summed start-to-terminal time of settled foreground Tool executions.
   */
  tool_ms?: number | null;
  /**
   * Mean dispatch-to-first-output time over requests that produced output.
   */
  mean_ttft_ms?: number | null;
  /**
   * Output tokens over decode time, summed over requests that report
   * usage and a positive decode span.
   */
  output_tokens_per_second?: number | null;
}
/**
 * One derived transcript item and its stable durable cursor.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientTranscriptEntry".
 */
export interface RuntimeClientTranscriptEntry {
  /**
   * Exact native Attempt process membership, derived by native owners.
   * Absence is not permission for a client to infer a process boundary.
   */
  turn_process?: TurnProcessView | null;
  /**
   * The native Attempt has not yet settled this accepted Assistant candidate.
   * A client retaining this row outside a refresh must reread it, not freeze absence.
   */
  response_pending?: boolean;
  /**
   * Exact completed-response evidence, never inferred by clients.
   */
  completed_response?: CompletedResponseView | null;
  /**
   * Canonical calls in block order with their native committed results.
   * A missing result means no committed result, never success or cancellation.
   */
  tool_calls?: ForegroundToolExecution[];
  /**
   * The cursor domain of durable transcript paging.
   */
  cursor: string;
  /**
   * The typed item resolved from a canonical durable owner.
   */
  item:
    | {
        turn: TurnProcessView;
        type: 'attempt_terminal';
      }
    | {
        /**
         * The canonical or durably accepted message.
         */
        message:
          | (UserMessageBlock & {
              role: 'user';
            })
          | (AssistantMessageBlock & {
              role: 'assistant';
            })
          | (ToolMessageBlock & {
              role: 'tool';
            });
        type: 'message';
      }
    | {
        audit: PublicationAudit;
        type: 'publication_audit';
      }
    | {
        /**
         * Identifies one durable runtime event.
         */
        event_id: string;
        /**
         * Event timestamp.
         */
        timestamp: string;
        /**
         * Owning Attempt; absent for native Scheduled work.
         */
        attempt_id?: AttemptId | null;
        /**
         * Owning turn; absent for native Scheduled work.
         */
        turn_id?: TurnId | null;
        /**
         * Interaction identity.
         */
        interaction_id: string;
        /**
         * Bounded durable subject.
         */
        subject:
          | {
              review: ReviewSpecification;
              type: 'review';
            }
          | {
              /**
               * Caller correlation for one native invocation. Capability source is
               * independently represented by [`ToolOrigin`].
               */
              invocation_id:
                | {
                    call_id: ToolCallId;
                    caller: 'agent';
                  }
                | {
                    node: WorkflowNodeInstance;
                    caller: 'workflow';
                  };
              /**
               * Identifies a tool definition in the capability set.
               */
              tool_id: string;
              /**
               * The model-facing tool name.
               */
              tool_name: string;
              /**
               * The digest of the canonical model-issued arguments.
               */
              arguments_digest: string;
              /**
               * The bounded policy explanation shown to the client.
               */
              reason: string;
              type: 'approval';
            }
          | {
              invocation_id: ToolInvocationId;
              requester: InteractionRequester;
              questionnaire: QuestionnaireSpecification;
              type: 'questionnaire';
            };
        type: 'interaction_requested';
      }
    | {
        /**
         * Identifies one durable runtime event.
         */
        event_id: string;
        /**
         * Event timestamp.
         */
        timestamp: string;
        /**
         * Owning Attempt; absent for native Scheduled work.
         */
        attempt_id?: AttemptId | null;
        /**
         * Owning turn; absent for native Scheduled work.
         */
        turn_id?: TurnId | null;
        /**
         * Interaction identity.
         */
        interaction_id: string;
        /**
         * Bounded durable settlement.
         */
        settlement:
          | {
              response: ReviewResponse;
              type: 'reviewed';
            }
          | {
              kind: ToolDeadlineKind;
              type: 'deadline_expired';
            }
          | {
              type: 'approved';
            }
          | {
              /**
               * The bounded client-facing denial reason.
               */
              reason: string;
              type: 'denied';
            }
          | {
              submission: QuestionnaireSubmission1;
              type: 'questionnaire_submitted';
            }
          | {
              type: 'questionnaire_declined';
            }
          | {
              /**
               * The first-winner cancellation cause.
               */
              reason:
                | 'user_requested'
                | 'runtime_shutdown'
                | 'parent_cancelled'
                | 'subagent_execution_deadline_exceeded';
              type: 'cancelled';
            };
        type: 'interaction_settled';
      };
}
/**
 * One native process owner shared by live, successful and unsuccessful Turns.
 * Repeated on each exact member so bounded pages are independently resolvable.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TurnProcessView".
 */
export interface TurnProcessView {
  conversation_id: ConversationId;
  attempt_id?: AttemptId | null;
  attempt_id_source?: AttemptIdentitySource | null;
  /**
   * Original terminal Journal identity, absent while running or inherited.
   */
  event_id?: EventId | null;
  /**
   * The cursor domain of durable transcript paging.
   */
  control_cursor: string;
  final_message_id?: MessageId | null;
  /**
   * Exact final canonical identity, present only for a proven completed response.
   */
  final_message_locator?: CanonicalContentLocator | null;
  final_message_id_deferred: boolean;
  /**
   * Original canonical metadata retains identities omitted from the inline view.
   */
  final_message_id_source?: ContentRef | null;
  message_count: number;
  tool_call_count: number;
  outcome: TurnProcessOutcome;
  /**
   * Normalized terminal failure, retained at its durable transcript position.
   */
  failure?: RuntimeClientAttemptFailure | null;
  started_at?: string | null;
  ended_at?: string | null;
}
/**
 * Derived response view over local execution or inherited lineage provenance.
 * Canonical content remains in the Message Ledger.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CompletedResponseView".
 */
export interface CompletedResponseView {
  closing_message_id?: MessageId | null;
  closing_locator: CanonicalContentLocator;
  retry_locator?: CanonicalContentLocator | null;
  deferred_fields: string[][];
  origin: ObservedResponseOrigin;
  /**
   * Durable completion timestamp; never a browser receipt time.
   */
  completed_at: string;
  /**
   * The identity of one exact historical Conversation Surface state.
   *
   * A revision is a monotonic counter in its own identity domain. The empty
   * Surface of a new conversation is [`SurfaceRevision::INITIAL`] (`0`), and
   * every accepted [`SurfaceOp`] advances it by exactly one, so revision `n`
   * is precisely "the Surface after the first `n` accepted operations".
   *
   * A revision is deliberately **not** a `MessageId`, an `AttemptId`, a
   * `RuntimeClientCursor`, an `InboundSequence`, an Event Journal sequence,
   * or a `CapabilityRevision`: none of those identify a Surface state, and
   * none of them may be substituted for one.
   */
  surface_revision: string;
  /**
   * Exact adopted ordinary input, when this Attempt has a replayable input.
   */
  retry_message_id?: MessageId | null;
  /**
   * All actual requests in the Attempt, only when every usage is known.
   */
  usage?: ModelUsage | null;
  timing?: CompletedResponseTiming | null;
  /**
   * Distinct provider-facing models of the Attempt's actual requests.
   */
  models?: string[];
}
/**
 * Original execution owner, including for inherited historical responses.
 */
export interface ObservedResponseOrigin {
  conversation_id: ConversationId;
  attempt_id?: AttemptId | null;
  attempt_id_source?: AttemptIdentitySource | null;
  closing_message_id?: MessageId | null;
}
/**
 * Historical product timing derived from native lifecycle and generation evidence.
 * Missing evidence stays absent; these are not destination execution facts.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CompletedResponseTiming".
 */
export interface CompletedResponseTiming {
  /**
   * Authoritative successful Attempt completion minus its start timestamp.
   */
  total_duration_ms?: number | null;
  /**
   * First actual request's adapter-dispatch-to-first-output duration.
   */
  ttft_ms?: number | null;
  /**
   * Sum of output-producing requests' first-output-to-provider-terminal spans.
   */
  generation_ms?: number | null;
  /**
   * Fully covered output usage divided by fully covered positive generation work.
   */
  output_tokens_per_second?: number | null;
}
/**
 * The foreground tool execution read model of one logical tool call.
 *
 * Keyed by the canonical logical tool-call identity, so parallel physical
 * completion timing can never corrupt logical identities or canonical
 * ordering. Native, MCP, and Python foreground executions converge through
 * this one shape.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ForegroundToolExecution".
 */
export interface ForegroundToolExecution {
  occurrence: RuntimeClientToolOccurrence;
  /**
   * Exact inline Assistant identity when bounded; `occurrence` remains authoritative.
   */
  message_id?: MessageId | null;
  /**
   * A stable index identifying one content block within the ordered content
   * list of the canonical message being assembled.
   *
   * Streaming facts (text deltas, reasoning deltas, provider continuation
   * state, tool-call content) reference the block they belong to by this
   * index, so interleaved and multiple blocks remain unambiguous without
   * exposing any provider-specific block id type.
   */
  block_index: number;
  /**
   * The logical tool-call identity.
   */
  call_id?: ToolCallId | null;
  metadata_ref?: ContentRef | null;
  assembly_complete: boolean;
  result_source?: JournalFieldRef | null;
  /**
   * Canonical-only settlement metadata, including before-Start outcomes.
   */
  result_metadata_ref?: ContentRef | null;
  /**
   * A committed canonical result is independent of execution-terminal Journal publication.
   */
  committed_result?: ToolExecutionResult | null;
  /**
   * The committed C result body is available through its exact metadata/payload sources.
   */
  committed_result_deferred: boolean;
  /**
   * The canonical tool identity.
   */
  tool_id?: ToolId | null;
  /**
   * The model-facing tool name at call time.
   */
  name: string;
  /**
   * The externally meaningful execution state.
   */
  state:
    | {
        /**
         * The assembled JSON arguments.
         */
        arguments: string;
        type: 'assembled';
      }
    | {
        /**
         * The assembled JSON arguments.
         */
        arguments: string;
        /**
         * The latest bounded progress, when any.
         */
        progress?: ToolProgress | null;
        type: 'running';
      }
    | {
        /**
         * The assembled JSON arguments.
         */
        arguments: string;
        result: ToolExecutionResult1;
        type: 'settled';
      };
}
/**
 * The normalized outcome of one tool execution.
 *
 * `ToolMessageBlock` composes this type instead of duplicating its fields,
 * keeping one source of truth for tool results.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolExecutionResult".
 */
export interface ToolExecutionResult {
  /**
   * Immutable native Workflow identity on the existing outer result.
   * Historical identity only: no execution state or continuation authority.
   * Its retention is exactly that of this result, never a separate registry.
   */
  workflow?: WorkflowToolIdentity | null;
  /**
   * Typed execution status, including unknown external outcomes.
   */
  status:
    | {
        type: 'success';
      }
    | {
        /**
         * Human-readable error message.
         */
        error: string;
        type: 'failed';
      }
    | {
        /**
         * The policy or human-readable approval reason.
         */
        reason: string;
        type: 'denied';
      }
    | {
        /**
         * Why the execution was cancelled.
         */
        reason:
          | 'user_requested'
          | 'runtime_shutdown'
          | 'parent_cancelled'
          | 'subagent_execution_deadline_exceeded';
        /**
         * Whether cancellation won before executor start or while execution
         * was already in flight.
         */
        phase: 'before_start' | 'during_execution';
        type: 'cancelled';
      }
    | {
        type: 'timed_out';
      }
    | {
        /**
         * A producer-owned diagnostic describing why certainty is
         * unavailable. It is rendered into the bounded model-facing
         * projection and is never parsed to decide semantics; the typed
         * variant itself is the certainty claim.
         */
        detail: string;
        type: 'outcome_unknown';
      };
  /**
   * Tool-owned result content.
   *
   * This content is TOOL-OWNED: [`ToolResultContent::Json`] is arbitrary
   * tool-owned structured data, and the runtime never infers semantics
   * from its property names. rustX reserves no ordinary JSON field names;
   * runtime-owned facts live in the typed fields of this struct. A
   * provider-independent, bounded model-facing representation is produced
   * by [`Self::model_facing_projection`]; producers do not append runtime
   * status or managed-output continuation text here.
   */
  content?: ToolResultContent[];
  /**
   * Execution duration in integer milliseconds (stable for persistence).
   */
  duration_ms: string;
  /**
   * Process exit code where the tool executed a process.
   */
  exit_code?: number | null;
  /**
   * Immutable managed Artifact references produced by the execution.
   */
  artifacts?: FileReference[];
  /**
   * Runtime-owned explicit declarations of mutable native Session files.
   * Only successful canonical Tool results authorize delivery cards.
   */
  deliveries?: SessionFileReference[];
  /**
   * Truncation metadata where output was truncated.
   */
  truncation?: TruncationState | null;
  /**
   * Runtime-owned managed textual-output continuation metadata: where
   * the complete — or honestly partial — textual output of this result
   * lives in the conversation's managed tool-output store (Issue #86).
   * Absent for results whose output fits the model-facing content.
   *
   * This is the one typed source of truth for complete-vs-partial
   * managed output; producers never encode these facts as magic
   * properties of tool-owned JSON, and generic runtime publication code
   * consumes only this typed field, never arbitrary JSON keys.
   */
  managed_output?: ManagedOutputContinuation | null;
}
/**
 * Runtime-owned identity, independent of model `ToolCall` text.
 * Retained with the canonical outer result, not with an execution owner.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowToolIdentity".
 */
export interface WorkflowToolIdentity {
  workflow_id: WorkflowId;
  program_digest: string;
}
/**
 * A reference to a Tool-generated managed file artifact; never a user upload.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "FileReference".
 */
export interface FileReference {
  /**
   * Identifies a durable artifact produced or referenced by the runtime.
   *
   * An artifact is identified by an opaque runtime-owned id, never by a
   * local filesystem path: paths are executor concerns and are not a
   * universal durable artifact identity.
   */
  artifact_id: string;
  /**
   * Optional display name.
   */
  name?: string | null;
  /**
   * Optional MIME type.
   */
  mime_type?: string | null;
  /**
   * Optional human-readable description.
   */
  description?: string | null;
}
/**
 * Truncation metadata for tool output.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TruncationState".
 */
export interface TruncationState {
  /**
   * Whether the result content was truncated.
   */
  truncated: boolean;
  /**
   * Size of the untruncated output in bytes, when known.
   */
  original_bytes?: string | null;
}
/**
 * A bounded structured progress notification of one tool execution.
 *
 * Progress is an execution fact, never canonical message history. All
 * fields are optional; an empty `ToolProgress` is a bare tick. The progress
 * message text is bounded by [`MAX_PROGRESS_MESSAGE_BYTES`].
 *
 * [`MAX_PROGRESS_MESSAGE_BYTES`]: crate::tools::limits::MAX_PROGRESS_MESSAGE_BYTES
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolProgress".
 */
export interface ToolProgress {
  /**
   * A short human-readable progress message, when there is one.
   */
  message?: string | null;
  /**
   * Completed units, when a total is known.
   */
  completed?: number | null;
  /**
   * Total units, when known.
   */
  total?: number | null;
}
/**
 * The normalized outcome of one tool execution.
 *
 * `ToolMessageBlock` composes this type instead of duplicating its fields,
 * keeping one source of truth for tool results.
 */
export interface ToolExecutionResult1 {
  /**
   * Immutable native Workflow identity on the existing outer result.
   * Historical identity only: no execution state or continuation authority.
   * Its retention is exactly that of this result, never a separate registry.
   */
  workflow?: WorkflowToolIdentity | null;
  /**
   * Typed execution status, including unknown external outcomes.
   */
  status:
    | {
        type: 'success';
      }
    | {
        /**
         * Human-readable error message.
         */
        error: string;
        type: 'failed';
      }
    | {
        /**
         * The policy or human-readable approval reason.
         */
        reason: string;
        type: 'denied';
      }
    | {
        /**
         * Why the execution was cancelled.
         */
        reason:
          | 'user_requested'
          | 'runtime_shutdown'
          | 'parent_cancelled'
          | 'subagent_execution_deadline_exceeded';
        /**
         * Whether cancellation won before executor start or while execution
         * was already in flight.
         */
        phase: 'before_start' | 'during_execution';
        type: 'cancelled';
      }
    | {
        type: 'timed_out';
      }
    | {
        /**
         * A producer-owned diagnostic describing why certainty is
         * unavailable. It is rendered into the bounded model-facing
         * projection and is never parsed to decide semantics; the typed
         * variant itself is the certainty claim.
         */
        detail: string;
        type: 'outcome_unknown';
      };
  /**
   * Tool-owned result content.
   *
   * This content is TOOL-OWNED: [`ToolResultContent::Json`] is arbitrary
   * tool-owned structured data, and the runtime never infers semantics
   * from its property names. rustX reserves no ordinary JSON field names;
   * runtime-owned facts live in the typed fields of this struct. A
   * provider-independent, bounded model-facing representation is produced
   * by [`Self::model_facing_projection`]; producers do not append runtime
   * status or managed-output continuation text here.
   */
  content?: ToolResultContent[];
  /**
   * Execution duration in integer milliseconds (stable for persistence).
   */
  duration_ms: string;
  /**
   * Process exit code where the tool executed a process.
   */
  exit_code?: number | null;
  /**
   * Immutable managed Artifact references produced by the execution.
   */
  artifacts?: FileReference[];
  /**
   * Runtime-owned explicit declarations of mutable native Session files.
   * Only successful canonical Tool results authorize delivery cards.
   */
  deliveries?: SessionFileReference[];
  /**
   * Truncation metadata where output was truncated.
   */
  truncation?: TruncationState | null;
  /**
   * Runtime-owned managed textual-output continuation metadata: where
   * the complete — or honestly partial — textual output of this result
   * lives in the conversation's managed tool-output store (Issue #86).
   * Absent for results whose output fits the model-facing content.
   *
   * This is the one typed source of truth for complete-vs-partial
   * managed output; producers never encode these facts as magic
   * properties of tool-owned JSON, and generic runtime publication code
   * consumes only this typed field, never arbitrary JSON keys.
   */
  managed_output?: ManagedOutputContinuation | null;
}
/**
 * Inbound information supplied to the current agent.
 *
 * A `UserMessageBlock` does not necessarily mean a human spoke: it is the
 * canonical home for anything inbound, including messages from other agents
 * (with [`UserSource::Agent`] provenance). Compaction summaries instead belong
 * to immutable model-context checkpoints. An inbound block
 * must never become `AssistantMessageBlock` or `ToolMessageBlock`, which are
 * reserved for output and actions of the current agent.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UserMessageBlock".
 */
export interface UserMessageBlock {
  /**
   * Identifies a committed canonical message block.
   */
  id: string;
  /**
   * The inbound content.
   */
  content: UserContentBlock[];
  /**
   * Provenance: who supplied the inbound information.
   */
  source:
    | 'human'
    | {
        agent: {
          /**
           * Identifies an Agent independently of its finite activations.
           * For a continuable child, this is the durable Agent-domain identity of
           * its child Conversation and remains unchanged across resume.
           */
          agent_id: string;
        };
      }
    | 'fleet'
    | 'external_system'
    | 'runtime'
    | {
        extension: {
          /**
           * The stable logical key of one certified extension.
           *
           * Package/content attestation is intentionally not part of this type.  A
           * package may be upgraded while preserving its logical ordering identity;
           * the assembly generation records the attestation separately.
           */
          contributor: string;
        };
      };
  /**
   * Typed kind of inbound information.
   */
  kind?:
    | {
        goal_continuation: GoalRef;
      }
    | 'message'
    | {
        context: ContextKind;
      };
  /**
   * The persisted UTC instant associated with the inbound message, when
   * the producer supplied one.
   *
   * An ordinary asynchronously delivered inbound message
   * ([`InboundKind::Message`]) carries the persisted instant of its
   * delivery; the producer supplies the original timestamp explicitly and
   * no wall-clock time is fabricated. Messages without a producer timestamp
   * remain representable: the field
   * defaults to `None` on deserialization and is omitted from the
   * canonical encoding while absent.
   */
  timestamp?: string | null;
}
/**
 * Authoritative bounded durable record.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "GoalSnapshot".
 */
export interface GoalSnapshot {
  reference: GoalRef;
  objective: string;
  phase: GoalPhase;
  blocked_reason?: string | null;
  autonomous_round_budget: number;
  autonomous_rounds_consumed: number;
  origin: GoalOrigin;
  last_round_message_id?: MessageId | null;
}
/**
 * The structured durable identity of one canonical Agent Status generation.
 *
 * The descriptor is attached to [`ContextKind::AgentStatus`] itself. Its
 * timestamp is the single Agent Status clock sample used to produce the
 * generation, and its typed module list is the source of truth for active
 * Surface visibility. Renderer text is never consulted for either fact.
 *
 * The fields are private so every value, including one decoded from durable
 * JSON, has non-empty, duplicate-free membership in deterministic semantic
 * order.
 *
 * ```compile_fail
 * use rustx::message::types::AgentStatusGenerationMetadata;
 *
 * fn mutate(metadata: &mut AgentStatusGenerationMetadata) {
 *     metadata.modules = Vec::new();
 * }
 * ```
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentStatusGenerationMetadata".
 */
export interface AgentStatusGenerationMetadata {
  generated_at: string;
  modules: AgentStatusModuleId[];
}
/**
 * One completed model generation produced by the current agent.
 *
 * One generation becomes one immutable `AssistantMessageBlock` containing
 * multiple content blocks. Streaming deltas are never committed here; they
 * belong to `ModelEvent` until the generation completes. `send_message`
 * results and other inbound material from other agents never appear in this
 * role.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AssistantMessageBlock".
 */
export interface AssistantMessageBlock {
  /**
   * Identifies a committed canonical message block.
   */
  id: string;
  /**
   * The completed generation content.
   */
  content: AssistantContentBlock[];
}
/**
 * Model reasoning content.
 *
 * Reasoning text is preserved for diagnostics, but reasoning/continuation
 * state is never flattened into plain text: provider-specific opaque state
 * survives on the [`ProviderContinuationState`] boundary for later
 * continuation.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ReasoningBlock".
 */
export interface ReasoningBlock {
  /**
   * The reasoning text, when the provider exposed it.
   */
  text?: string | null;
  /**
   * Provider continuation state required to continue the generation.
   */
  provider_state?: ProviderContinuationState | null;
}
/**
 * Continuation state for the `Anthropic` Messages protocol.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AnthropicContinuation".
 */
export interface AnthropicContinuation {
  /**
   * Opaque provider state preserved verbatim by the adapter.
   */
  opaque: {
    [k: string]: unknown;
  };
}
/**
 * One tool call issued by the current agent.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolCall".
 */
export interface ToolCall {
  /**
   * Provider/model-issued opaque correlation string, scoped to a request/publication.
   * Not a rustX global identity: canonical ownership uses `ToolCallOccurrenceRef`.
   */
  id: string;
  /**
   * Identifies a tool definition in the capability set.
   */
  tool_id: string;
  /**
   * Name of the tool at call time, sufficient for resolution together with
   * `tool_id`.
   */
  name: string;
  /**
   * Arbitrary JSON arguments for the tool call.
   */
  arguments: {
    [k: string]: unknown;
  };
}
/**
 * A refusal generated by the model.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RefusalBlock".
 */
export interface RefusalBlock {
  /**
   * The refusal explanation text.
   */
  text: string;
}
/**
 * The result of one tool call produced by the current agent.
 *
 * This block is the canonical conversation record of an execution outcome
 * and composes [`ToolExecutionResult`] as its single source of truth. For
 * `send_message`-style platform tools, the result is only the delivery
 * acceptance/rejection acknowledgment; a later reply from the recipient
 * arrives as a `UserMessageBlock` with agent provenance and is never nested
 * here.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolMessageBlock".
 */
export interface ToolMessageBlock {
  /**
   * Identifies a committed canonical message block.
   */
  id: string;
  occurrence: ToolCallOccurrenceRef;
  /**
   * Provider/model-issued opaque correlation string, scoped to a request/publication.
   * Not a rustX global identity: canonical ownership uses `ToolCallOccurrenceRef`.
   */
  tool_call_id: string;
  /**
   * Identifies a tool definition in the capability set.
   */
  tool_id: string;
  result: ToolExecutionResult2;
}
/**
 * Exact native canonical owner of this result.
 */
export interface ToolCallOccurrenceRef {
  /**
   * Identifies a committed canonical message block.
   */
  assistant_message_id: string;
  /**
   * A stable index identifying one content block within the ordered content
   * list of the canonical message being assembled.
   *
   * Streaming facts (text deltas, reasoning deltas, provider continuation
   * state, tool-call content) reference the block they belong to by this
   * index, so interleaved and multiple blocks remain unambiguous without
   * exposing any provider-specific block id type.
   */
  block_index: number;
}
/**
 * The normalized outcome of one tool execution.
 *
 * `ToolMessageBlock` composes this type instead of duplicating its fields,
 * keeping one source of truth for tool results.
 */
export interface ToolExecutionResult2 {
  /**
   * Immutable native Workflow identity on the existing outer result.
   * Historical identity only: no execution state or continuation authority.
   * Its retention is exactly that of this result, never a separate registry.
   */
  workflow?: WorkflowToolIdentity | null;
  /**
   * Typed execution status, including unknown external outcomes.
   */
  status:
    | {
        type: 'success';
      }
    | {
        /**
         * Human-readable error message.
         */
        error: string;
        type: 'failed';
      }
    | {
        /**
         * The policy or human-readable approval reason.
         */
        reason: string;
        type: 'denied';
      }
    | {
        /**
         * Why the execution was cancelled.
         */
        reason:
          | 'user_requested'
          | 'runtime_shutdown'
          | 'parent_cancelled'
          | 'subagent_execution_deadline_exceeded';
        /**
         * Whether cancellation won before executor start or while execution
         * was already in flight.
         */
        phase: 'before_start' | 'during_execution';
        type: 'cancelled';
      }
    | {
        type: 'timed_out';
      }
    | {
        /**
         * A producer-owned diagnostic describing why certainty is
         * unavailable. It is rendered into the bounded model-facing
         * projection and is never parsed to decide semantics; the typed
         * variant itself is the certainty claim.
         */
        detail: string;
        type: 'outcome_unknown';
      };
  /**
   * Tool-owned result content.
   *
   * This content is TOOL-OWNED: [`ToolResultContent::Json`] is arbitrary
   * tool-owned structured data, and the runtime never infers semantics
   * from its property names. rustX reserves no ordinary JSON field names;
   * runtime-owned facts live in the typed fields of this struct. A
   * provider-independent, bounded model-facing representation is produced
   * by [`Self::model_facing_projection`]; producers do not append runtime
   * status or managed-output continuation text here.
   */
  content?: ToolResultContent[];
  /**
   * Execution duration in integer milliseconds (stable for persistence).
   */
  duration_ms: string;
  /**
   * Process exit code where the tool executed a process.
   */
  exit_code?: number | null;
  /**
   * Immutable managed Artifact references produced by the execution.
   */
  artifacts?: FileReference[];
  /**
   * Runtime-owned explicit declarations of mutable native Session files.
   * Only successful canonical Tool results authorize delivery cards.
   */
  deliveries?: SessionFileReference[];
  /**
   * Truncation metadata where output was truncated.
   */
  truncation?: TruncationState | null;
  /**
   * Runtime-owned managed textual-output continuation metadata: where
   * the complete — or honestly partial — textual output of this result
   * lives in the conversation's managed tool-output store (Issue #86).
   * Absent for results whose output fits the model-facing content.
   *
   * This is the one typed source of truth for complete-vs-partial
   * managed output; producers never encode these facts as magic
   * properties of tool-owned JSON, and generic runtime publication code
   * consumes only this typed field, never arbitrary JSON keys.
   */
  managed_output?: ManagedOutputContinuation | null;
}
/**
 * The bounded immutable publication audit.
 */
export interface PublicationAudit {
  /**
   * Identifies one user-facing publication stream (Issue #108, FND-03).
   *
   * A publication stream is the durable release plane of exactly one
   * provider request: it is the identity under which committed-for-release
   * frames are staged, under which the publication terminal marker
   * commits, and under which the stream settles exactly once as canonical,
   * unaccepted, or incomplete. It is deliberately distinct from
   * [`RequestId`] (the provider execution fact) and from [`MessageId`]
   * (the canonical conversation fact), because publication durability is
   * a separate plane from both.
   */
  stream_id: string;
  /**
   * Identifies one attempt to execute an agent manifest.
   */
  attempt_id: string;
  /**
   * Identifies one turn within an attempt.
   */
  turn_id: string;
  /**
   * Identifies one actual provider-neutral model request.
   *
   * A request identity is distinct from an attempt, turn, retry ordinal,
   * and Event Journal sequence. Primary requests derive the `request:`
   * namespace from immutable [`RequestIdentity`](crate::model::snapshot::RequestIdentity).
   * Auxiliary requests allocate the separate `req_` `UUIDv7` namespace with
   * [`RequestId::generate_auxiliary`]. Each physical retry is a distinct
   * request. Identity alone grants no execution permission.
   */
  request_id: string;
  /**
   * Identifies a committed canonical message block.
   */
  message_id: string;
  /**
   * Which of the two audit settlements this is.
   */
  kind: 'unaccepted' | 'incomplete';
  /**
   * The consolidated committed-for-release content, in block order.
   */
  content: PublicationAuditBlock[];
  /**
   * When the audit terminalized.
   */
  settled_at: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ReviewSpecification".
 */
export interface ReviewSpecification {
  instance: WorkflowNodeInstance;
  subject: ReviewSubject;
  /**
   * Bounded committed Workflow values presented beside the subject.
   */
  context: unknown[];
}
/**
 * The canonical identity of the tool that asked. Durable evidence of
 * *who* asked, so an audit of an MCP elicitation names its server.
 */
export interface InteractionRequester {
  /**
   * Identifies a tool definition in the capability set.
   */
  tool_id: string;
  /**
   * The safe model-facing tool name.
   */
  tool_name: string;
  /**
   * The registry-resolved tool origin, which carries MCP server identity.
   */
  origin:
    | 'builtin'
    | {
        mcp: {
          server_id: McpServerId;
        };
      }
    | {
        managed_python: {
          package: string;
        };
      };
}
/**
 * The exact immutable facts projected to the Runtime Client.
 */
export interface QuestionnaireSpecification {
  /**
   * One to four related blocking questions.
   */
  questions: QuestionSpecification[];
}
/**
 * One question in a questionnaire.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "QuestionSpecification".
 */
export interface QuestionSpecification {
  /**
   * The full question shown above the answer surface.
   */
  question: string;
  /**
   * The short label used by the question tab.
   */
  header: string;
  /**
   * The exact shape of a legal answer.
   */
  answer:
    | {
        /**
         * The inclusive minimum Unicode scalar count, when the producer declares one.
         */
        min_length?: number | null;
        /**
         * The inclusive maximum Unicode scalar count, when the producer declares one.
         */
        max_length?: number | null;
        /**
         * The declared text shape, when the producer declares one.
         */
        format?: TextFormat | null;
        type: 'text';
      }
    | {
        /**
         * The inclusive minimum, when the producer declares one.
         */
        minimum?: FiniteNumber | null;
        /**
         * The inclusive maximum, when the producer declares one.
         */
        maximum?: FiniteNumber | null;
        type: 'number';
      }
    | {
        /**
         * The inclusive minimum, when the producer declares one.
         */
        minimum?: (ExactInteger & ExactInteger1) | null;
        /**
         * The inclusive maximum, when the producer declares one.
         */
        maximum?: (ExactInteger & ExactInteger1) | null;
        type: 'integer';
      }
    | {
        type: 'boolean';
      }
    | {
        /**
         * The finite declared options, addressed by index.
         */
        options: OptionSpecification[];
        /**
         * Whether a free-text answer outside the declared options is legal.
         */
        allow_custom: boolean;
        type: 'single_choice';
      }
    | {
        /**
         * The finite declared options, addressed by index.
         */
        options: OptionSpecification[];
        /**
         * The inclusive lower bound on how many options a submitted answer selects.
         */
        min_selected: number;
        /**
         * The inclusive upper bound on how many options a submitted answer selects.
         */
        max_selected: number;
        /**
         * Whether a free-text answer outside the declared options is legal.
         */
        allow_custom: boolean;
        type: 'multi_choice';
      };
}
/**
 * One authored option in a choice question.
 *
 * The label is **presentation**. A response addresses this option by its
 * zero-based position in [`SingleChoiceSpecification::options`] or
 * [`MultiChoiceSpecification::options`], so a duplicated, reserved, or forged
 * display string can never select a different underlying value.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "OptionSpecification".
 */
export interface OptionSpecification {
  /**
   * The short option label shown in the selection list.
   */
  label: string;
  /**
   * The meaning and trade-offs of the option.
   */
  description: string;
  /**
   * Optional Markdown rendered in the preview pane.
   */
  preview?: string | null;
}
/**
 * The submitted decisions for one questionnaire. Omitted questions are
 * intentionally allowed so a user may submit a partial questionnaire.
 */
export interface QuestionnaireSubmission1 {
  /**
   * At most one entry per answered question.
   */
  answers: QuestionnaireAnswerEntry[];
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ObservedTurnIdentity".
 */
export interface ObservedTurnIdentity {
  conversation_id: ConversationId;
  attempt_id?: AttemptId | null;
  attempt_id_source?: AttemptIdentitySource | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientContextOccupancy".
 */
export interface RuntimeClientContextOccupancy {
  estimated: boolean;
  input_tokens: string;
  context_window_tokens: string;
  model?: string | null;
  model_deferred: boolean;
  /**
   * Exact selected-field domain, read with the occupancy's captured checkpoint cut.
   */
  model_source?: string | null;
  breakdown: ContextBreakdown;
}
/**
 * Where one request's measured input went, priced by the provider-neutral
 * `ceil(bytes / 4)` estimate. System prompt and Tool definitions are priced
 * heuristically; messages are the remainder of the measured or estimated total.
 * The parts sum to that total unless the component estimates alone exceed it.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ContextBreakdown".
 */
export interface ContextBreakdown {
  system_tokens: string;
  tool_tokens: string;
  message_tokens: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CheckpointPage".
 */
export interface CheckpointPage {
  /**
   * Nested advancing domain frontiers represented by this exact page cut.
   */
  collections?: CollectionFrontier[];
  cursor: RuntimeClientCursor;
  read_cut: CheckpointReadCut;
  domain: string;
  total: string;
  items: CheckpointPageItem[];
  next_offset?: string | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ObservedTurnPage".
 */
export interface ObservedTurnPage {
  cut: ConversationReadCut;
  total: string;
  offset: number;
  /**
   * @maxItems 64
   */
  turns: ObservedTurn[];
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ObservedTurn".
 */
export interface ObservedTurn {
  id: ObservedTurnIdentity;
  ordinal: number;
  cursor?: TranscriptCursor | null;
  prompt: string;
  response: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientCheckpoint".
 */
export interface RuntimeClientCheckpoint {
  snapshot: RuntimeClientSnapshot;
  cursor: RuntimeClientCursor;
  read_cut: CheckpointReadCut;
  content: ContentBinding[];
  collections: CollectionFrontier[];
}
/**
 * The authoritative Runtime Client snapshot of one conversation runtime.
 *
 * Every section is a deterministic projection of one authoritative
 * runtime owner. The shape belongs to the Runtime Client protocol: internal
 * snapshot types are projected into these external DTOs, never exposed
 * directly.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientSnapshot".
 */
export interface RuntimeClientSnapshot {
  settings_evidence: SettingsEvidence;
  /**
   * The frozen effective native Agent Extension composition of the Agent
   * runtime this snapshot projects (Issue #256).
   *
   * This is the composition the attached runtime is *already executing
   * against*, read from the extension owners it materialized — the value
   * frozen at `LocalConversationCore::compose` for a root, and the value
   * carried in `ResolvedSubagentSpec::extensions` for a Subagent child.
   * Nothing on this path rereads `rustx.toml`, project or host
   * configuration, a role document, a `ProspectiveSessionConfig`, or the latest
   * `RuntimeResourceSnapshot`, and nothing infers it from Agent Status
   * observations, context messages, or the Event Journal. It is therefore
   * deliberately distinct from `rustx config show --sources`, which
   * describes a *prospective next launch* and may legitimately disagree
   * after a configuration edit.
   *
   * `None` means no authoritative Agent composition is available to
   * project — historical-only durable inspection. It is never filled from
   * current disk configuration or built-in defaults. Inside the value,
   * `agent_status: None` is the separate fact that the extension is not
   * part of this Agent's composition at all.
   */
  effective_plugins?: EffectivePlugins | null;
  workflows: WorkflowSnapshot;
  /**
   * The conversation this snapshot belongs to.
   */
  conversation_id: string;
  /**
   * Whether runtime drain has begun and new inbound admission is closed.
   * The correlated shutdown response resolves only after quiescence.
   */
  shutting_down: boolean;
  /**
   * The runtime-wide control state for tool approval behavior.
   *
   * `Policy` consults each resolved Tool's
   * [`ToolApprovalPolicy`](crate::tools::types::ToolApprovalPolicy).
   * `FullAccess` changes only the effective approval result to `Never`; it
   * never changes availability, activation, execution ownership, concurrency,
   * or tool authority.
   */
  effective_approval_mode: 'policy' | 'full_access';
  /**
   * Whether this live runtime could adopt a prepared Session configuration
   * now. Runtime-domain advisory state published by the runtime with its
   * adoption gate's idle rule over in-memory owners, and changed only by the
   * `configuration_adoption_eligibility_changed` event; it describes no
   * candidate and is never configuration application state. `unavailable` until the runtime runs, after it stops, and for a
   * durable inspection projection. `session/adoptConfiguration` revalidates
   * the real gate at its commit boundary.
   */
  configuration_adoption_eligibility:
    | {
        status: 'eligible';
      }
    | {
        status: 'busy';
      }
    | {
        status: 'unavailable';
      };
  /**
   * The runtime's durable-authority failure, when it has entered the
   * explicit degraded state. While set, no new durable admission/execution
   * work may begin.
   */
  durability_failure?: RuntimeDurabilityFailure | null;
  /**
   * The client projection of canonical Message Ledger observations.
   *
   * It is repaired from the native durable authority at bootstrap and
   * from committed observations while live; it is never independently
   * mutable or recovery input. A restarted projection contains the
   * current Surface working set, while historical Ledger pages remain
   * available through `ConversationStore` APIs.
   */
  messages: MessageBlock[];
  message_descriptors: RuntimeClientMessageDescriptor[];
  /**
   * Native projection-selected settlement head; retained audit bodies remain in storage.
   */
  publication_settlements?: RuntimeClientPublicationSettlementRecord[];
  transcript: RuntimeClientTranscriptPage1;
  trace: TracePage1;
  /**
   * Lifecycle repairs for explicitly requested loaded Trace identities.
   *
   * @maxItems 512
   */
  trace_updates: TraceLifecycle[];
  /**
   * The current/latest attempt view, when any attempt exists.
   */
  attempt?: RuntimeClientAttempt | null;
  inbound: InboundDiagnostics;
  /**
   * Live process-owned native interaction requests. This is projection
   * state, never durable recovery input or client-owned truth.
   */
  pending_interactions?: RuntimeClientInteractionHeader[];
  /**
   * All background executions in execution allocation order, including
   * terminal records retained by the authoritative registry.
   */
  jobs?: RuntimeClientJob[];
  /**
   * Durable child Agents in creation order. Repeated activations replace
   * the same Agent row; activation history remains in the Event Journal.
   */
  agents?: RuntimeClientAgent[];
  /**
   * The bounded newest window of composed Agent Status observations, in
   * runtime composition order (oldest first).
   *
   * This is a **list, not a latest value**: a composed status is a
   * historical fact of the conversation, and a later attempt neither
   * retracts nor relocates an earlier one. A client that wants "the
   * latest" takes the last element; it never needs a second field that
   * could disagree with this order.
   *
   * The window is bounded by [`AGENT_STATUS_WINDOW`], and that bound is
   * owned **here, in the projection, and nowhere else**. A client folding
   * [`RuntimeClientEvent::AgentStatusComposed`](super::RuntimeClientEvent)
   * incrementally never applies a retention rule of its own: each event
   * carries the eviction this exact projection transition performed, so
   * the fold reproduces the window rather than re-deciding it. Two
   * retention owners would be two policies, and after the bound was
   * crossed a snapshot repair would silently drop compositions a
   * continuously subscribed client still believed in.
   *
   * The canonical Agent Status Context message is request-scoped model
   * history and never enters the durable transcript, so unlike a
   * transcript page this window cannot be paged backwards: a composition
   * older than the window is simply no longer projected. Like `attempt`
   * and `context`, it describes the live runtime and starts empty on a
   * fresh runtime.
   */
  statuses?: AgentStatusView[];
  context?: RuntimeClientContextView;
  capabilities: CapabilityView;
  resources?: RuntimeClientResourcesView;
  /**
   * The redacted session model state: the authoritative *desired*
   * configuration and its resolution.
   *
   * This is deliberately distinct from
   * [`RuntimeClientAttempt::model`], which is the immutable snapshot an
   * already-admitted attempt froze. While an attempt on model A runs and
   * the session has been switched to model B, this section truthfully
   * shows B and the attempt section truthfully shows A.
   *
   * No credential, adapter object, provider HTTP client, or
   * synchronization identity appears here.
   * Absent when no live Session model authority exists (historical inspection).
   */
  model?: SessionModelView | null;
  /**
   * Admission availability remains explicit when growing model details are deferred.
   */
  model_status?: RuntimeClientModelStatus | null;
  /**
   * The conversation's task list, as of the newest committed `todo`
   * result — present exactly when this runtime composes the **Todo**
   * Agent Extension (Issue #259).
   *
   * This is a **projection of canonical history, not a second
   * authority**: the runtime derives it from exactly the tool results the
   * Ledger holds, the same fact the runtime's own
   * [`ConversationTodoList`] is rebuilt from, so the two can never
   * disagree.
   *
   * It is carried here rather than left for a client to scan out of the
   * transcript because a client holds only a bounded newest page of that
   * transcript. A conversation that committed a page or more of messages
   * after its last `todo` result would otherwise attach with no list at
   * all, and would appear to have none until the reader happened to page
   * far enough back — while the runtime, reading the whole Ledger, still
   * had one.
   *
   * A conversation that never called `todo` carries the empty list.
   *
   * `None` is a different fact and must render differently: this runtime
   * composes no Todo extension, so there is no current task list to show
   * at all. Canonical history may still contain `todo` calls and results
   * from a launch that did compose it; those remain renderable as
   * *transcript history* and must not be folded back into a current panel.
   *
   * [`ConversationTodoList`]: crate::tools::todo::ConversationTodoList
   */
  todos?: TodoSnapshot | null;
  /**
   * Durable Goal read model at this projection cursor; absent when the
   * extension is not composed. Journal facts never reconstruct it.
   */
  goal?: RuntimeClientGoalView | null;
}
/**
 * The frozen effective native Agent Extension composition of the Agent
 * runtime this snapshot projects (Issue #256).
 *
 * This is a **projection of the attached runtime's own composition**, never
 * a reread of authoring configuration. It is closed and typed — one named
 * member per native extension — so the vocabulary grows only when a native
 * extension is deliberately added to it. There is no map, no
 * `serde_json::Value`, no plugin descriptor, and no dynamic registry view.
 *
 * Two different nullabilities meet on this path and must not be confused:
 *
 * - the snapshot's `effective_plugins` is `None` when there is no
 *   authoritative Agent composition to project at all — historical-only
 *   durable inspection. It is never filled from disk, built-in defaults,
 *   or the latest runtime configuration;
 * - `agent_status` inside it is `None` when the extension is **not part of
 *   this Agent's composition**. That is a different fact from "composed,
 *   with both contributors switched off", which is
 *   `Some(EffectiveAgentStatusExtension { time: disabled, background:
 *   disabled })`.
 *
 * It is also independent of whether any Agent Status was actually composed
 * for a step: a runtime with the extension enabled and no eligible status
 * contribution yet still reports `Some(..)`. Observations describe steps;
 * this describes the composition.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectivePlugins".
 */
export interface EffectivePlugins {
  /**
   * The composed Agent Status extension, or `None` when this Agent
   * composes no Agent Status at all.
   */
  agent_status?: EffectiveAgentStatusExtension | null;
  /**
   * The composed Todo extension, or `None` when this Agent composes no
   * Todo at all (Issue #259).
   *
   * This is the authoritative answer to "does this runtime have a current
   * task list, a `todo` Tool, and a Todo panel?" — and it is the only
   * authoritative answer. A client must not infer it from the presence of
   * `todo` results in the transcript, which are historical facts of the
   * conversation rather than facts about the runtime attached to it.
   */
  todo?: EffectiveTodoExtension | null;
  /**
   * Root Goal capability, frozen for this launch.
   */
  goal?: GoalExtensionConfig | null;
}
/**
 * The frozen contributor configuration of a composed Agent Status extension.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectiveAgentStatusExtension".
 */
export interface EffectiveAgentStatusExtension {
  time: EffectiveTimeStatus;
  background: EffectiveBackgroundStatus;
}
/**
 * The frozen Time contributor of a composed Agent Status extension.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectiveTimeStatus".
 */
export interface EffectiveTimeStatus {
  enabled: boolean;
  /**
   * The IANA timezone frozen for this composition, or `None` when none was
   * configured. `None` is "no explicit timezone", not "UTC".
   */
  timezone?: string | null;
}
/**
 * The frozen Background contributor of a composed Agent Status extension.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectiveBackgroundStatus".
 */
export interface EffectiveBackgroundStatus {
  enabled: boolean;
}
/**
 * The frozen Todo extension of a composition that includes it.
 *
 * It carries no field: Todo has no contributor configuration, so being
 * composed is the whole fact. It is a struct rather than a bare `bool`
 * because the vocabulary is closed and typed, and because a later
 * contributor would be added here rather than by changing the shape of the
 * value clients already parse.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectiveTodoExtension".
 */
export interface EffectiveTodoExtension {}
/**
 * Frozen Goal composition; domain bounds are native, not launch settings.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "GoalExtensionConfig".
 */
export interface GoalExtensionConfig {}
/**
 * Bounded native Workflow state, never reconstructed from the journal.
 */
export interface WorkflowSnapshot {
  revision: string;
  runs: WorkflowRunView[];
  omitted_runs: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowRunView".
 */
export interface WorkflowRunView {
  id: WorkflowRunId;
  workflow_id: WorkflowId;
  program_digest: string;
  resource_revision: RuntimeResourceRevision;
  tool_call_id?: ToolCallId | null;
  state: WorkflowState;
  instances: WorkflowInstanceView[];
  omitted_instances: number;
  steps_consumed: number;
  steps_max: number;
  agents_consumed: number;
  model_calls_consumed: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowInstanceView".
 */
export interface WorkflowInstanceView {
  block: WorkflowBlockInstance;
  node?: string | null;
  visit?: number | null;
  kind: WorkflowNodeKind;
  state: WorkflowState;
  child?: SubagentId | null;
  /**
   * Exact read-only child identity; never grants durable Agent controls.
   */
  child_agent_id?: AgentId | null;
  invocation?: ToolInvocationId | null;
  tool_id?: ToolId | null;
  /**
   * Logical Auxiliary identity only; prompts and output stay with their owners.
   */
  auxiliary_call_id?: AuxiliaryCallId | null;
  interaction?: InteractionRef | null;
  iteration?: number | null;
  iterations_max?: number | null;
  loop_exit?: WorkflowLoopExit | null;
  checks_passed?: boolean | null;
  review_accepted?: boolean | null;
}
/**
 * The client-visible durable-authority failure state of a conversation
 * runtime (Issue #63).
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeDurabilityFailure".
 */
export interface RuntimeDurabilityFailure {
  /**
   * The operation that failed persistently.
   */
  operation: string;
  /**
   * The human-readable failure diagnostic.
   */
  diagnostic: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientMessageDescriptor".
 */
export interface RuntimeClientMessageDescriptor {
  deliveries?: RuntimeClientDeliveryHead | null;
  identity?: MessageId | null;
  locator: CanonicalContentLocator;
  role: string;
  kind: string;
  block_count: string;
  tool_call_count: string;
  /**
   * Finite native Tool occurrences; remaining calls use the advertised frontier.
   */
  tool_calls: ForegroundToolExecution[];
  turn_process?: TurnProcessView | null;
  completed_response?: CompletedResponseView | null;
  content: ContentBinding[];
  collections: CollectionFrontier[];
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientDeliveryHead".
 */
export interface RuntimeClientDeliveryHead {
  domain: string;
  total: string;
  /**
   * @maxItems 8
   */
  items:
    | []
    | [RuntimeClientDeliveryItem]
    | [RuntimeClientDeliveryItem, RuntimeClientDeliveryItem]
    | [RuntimeClientDeliveryItem, RuntimeClientDeliveryItem, RuntimeClientDeliveryItem]
    | [
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem
      ]
    | [
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem
      ]
    | [
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem
      ]
    | [
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem
      ]
    | [
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem,
        RuntimeClientDeliveryItem
      ];
  next_offset?: number | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientDeliveryItem".
 */
export interface RuntimeClientDeliveryItem {
  index: number;
  locator: CanonicalContentLocator;
  file?: SessionFileReference | null;
  metadata_deferred: boolean;
  encoded_bytes: string;
  metadata_domain: string;
}
/**
 * Exact source placement and native cut of one positive publication settlement.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientPublicationSettlementRecord".
 */
export interface RuntimeClientPublicationSettlementRecord {
  settlement: RuntimeClientPublicationSettlement;
  transcript_cursor: RuntimeClientTranscriptCursor;
  read_cut: ObservationReadCut;
}
/**
 * Positive native audit settlement when its retained content is deferred.
 * This is Publication Plane authority, not canonical C or execution settlement.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientPublicationSettlement".
 */
export interface RuntimeClientPublicationSettlement {
  stream_id?: PublicationStreamId | null;
  message_id?: MessageId | null;
  attempt_id?: AttemptId | null;
  turn_id?: TurnId | null;
  request_id?: RequestId | null;
  kind: PublicationAuditKind;
  settled_at: string;
  block_count: string;
}
/**
 * One bounded newest-or-older page of derived transcript history.
 */
export interface RuntimeClientTranscriptPage1 {
  /**
   * The exact native source cut from which bodies and evidence were read.
   */
  source_cut?: ObservationReadCut | null;
  /**
   * Deferred bytes remain explicitly recoverable on older historical pages.
   */
  content?: ContentBinding[];
  collections?: CollectionFrontier[];
  /**
   * Native items whose structural display requires bounded further reading.
   */
  deferred_entries?: CheckpointPageItem[];
  /**
   * Last native transcript position inherited by this lineage; zero for an empty prefix.
   */
  inherited_through?: RuntimeClientTranscriptCursor | null;
  /**
   * Native whole-conversation totals, independent of this page.
   */
  statistics?: ConversationStatistics | null;
  /**
   * Items in chronological order within this page.
   */
  entries?: RuntimeClientTranscriptEntry[];
  /**
   * The exclusive cursor for the next older page.
   */
  next_cursor?: RuntimeClientTranscriptCursor | null;
}
/**
 * One finite page of bounded summary records, oldest first.
 */
export interface TracePage1 {
  read_cut?: CheckpointReadCut | null;
  /**
   * Current-cut repairs for explicitly requested loaded identities.
   */
  updates?: TraceLifecycle[];
  records: TraceRecord[];
  next_cursor?: TraceCursor | null;
}
/**
 * The external attempt view of the Runtime Client projection.
 *
 * The view folds attempt lifecycle, turn progress, in-flight agent
 * output, and foreground tool execution into one structured read model.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientAttempt".
 */
export interface RuntimeClientAttempt {
  /**
   * Exact inline display identity, when bounded.
   */
  attempt_id?: AttemptId | null;
  /**
   * Original native owner evidence when the inline alias is deferred.
   */
  attempt_id_source?: AttemptIdentitySource | null;
  /**
   * The externally meaningful attempt phase.
   */
  phase:
    | {
        type: 'incomplete';
      }
    | {
        type: 'admitted';
      }
    | {
        type: 'running';
      }
    | {
        /**
         * The platform-level terminal settlement.
         */
        outcome:
          | {
              /**
               * The normalized finish reason.
               */
              finish_reason:
                | {
                    type: 'stop';
                  }
                | {
                    type: 'tool_calls';
                  }
                | {
                    type: 'length';
                  }
                | {
                    type: 'content_filter';
                  }
                | {
                    type: 'refusal';
                  }
                | {
                    /**
                     * The original provider reason, preserved for diagnostics.
                     */
                    reason: string;
                    type: 'other';
                  };
              /**
               * The original reason exceeds its finite inline preview.
               */
              finish_reason_deferred?: boolean;
              /**
               * Exact allowlisted reason bytes at the published native cut.
               */
              finish_reason_source?: JournalFieldRef | null;
              type: 'completed';
            }
          | {
              /**
               * Why the attempt was cancelled.
               */
              reason:
                | 'user_requested'
                | 'runtime_shutdown'
                | 'parent_cancelled'
                | 'subagent_execution_deadline_exceeded';
              type: 'cancelled';
            }
          | {
              type: 'timed_out';
            }
          | {
              /**
               * Which limit was exceeded.
               */
              limit: 'max_turns' | 'max_tool_calls' | 'max_runtime_seconds';
              type: 'limit_exceeded';
            }
          | {
              /**
               * The normalized client-visible failure of one attempt.
               *
               * This is the external projection of the internal [`AttemptFailure`]
               * ([`crate::events::types::AttemptFailure`]): provider-specific fields
               * (such as the raw provider error code) never appear here.
               */
              error:
                | {
                    /**
                     * Error classes the runtime distinguishes for retry/termination decisions.
                     * Provider SDK error structs never cross this boundary.
                     */
                    kind:
                      | 'invalid_request'
                      | 'authentication'
                      | 'rate_limit'
                      | 'timeout'
                      | 'transport'
                      | 'provider_error'
                      | 'context_window_exceeded'
                      | 'cancelled'
                      | 'unsupported'
                      | 'malformed_tool_proposal'
                      | 'generation_degenerated'
                      | 'generation_budget_exceeded';
                    /**
                     * The normalized human-readable message.
                     */
                    message: string;
                    /**
                     * The retry hint, when the provider reported one.
                     */
                    retry_after_ms?: string | null;
                    type: 'model';
                  }
                | {
                    /**
                     * The normalized runtime error.
                     */
                    error:
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'internal';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'invalid_state';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'unsupported';
                        }
                      | {
                          /**
                           * The tool name the model called.
                           */
                          name: string;
                          type: 'unknown_tool';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'durable_store';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'contract_violation';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'context_preparation_failed';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'context_compaction_failed';
                        }
                      | {
                          /**
                           * The policy's bounded rejection reason.
                           */
                          reason: string;
                          type: 'pre_step_rejected';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'pre_step_policy_failed';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'tool_result_observation_failed';
                        }
                      | {
                          /**
                           * The bounded recovery diagnostic: which durable evidence settled
                           * the attempt and what remained indeterminate.
                           */
                          message: string;
                          type: 'restart_interrupted';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'deferred_context_rejected';
                        };
                    type: 'runtime';
                  };
              /**
               * The normalized diagnostic exceeds its finite inline preview.
               */
              error_deferred?: boolean;
              /**
               * Normalized diagnostic source; never a raw provider error.
               */
              error_source?: JournalFieldRef | null;
              type: 'failed';
            };
        type: 'settled';
      };
  /**
   * The number of completed turns.
   */
  turn: number;
  /**
   * The latest normalized usage of a completed model request, when any.
   */
  last_usage?: ModelUsage | null;
  /**
   * The in-flight Assistant output, when a message is streaming: enough
   * accumulated state to repair every client-visible streaming effect.
   */
  in_flight?: InFlightAssistantMessage | null;
  /**
   * The foreground tool executions of the attempt in call-assembly
   * order.
   */
  foreground?: ForegroundToolExecution[];
  /**
   * The immutable model snapshot this attempt was admitted with.
   *
   * A client never has to infer "which model is this attempt actually
   * using" from event ordering: the answer is here for the attempt's
   * whole lifetime, even after the session moved on to another model.
   */
  model?: AttemptModelView | null;
  /**
   * Unavailable for history lacking native admission evidence. Never derived from current resources.
   */
  execution_settings?: AdmittedSettings | null;
}
/**
 * The accumulated in-flight output of one streaming Assistant message.
 *
 * A checkpoint carries bounded previews and exact publication frontiers.
 * Clients recover deferred bytes from those native references, independently
 * of advancing their Applied Cursor through complete display transitions.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InFlightAssistantMessage".
 */
export interface InFlightAssistantMessage {
  /**
   * Identifies a committed canonical message block.
   */
  message_id: string;
  /**
   * The ordered content blocks assembled so far.
   */
  blocks?: InFlightBlock[];
}
/**
 * The redacted client-facing projection of one attempt's frozen model
 * snapshot.
 *
 * This is what makes "session desired model = B, running attempt model = A"
 * unambiguous without a client inferring anything from event ordering.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AttemptModelView".
 */
export interface AttemptModelView {
  primary: ModelInvocationView;
  /**
   * The attempt's frozen summary policy.
   */
  summary:
    | {
        mode: 'session';
      }
    | {
        /**
         * An authored Model identity. Its spelling has no provider or wire semantics.
         */
        model: string;
        /**
         * The model interaction protocol an adapter must speak.
         */
        protocol: 'openai_chat_completions' | 'openai_responses' | 'anthropic_messages';
        /**
         * The model's context window in tokens.
         */
        contextWindow: number;
        /**
         * The model's configured maximum output tokens.
         */
        modelMaxOutputTokens: number;
        /**
         * The effective output budget.
         */
        maxOutputTokens: number;
        /**
         * The selected Model Profile, when the model declares any.
         */
        profile?: ModelProfileId | null;
        /**
         * Whether reasoning is semantically enabled.
         */
        reasoningEnabled: boolean;
        /**
         * The effective opaque provider request parameters.
         */
        requestParams?: {
          [k: string]: unknown;
        };
        capabilities: ModelCapabilities2;
        declaredCapabilities: ModelCapabilities3;
        mode: 'explicit';
      };
}
/**
 * The attempt's frozen primary invocation.
 */
export interface ModelInvocationView {
  /**
   * An authored Model identity. Its spelling has no provider or wire semantics.
   */
  model: string;
  /**
   * The model interaction protocol an adapter must speak.
   */
  protocol: 'openai_chat_completions' | 'openai_responses' | 'anthropic_messages';
  /**
   * The model's context window in tokens.
   */
  contextWindow: number;
  /**
   * The model's configured maximum output tokens.
   */
  modelMaxOutputTokens: number;
  /**
   * The effective output budget.
   */
  maxOutputTokens: number;
  /**
   * The selected Model Profile, when the model declares any.
   */
  profile?: ModelProfileId | null;
  /**
   * Whether reasoning is semantically enabled.
   */
  reasoningEnabled: boolean;
  /**
   * The effective opaque provider request parameters.
   */
  requestParams?: {
    [k: string]: unknown;
  };
  capabilities: ModelCapabilities;
  declaredCapabilities: ModelCapabilities1;
}
/**
 * The effective capabilities.
 */
export interface ModelCapabilities {
  /**
   * The accepted input modalities.
   */
  inputModalities: Modality[];
  /**
   * The produced output modalities.
   */
  outputModalities: Modality[];
  /**
   * Whether the model can be given tool definitions and can call them.
   */
  toolCalls: boolean;
  /**
   * Whether the model semantically supports reasoning.
   */
  reasoning: boolean;
  /**
   * Native API-enforced JSON Schema output for this model, endpoint and protocol.
   * An omitted claim is conservative; ordinary text requests never opt in implicitly.
   */
  jsonSchemaOutput?: boolean;
}
/**
 * The raw capabilities the catalog claims, for clients that want to
 * explain why an effective capability is absent.
 */
export interface ModelCapabilities1 {
  /**
   * The accepted input modalities.
   */
  inputModalities: Modality[];
  /**
   * The produced output modalities.
   */
  outputModalities: Modality[];
  /**
   * Whether the model can be given tool definitions and can call them.
   */
  toolCalls: boolean;
  /**
   * Whether the model semantically supports reasoning.
   */
  reasoning: boolean;
  /**
   * Native API-enforced JSON Schema output for this model, endpoint and protocol.
   * An omitted claim is conservative; ordinary text requests never opt in implicitly.
   */
  jsonSchemaOutput?: boolean;
}
/**
 * The effective capabilities.
 */
export interface ModelCapabilities2 {
  /**
   * The accepted input modalities.
   */
  inputModalities: Modality[];
  /**
   * The produced output modalities.
   */
  outputModalities: Modality[];
  /**
   * Whether the model can be given tool definitions and can call them.
   */
  toolCalls: boolean;
  /**
   * Whether the model semantically supports reasoning.
   */
  reasoning: boolean;
  /**
   * Native API-enforced JSON Schema output for this model, endpoint and protocol.
   * An omitted claim is conservative; ordinary text requests never opt in implicitly.
   */
  jsonSchemaOutput?: boolean;
}
/**
 * The raw capabilities the catalog claims, for clients that want to
 * explain why an effective capability is absent.
 */
export interface ModelCapabilities3 {
  /**
   * The accepted input modalities.
   */
  inputModalities: Modality[];
  /**
   * The produced output modalities.
   */
  outputModalities: Modality[];
  /**
   * Whether the model can be given tool definitions and can call them.
   */
  toolCalls: boolean;
  /**
   * Whether the model semantically supports reasoning.
   */
  reasoning: boolean;
  /**
   * Native API-enforced JSON Schema output for this model, endpoint and protocol.
   * An omitted claim is conservative; ordinary text requests never opt in implicitly.
   */
  jsonSchemaOutput?: boolean;
}
/**
 * Facts frozen together with the attempt model under native admission.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AdmittedSettings".
 */
export interface AdmittedSettings {
  /**
   * Whether the admitted catalog publishes native image reading.
   * Absent only when historical admission evidence is unavailable.
   */
  read_image_active?: boolean | null;
  resource_revision: RuntimeResourceRevision;
  approval_mode: ApprovalMode;
}
/**
 * The inbound mailbox diagnostics (pending items and the latest
 * finite drain observation).
 */
export interface InboundDiagnostics {
  /**
   * The currently pending inbound items in runtime-assigned inbound
   * sequence order.
   */
  pending?: InboundItemView[];
  /**
   * The latest observed finite drain boundary, when any drain occurred.
   */
  last_drain?: InboundDrainView | null;
}
/**
 * One pending inbound item of the diagnostics view.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InboundItemView".
 */
export interface InboundItemView {
  /**
   * Native compare-and-set revision.
   */
  revision: string;
  /**
   * A conversation-scoped inbound sequence number.
   *
   * The sequence identifies one item of the conversation's inbound ordering
   * domain. It is **not**
   * [`RuntimeEventEnvelope`](crate::events::types::RuntimeEventEnvelope)`::sequence`
   * and is never allocated from the Event Journal sequence; the durable
   * Pending Inbound Inbox owns allocation, and the first successful
   * acceptance of a conversation receives `1`.
   */
  sequence: string;
  /**
   * The bounded native header defers part of its authoritative message.
   */
  deferred?: boolean;
  message: UserMessageBlock1;
}
/**
 * Inbound information supplied to the current agent.
 *
 * A `UserMessageBlock` does not necessarily mean a human spoke: it is the
 * canonical home for anything inbound, including messages from other agents
 * (with [`UserSource::Agent`] provenance). Compaction summaries instead belong
 * to immutable model-context checkpoints. An inbound block
 * must never become `AssistantMessageBlock` or `ToolMessageBlock`, which are
 * reserved for output and actions of the current agent.
 */
export interface UserMessageBlock1 {
  /**
   * Identifies a committed canonical message block.
   */
  id: string;
  /**
   * The inbound content.
   */
  content: UserContentBlock[];
  /**
   * Provenance: who supplied the inbound information.
   */
  source:
    | 'human'
    | {
        agent: {
          /**
           * Identifies an Agent independently of its finite activations.
           * For a continuable child, this is the durable Agent-domain identity of
           * its child Conversation and remains unchanged across resume.
           */
          agent_id: string;
        };
      }
    | 'fleet'
    | 'external_system'
    | 'runtime'
    | {
        extension: {
          /**
           * The stable logical key of one certified extension.
           *
           * Package/content attestation is intentionally not part of this type.  A
           * package may be upgraded while preserving its logical ordering identity;
           * the assembly generation records the attestation separately.
           */
          contributor: string;
        };
      };
  /**
   * Typed kind of inbound information.
   */
  kind?:
    | {
        goal_continuation: GoalRef;
      }
    | 'message'
    | {
        context: ContextKind;
      };
  /**
   * The persisted UTC instant associated with the inbound message, when
   * the producer supplied one.
   *
   * An ordinary asynchronously delivered inbound message
   * ([`InboundKind::Message`]) carries the persisted instant of its
   * delivery; the producer supplies the original timestamp explicitly and
   * no wall-clock time is fabricated. Messages without a producer timestamp
   * remain representable: the field
   * defaults to `None` on deserialization and is omitted from the
   * canonical encoding while absent.
   */
  timestamp?: string | null;
}
/**
 * The latest observed finite drain boundary.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InboundDrainView".
 */
export interface InboundDrainView {
  /**
   * A conversation-scoped inbound sequence number.
   *
   * The sequence identifies one item of the conversation's inbound ordering
   * domain. It is **not**
   * [`RuntimeEventEnvelope`](crate::events::types::RuntimeEventEnvelope)`::sequence`
   * and is never allocated from the Event Journal sequence; the durable
   * Pending Inbound Inbox owns allocation, and the first successful
   * acceptance of a conversation receives `1`.
   */
  watermark: string;
  /**
   * The number of drained items.
   */
  count: number;
}
/**
 * Finite display identity of an actionable native request. A missing body is
 * an explicit deferred source, never an empty or reconstructed Questionnaire.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientInteractionHeader".
 */
export interface RuntimeClientInteractionHeader {
  /**
   * Exact native source offset, stable for this pending lifetime.
   */
  source_offset: number;
  interaction: InteractionRef;
  source: InteractionSource;
  request_id: InteractionId;
  conversation_id: ConversationId;
  execution: InteractionExecution;
  kind: RuntimeClientInteractionKind;
  approval?: RuntimeClientApprovalSummary | null;
  body?: InteractionRequest | null;
  body_complete: boolean;
  workflow_node?: WorkflowNodeInstance | null;
  workflow_node_deferred: boolean;
  workflow_node_path?: string[] | null;
}
/**
 * Finite decision metadata; business arguments remain an exact deferred native source.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientApprovalSummary".
 */
export interface RuntimeClientApprovalSummary {
  tool_id?: ToolId | null;
  tool_name: string;
  reason: string;
  mode: ToolInvocationMode;
  deferred_fields: string[][];
}
/**
 * One live interaction request projected to a Runtime Client.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InteractionRequest".
 */
export interface InteractionRequest {
  /**
   * Identifies one process-owned native human interaction.
   *
   * Interaction identities are derived from the conversation runtime's
   * non-reused [`AttemptId`] domain plus an ordinal owned by the
   * interaction coordinator.  They are deliberately not allocated from
   * a process-local counter alone: a response retained by a client from a
   * crashed process can therefore never name a different interaction in a
   * restarted runtime.
   */
  id: string;
  /**
   * The conversation that owns the interaction.
   */
  conversation_id: string;
  /**
   * One truthful native interaction owner. Scheduled work has no model turn.
   */
  execution:
    | {
        attempt_id: AttemptId;
        turn: number;
        type: 'attempt';
      }
    | {
        run: WorkflowRunId;
        type: 'scheduled';
      };
  /**
   * The bounded interaction facts.
   */
  kind:
    | {
        review: ReviewSpecification;
        subject_digest: string;
        type: 'review';
      }
    | {
        /**
         * Caller correlation for one native invocation. Capability source is
         * independently represented by [`ToolOrigin`].
         */
        invocation_id:
          | {
              call_id: ToolCallId;
              caller: 'agent';
            }
          | {
              node: WorkflowNodeInstance;
              caller: 'workflow';
            };
        /**
         * Identifies a tool definition in the capability set.
         */
        tool_id: string;
        /**
         * The safe model-facing tool name.
         */
        tool_name: string;
        /**
         * The registry-resolved tool origin.
         */
        origin:
          | 'builtin'
          | {
              mcp: {
                server_id: McpServerId;
              };
            }
          | {
              managed_python: {
                package: string;
              };
            };
        /**
         * The resolved execution ownership of one canonical invocation.
         *
         * The runtime resolves the effective mode from the tool's declared
         * [`ToolExecutionPolicy`] and (for `ModelSelectable`) the required
         * `execution_mode` invocation field, and delivers it to the executor as
         * canonical invocation data. The field is consumed before business-argument
         * validation and never reaches an executor. No policy resolution happens
         * inside executors.
         */
        mode: 'foreground' | 'background';
        /**
         * The already validated business arguments.  This is descriptive
         * data only; it is never accepted back as replacement input.
         */
        arguments: {
          [k: string]: unknown;
        };
        /**
         * The native policy's bounded explanation for asking.
         */
        reason: string;
        type: 'approval';
      }
    | {
        invocation_id: ToolInvocationId;
        requester: InteractionRequester1;
        questionnaire: QuestionnaireSpecification1;
        type: 'questionnaire';
      };
}
/**
 * The canonical identity of the tool that asked — a native tool, or
 * an MCP-served tool whose `ToolOrigin` names its server. Every
 * Runtime Client renders the requester from these facts and never
 * infers the source from the prompt text.
 */
export interface InteractionRequester1 {
  /**
   * Identifies a tool definition in the capability set.
   */
  tool_id: string;
  /**
   * The safe model-facing tool name.
   */
  tool_name: string;
  /**
   * The registry-resolved tool origin, which carries MCP server identity.
   */
  origin:
    | 'builtin'
    | {
        mcp: {
          server_id: McpServerId;
        };
      }
    | {
        managed_python: {
          package: string;
        };
      };
}
/**
 * The complete immutable facts shown to the Runtime Client.
 */
export interface QuestionnaireSpecification1 {
  /**
   * One to four related blocking questions.
   */
  questions: QuestionSpecification[];
}
/**
 * The external background execution read model.
 *
 * Projected from the authoritative [`ConversationBackgroundRegistry`]
 * ([`crate::tools::background::ConversationBackgroundRegistry`]); the
 * container shape belongs to the Runtime Client protocol while the
 * lifecycle, progress, and result leaf types are stable runtime-owned
 * value contracts. No internal task handles or process ids ever appear.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientJob".
 */
export interface RuntimeClientJob {
  deferred_fields?: string[][];
  /**
   * Native Bash presentation, derived from the canonical invocation.
   */
  bash?: BashPresentation | null;
  /**
   * The detached runtime execution identity.
   */
  job_id: string;
  /**
   * The canonical tool identity.
   */
  tool_id?: ToolId | null;
  /**
   * The model-facing tool name.
   */
  tool_name: string;
  /**
   * The public lifecycle of one background execution.
   *
   * Terminal states are absorbing. The allowed public transitions are:
   *
   * ```text
   * Starting  → Running
   * Starting  → Cancelling
   * Starting  → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
   * Running   → Cancelling
   * Running   → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
   * Starting / Running / Cancelling → PublishingTerminal
   * Cancelling → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
   * PublishingTerminal → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
   * ```
   *
   * The terminal vocabulary claims only facts the executor proved: `Failed`
   * is a *known* failure, `Denied` is policy/approval refusal (not `Failed`),
   * `TimedOut` is a proven deadline settlement, and
   * `OutcomeUnknown` means the executor could not prove the external outcome
   * at all. A terminal lifecycle never claims a stronger external outcome
   * than the canonical [`ToolExecutionStatus`] of the stored result proves —
   * in particular an unknown outcome is never labelled `Failed`, and a
   * cancellation request that could not be confirmed physically never
   * becomes `Cancelled`.
   *
   * [`BackgroundLifecycle::PublishingTerminal`] is the honest non-terminal
   * state in which the executor has returned its terminal candidate and the
   * registry now owns durable terminal publication; it is never `Running`:
   * the executor has returned and the runner now owns the durable settlement
   * continuation, retaining the settlement candidate until publication
   * reaches a terminal outcome. An internal unpublished prepared
   * state implements dispatch atomicity but never leaks as an accepted
   * execution.
   */
  state:
    | 'starting'
    | 'running'
    | 'cancelling'
    | 'publishing_terminal'
    | 'succeeded'
    | 'failed'
    | 'denied'
    | 'cancelled'
    | 'timed_out'
    | 'outcome_unknown';
  /**
   * The latest bounded progress, when any was reported.
   */
  progress?: ToolProgress | null;
  /**
   * The bounded terminal result, when terminal.
   */
  result?: ToolExecutionResult | null;
}
/**
 * Presentation metadata derived from canonical native Bash arguments.
 * Command remains the authoritative execution fact.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "BashPresentation".
 */
export interface BashPresentation {
  command: string;
  description?: string | null;
}
/**
 * One durable child Agent, keyed by `agent_id` across every activation.
 *
 * The registry owns lifecycle arbitration. Journal ownership facts rebuild
 * identity and activation correlation; canonical child history owns content.
 * `activation_id` identifies the latest activation, while `current_activation`
 * is absent when inactive or reserving the next activation. Neither replaces
 * the stable Agent identity. Activity is bounded observation, never authority.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientAgent".
 */
export interface RuntimeClientAgent {
  deferred_fields?: string[][];
  title: string;
  parent_agent_id: AgentId;
  /**
   * Identifies one finite activation owned by an Agent, or one finite
   * Workflow child execution.
   *
   * Repeated native Agent activations have distinct `SubagentId`s while
   * retaining the same durable `AgentId`. This is not an OS pid: process
   * identity or PID reuse can never establish durable ownership or proof
   * of physical settlement.
   */
  activation_id: string;
  /**
   * Identifies an Agent independently of its finite activations.
   * For a continuable child, this is the durable Agent-domain identity of
   * its child Conversation and remains unchanged across resume.
   */
  agent_id: string;
  /**
   * The child's own durable conversation identity.
   */
  child_conversation_id: string;
  /**
   * The canonical named-agent identity frozen at start (Issue #144).
   */
  agent: string;
  /**
   * The deterministic definition digest frozen at start (Issue #144).
   *
   * A client observing an already-running child sees the definition it
   * actually started with, so a later configuration adoption that redefines the
   * same agent name can never be mistaken for a change to that child.
   */
  definition_digest: string;
  /**
   * The deterministic **effective execution profile** digest frozen at
   * child start (Issue #258).
   *
   * It distinguishes two children of one named agent that an authorized
   * invocation override specialized differently — the same
   * `definition_digest`, different effective tools, Skills, or extensions.
   * It is committed with durable ownership, so it survives a restart
   * unchanged and a recovery-projected child reports the same value a live
   * one does.
   *
   * It is a bounded correlation identity and nothing more: no effective
   * selection, prompt, Skill body, or materialization detail is projected
   * with it, and no authority decision reads it.
   */
  profile_digest: string;
  /**
   * The authoritative lifecycle state.
   */
  state: ('admitting' | 'active' | 'stopping' | 'inactive') | 'unavailable';
  current_activation?: SubagentId | null;
  activation_state: SubagentState;
  /**
   * The bounded terminal failure/cancellation diagnostic, once known.
   *
   * A successful child's answer content never appears here (Issue
   * #178): the durable terminal inbound publication is the one result
   * channel.
   */
  detail?: string | null;
  observation: SubagentObservation;
  /**
   * The redacted execution profile frozen at child start (Issue #178);
   * `None` for recovery-projected records. Named `execution_profile`
   * on the wire: the bare `profile` key was the obsolete pre-#144
   * profile-name field and stays retired.
   */
  execution_profile?: SubagentExecutionProfile | null;
  /**
   * When the ownership committed; clients derive elapsed time from it.
   */
  started_at: string;
  /**
   * The Session execution binding this Agent froze at creation.
   */
  workspace: string;
}
/**
 * The latest live activity projection reported by the child (Issue
 * #178). Latest-value coalesced; never a lifecycle authority.
 */
export interface SubagentObservation {
  /**
   * The child-owned projection revision; strictly increasing per applied
   * transition.
   */
  revision: string;
  /**
   * Exact child-owned current Attempt; absent before start and after settlement.
   */
  attempt_id?: AttemptId | null;
  /**
   * What the child is observably doing right now.
   */
  activity:
    | {
        type: 'awaiting_activity';
      }
    | {
        /**
         * Identifies one actual provider-neutral model request.
         *
         * A request identity is distinct from an attempt, turn, retry ordinal,
         * and Event Journal sequence. Primary requests derive the `request:`
         * namespace from immutable [`RequestIdentity`](crate::model::snapshot::RequestIdentity).
         * Auxiliary requests allocate the separate `req_` `UUIDv7` namespace with
         * [`RequestId::generate_auxiliary`]. Each physical retry is a distinct
         * request. Identity alone grants no execution permission.
         */
        request_id: string;
        /**
         * The retry ordinal of THIS request: 0 for a first attempt, `n`
         * for the request the nth scheduled retry armed.
         */
        retry: number;
        type: 'model';
      }
    | {
        /**
         * The scheduled retry ordinal.
         */
        retry: number;
        type: 'retrying_model';
      }
    | {
        active: number;
        representative?: SubagentToolRepresentative | null;
        type: 'tool_group';
      }
    | {
        type: 'compacting';
      }
    | {
        /**
         * What the child waits on.
         */
        on:
          | {
              type: 'review';
            }
          | {
              /**
               * The tool awaiting approval.
               */
              tool_id?: ToolId | null;
              type: 'approval';
            }
          | {
              type: 'questionnaire';
            };
        type: 'waiting';
      };
  /**
   * When the latest applied transition was folded (child-side clock).
   */
  last_activity_at?: string | null;
  counters: SubagentActivityCounters;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SubagentToolRepresentative".
 */
export interface SubagentToolRepresentative {
  tool_call_id: ToolCallId;
  tool_id: ToolId;
  progress?: ToolProgress | null;
}
/**
 * Cumulative transition counters since the child started.
 */
export interface SubagentActivityCounters {
  /**
   * Model requests started.
   */
  model_requests: number;
  /**
   * Model retry schedules observed (cumulative count).
   */
  model_retries: number;
  /**
   * Tool executions finished (completed plus failed).
   */
  tool_executions: number;
}
/**
 * The safe, redacted execution profile of one child, frozen at child start
 * (Issue #178).
 *
 * Derived from the frozen model authority exactly once by
 * [`SubagentExecutionProfile::from_frozen`]: it carries only the effective
 * model identity, Model Profile, and reasoning state. Credentials, endpoints, provider
 * bindings, and every other binding internal are never projected.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SubagentExecutionProfile".
 */
export interface SubagentExecutionProfile {
  /**
   * The effective fully qualified model reference (`provider/model`).
   */
  model: string;
  /**
   * The selected Model Profile, when the model declares any.
   */
  profile?: ModelProfileId | null;
  /**
   * Whether the invocation semantically enables reasoning.
   */
  reasoning_enabled: boolean;
}
/**
 * The structured Agent Status view of one composition.
 *
 * Derived from the exact composed status the model path consumed: the
 * structured sections and the canonical rendered representation originate
 * from the same composition, so a client never parses the rendered text
 * to recover structure and never triggers a second composition.
 *
 * The view also carries the runtime facts that place the composition in
 * conversation order: the eligible
 * [`opportunities`](Self::opportunities), each with the durable identity it
 * was established against. Placement is a runtime fact because only the
 * runtime knows it, and it is frozen where it is determined rather than
 * reconstructed downstream; how a client draws a status at that place is
 * presentation and stays entirely outside this type.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentStatusView".
 */
export interface AgentStatusView {
  /**
   * Identifies one attempt to execute an agent manifest.
   */
  attempt_id: string;
  /**
   * The turn number of the request preparation.
   */
  turn: number;
  /**
   * The canonical Agent Status User message described by this view. It is
   * the stable identity of the composition: observing the same
   * `status_message_id` twice describes one composition, never two.
   */
  status_message_id?: MessageId | null;
  source_locator?: CanonicalContentLocator | null;
  request_id?: RequestId | null;
  rendered_ref?: ContentRef | null;
  opportunities: AgentStatusOpportunityView;
  /**
   * The ordered structured sections.
   */
  sections: RuntimeClientStatusSection[];
  /**
   * The canonical rendered representation, derived from the same
   * composition as the sections.
   *
   * It exists for diagnostics and for proving that a client and the model
   * saw one composition. It is **not** a presentation source: a client
   * renders [`sections`](Self::sections) and never parses this text back
   * into structure.
   */
  rendered: string;
}
/**
 * The delivery opportunities that made this generation eligible, each
 * carrying its own placement fact.
 *
 * The Agent Status Context message is request-scoped model history: it
 * carries no transcript cursor of its own and never becomes a transcript
 * item, so placement has to be published or it cannot be known. Each
 * opportunity publishes the identity it was established against —
 * `FreshInbound` the exact inbound message, `PostToolBatch` the durable
 * position of its settled tool batch — and both were frozen by the
 * semantic owner at that establishment, not sampled when this
 * observation was folded.
 */
export interface AgentStatusOpportunityView {
  /**
   * The `FreshInbound` opportunity that produced this status, when one is
   * present. Future delivery opportunities can be added alongside it
   * without making this member structurally mandatory.
   */
  fresh_inbound?: FreshInboundOpportunityView | null;
  /**
   * The complete settled tool batch that made this existing primary step
   * eligible, when present.
   */
  post_tool_batch?: PostToolBatchOpportunityView | null;
}
/**
 * The external view of one `FreshInbound` status opportunity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "FreshInboundOpportunityView".
 */
export interface FreshInboundOpportunityView {
  /**
   * The inbound message that made status generation eligible.
   */
  target_message_id?: MessageId | null;
  target_locator?: CanonicalContentLocator | null;
  transcript_anchor?: RuntimeClientTranscriptCursor | null;
  target_metadata_ref?: ContentRef | null;
}
/**
 * The external view of one `PostToolBatch` status opportunity.
 *
 * The opportunity itself remains a marker with no durable or scheduling
 * metadata. What it carries here is the one ordering fact that places a
 * composition made from it: the durable transcript position of the canonical
 * `ToolResult` batch that established it, frozen by the Agent Loop at that
 * batch's commit.
 *
 * The freeze point matters and is the whole reason this is a published fact
 * rather than something a client or the projection reconstructs. A status is
 * composed at the primary-step preparation that consumes this opportunity,
 * but it is not observed until the durable model-turn-start commit lands,
 * and inbound acceptance is an independent durable boundary that may commit
 * in between. Anything that read "the newest durable position" at fold time
 * would place the status after an unrelated inbound turn.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "PostToolBatchOpportunityView".
 */
export interface PostToolBatchOpportunityView {
  /**
   * The durable position of the settled `ToolResult` batch this
   * opportunity belongs to.
   *
   * `None` only when that batch committed no visible transcript item, in
   * which case the composition carries no transcript-position placement
   * and a client draws no annotation for it.
   */
  transcript_anchor?: RuntimeClientTranscriptCursor | null;
}
/**
 * Exactly the bounded Background presentation accepted by the Status owner.
 * Job invocation bodies and terminal results remain in the native Job read domain.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientStatusBackgroundExecution".
 */
export interface RuntimeClientStatusBackgroundExecution {
  job_id: ToolExecutionId;
  tool_name: string;
  state: BackgroundLifecycle;
  progress?: ToolProgress | null;
}
/**
 * One bounded Todo task in the Agent Status client view.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientTodoStatusTask".
 */
export interface RuntimeClientTodoStatusTask {
  /**
   * The conversation-owned task id.
   */
  id: string;
  /**
   * The bounded task subject.
   */
  subject: string;
  /**
   * The bounded in-progress label, when present.
   */
  active_form?: string | null;
  /**
   * The committed lifecycle status.
   */
  status: 'pending' | 'in_progress' | 'completed' | 'deleted';
  /**
   * Whether an active dependency still blocks this task.
   */
  blocked: boolean;
}
/**
 * Runtime-owned context-compaction diagnostics. The values describe
 * committed `RuntimeEvent::CompactionCompleted` facts by identity;
 * they are never a second Conversation Surface authority and never
 * carry summary content (the committed summary itself appears in
 * `messages`, like every other canonical Ledger fact).
 */
export interface RuntimeClientContextView {
  /**
   * Independent current historical projection, including inherited activations.
   */
  projection_head?: RuntimeClientProjectionHeadView | null;
  /**
   * Latest manual operation admitted in this runtime incarnation. Never inferred
   * from a newer canonical checkpoint. Absence is insufficient evidence.
   */
  manual_compaction?: RuntimeClientManualCompaction | null;
  /**
   * Latest pre-commit diagnostic; cleared when the next compaction starts.
   */
  compaction_error?: string | null;
  /**
   * Latest native measured request or committed compacted-context estimate.
   */
  occupancy?: RuntimeClientContextOccupancy | null;
  /**
   * Whether the runtime currently owns a context-compaction operation.
   * This is live operation state, not inferred from token usage.
   */
  compaction_in_progress: boolean;
  /**
   * Runtime Client projection statistic: the number of committed
   * compaction completions folded into this read model. The compaction
   * generation remains the conversation-owned identity.
   */
  compaction_count: number;
  /**
   * The latest committed compaction metadata, when compaction occurred.
   */
  latest_compaction?: RuntimeClientCompactionView | null;
}
/**
 * Bounded read evidence for the most recently admitted manual compaction.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientManualCompaction".
 */
export interface RuntimeClientManualCompaction {
  /**
   * Caller correlation; absent for an uncorrelated native caller.
   */
  request_id?: ManualCompactionRequestId | null;
  /**
   * False until coordinator state is restored and maintenance released.
   */
  released: boolean;
  /**
   * Exact pre-commit failure diagnostic, if released unsuccessfully.
   */
  error?: string | null;
}
/**
 * The active capability projection.
 */
export interface CapabilityView {
  /**
   * Configured Tool intent before effective model gating.
   */
  configured_tools: RuntimeClientTool[];
  /**
   * The active monotonic capability revision.
   */
  revision: string;
  /**
   * The deterministic active Tool catalog in registry order. Model
   * requests and execution use exactly this set.
   */
  tools?: RuntimeClientTool[];
  /**
   * The complete available Tool catalog, including inactive Tools. The
   * active set above is an explicit subset; availability never implies
   * model activation.
   */
  available_tools?: RuntimeClientTool[];
  /**
   * The deterministic model-visible Skill catalog ordered by Skill name.
   * Skills hidden by `disable-model-invocation` remain runtime-owned but
   * are omitted here. Every entry includes the canonical absolute host
   * path of its `SKILL.md`.
   */
  skills?: RuntimeClientSkill[];
  /**
   * The typed availability of every evaluated optional capability
   * source, in deterministic source-identity order (Issue #81).
   */
  sources?: CapabilitySourceView[];
}
/**
 * One external tool catalog entry.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientTool".
 */
export interface RuntimeClientTool {
  /**
   * Identifies a tool definition in the capability set.
   */
  id: string;
  /**
   * The stable model-facing tool name.
   */
  name: string;
  /**
   * The human-readable description.
   */
  description: string;
  /**
   * The canonical JSON Schema of accepted arguments.
   */
  input_schema: {
    [k: string]: unknown;
  };
  /**
   * Who owns an invocation: attempt (foreground) or conversation
   * (background).
   */
  execution_policy: 'foreground_only' | 'background_only' | 'model_selectable';
  /**
   * How calls within one batch are scheduled.
   */
  concurrency_policy: 'sequential' | 'parallel';
  /**
   * Whether execution requires a native approval interaction.
   */
  approval_policy: 'never' | 'always';
  /**
   * The replay policy.
   */
  replay_policy: 'never' | 'idempotent';
  /**
   * Where the tool comes from.
   */
  origin:
    | 'builtin'
    | {
        mcp: {
          server_id: McpServerId;
        };
      }
    | {
        managed_python: {
          package: string;
        };
      };
}
/**
 * One external Skill catalog entry.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientSkill".
 */
export interface RuntimeClientSkill {
  /**
   * The validated standard Skill identity.
   */
  id: string;
  /**
   * The immutable Skill version identity.
   */
  version_id: string;
  /**
   * The validated standard Skill name.
   */
  name: string;
  /**
   * The validated standard Skill description.
   */
  description: string;
  /**
   * The canonical absolute host path of the package's `SKILL.md`.
   *
   * This is a real filesystem path, not a runtime-owned virtual locator:
   * a Skill package is an ordinary host directory, and the same path
   * serves Read, Bash, Grep, and Glob alike.
   */
  location: string;
}
/**
 * One optional capability source's availability projection.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CapabilitySourceView".
 */
export interface CapabilitySourceView {
  /**
   * The stable source identity.
   */
  source:
    | {
        package: string;
        type: 'managed_python';
      }
    | {
        /**
         * Identifies an MCP server bound to the runtime.
         */
        server_id: string;
        type: 'mcp';
      };
  /**
   * The authoritative availability state.
   */
  state:
    | {
        type: 'unprepared';
      }
    | {
        type: 'ready';
      }
    | {
        type: 'unavailable';
      };
}
/**
 * The active runtime resource generation: the project context files
 * the runtime actually loaded, and whether an agent profile is frozen
 * into the generation.
 *
 * This is deliberately separate from [`CapabilityView`]: a
 * resource publication advances the resource revision while the
 * capability revision stays put. Nothing here is conversation content
 * — a project context file is request input the runtime assembles into
 * the Effective System Prompt, never a canonical Ledger message.
 */
export interface RuntimeClientResourcesView {
  /**
   * Facts copied from the same immutable generation as this revision.
   * Deferred inspection facts are loaded through finite native field pages.
   */
  inspection?: CapabilityInspection | null;
  /**
   * Identifies one immutable process-local runtime resource generation.
   *
   * This is deliberately separate from `ContextGeneration` (one context
   * assembly provenance set) and `CapabilityRevision` (one executable
   * capability set). A resource-only change may advance this revision while
   * retaining an identical capability revision.
   */
  revision?: string;
  /**
   * The runtime-loaded project instruction files, root-most to
   * workspace, in the exact order the runtime concatenated them.
   */
  context_files?: RuntimeClientContextFile[];
  /**
   * Whether an immutable agent profile/persona is frozen into this
   * generation. The persona text itself is runtime-owned request input.
   */
  agent_profile?: boolean;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CapabilityInspection".
 */
export interface CapabilityInspection {
  definitions: ResourceDefinition[];
  resource_diagnostics: ResourceDiagnostic[];
  main?: AgentInspection | null;
  agents: {
    [k: string]: AgentInspection;
  };
  workflows: {
    [k: string]: WorkflowInspection;
  };
  sources: {
    [k: string]: SourceInspection;
  };
  skills: SkillProvenance[];
  skill_diagnostics: SkillDiagnostic[];
}
/**
 * A defined identity is independent of selection and materialization. Clients
 * present these native facts without discovering paths or computing overlays.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ResourceDefinition".
 */
export interface ResourceDefinition {
  family: ResourceFamily;
  name: string;
  location: ResourceLocation;
  valid: boolean;
}
/**
 * Frozen authored ownership of a complete resource identity. Discovery records
 * the losing location without parsing or borrowing any of its fields.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ResourceLocation".
 */
export interface ResourceLocation {
  scope: SourceScope;
  path: string;
  shadowed?: string | null;
}
/**
 * Bounded resource diagnostics expose source ownership, never source contents.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ResourceDiagnostic".
 */
export interface ResourceDiagnostic {
  subject: ResourceDiagnosticSubject;
  file?: string | null;
  /**
   * The authored field the loader located the failure at. It locates the
   * failure inside `file`; it is not an identity.
   */
  field: string;
  reason: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentInspection".
 */
export interface AgentInspection {
  identity: AgentIdentity;
  source?: string | null;
  tools: ToolInspection[];
  /**
   * The runtime-composed Agent control group. It is derived from the
   * admitted `agents` alone, never from `tool_selection`, and is either
   * empty or all five controls.
   */
  agent_controls: ToolInspection[];
  tool_selection: AgentToolSelection[];
  skills: SkillProvenance[];
  agents: SubagentName[];
  workflows: WorkflowId[];
  plugins: ExtensionInspection[];
  diagnostics: AgentProfileDiagnostic[];
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolInspection".
 */
export interface ToolInspection {
  id: ToolId;
  name: string;
  origin: ToolOrigin;
}
/**
 * The provenance record of one effective Skill identity.
 *
 * This is generation/inspection metadata. It deliberately never enters the
 * model-facing catalog: a Skill's instructions are not improved by knowing
 * which root won it.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SkillProvenance".
 */
export interface SkillProvenance {
  /**
   * The effective logical Skill identity.
   */
  name: string;
  /**
   * The source that owns the effective package.
   */
  source: 'user' | 'workspace';
  /**
   * The effective package's `SKILL.md` location.
   */
  location: string;
  /**
   * Valid same-identity packages that lost the merge, in canonical order.
   */
  shadowed: ShadowedSkill[];
}
/**
 * One valid package that a higher-precedence source shadowed.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ShadowedSkill".
 */
export interface ShadowedSkill {
  /**
   * The source whose package was shadowed.
   */
  source: 'user' | 'workspace';
  /**
   * The shadowed package's `SKILL.md` location.
   */
  location: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ExtensionInspection".
 */
export interface ExtensionInspection {
  identity: NativeExtension;
  active: boolean;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowAdmissionDiagnostic".
 */
export interface WorkflowAdmissionDiagnostic {
  path: string;
  reason: WorkflowDependencyFailure;
}
/**
 * One runtime-loaded project instruction file.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientContextFile".
 */
export interface RuntimeClientContextFile {
  /**
   * The canonical absolute host path the runtime read.
   */
  path: string;
  /**
   * The exact byte length of the loaded content.
   */
  bytes: number;
}
/**
 * The redacted client-facing projection of the session model state.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionModelView".
 */
export interface SessionModelView {
  configured: SessionModelConfig1;
  /**
   * The effective primary invocation last resolved for `configured`.
   * Absent only when the Session was loaded with an already unavailable
   * selection.
   */
  effective?: ModelInvocationView1 | null;
  /**
   * The summary policy last resolved for `configured`, absent exactly
   * when `effective` is.
   */
  summary?: AuxiliaryModelView | null;
  /**
   * The independent Title policy last resolved for this configuration.
   */
  title?: AuxiliaryModelView | null;
  classification?: AuxiliaryModelView | null;
  btw?: AuxiliaryModelView | null;
  /**
   * Why the Session's published Model Catalog does not admit
   * `configured`. While present, `effective` and `summary` are display
   * facts only: no new Attempt is admitted until a valid selection is
   * committed, and nothing falls back to a default.
   */
  unavailable?: string | null;
}
/**
 * The authoritative mutable model configuration of one conversation
 * session.
 *
 * This one type is the session's state, the `model_get` result, and the
 * `model_set` parameter: an update is a whole-state replacement, never an
 * ambiguous JSON patch.
 */
export interface SessionModelConfig1 {
  /**
   * The selected catalog model.
   */
  model: string;
  /**
   * The selected Model Profile; the model's `default_profile` is used
   * when absent.
   */
  profile?: ModelProfileId | null;
  /**
   * The session request-parameter overrides.
   */
  requestParams?: {
    [k: string]: unknown;
  };
  /**
   * The session output-budget override; the selected profile's default,
   * else the model's hard maximum, is used when absent.
   */
  maxOutputTokens?: number | null;
  /**
   * The compaction summary model policy.
   */
  summaryModel?:
    | {
        mode: 'session';
      }
    | {
        /**
         * The catalog model reference.
         */
        model: string;
        /**
         * The selected Model Profile; the model's `default_profile` is used
         * when absent.
         */
        profile?: ModelProfileId | null;
        /**
         * The explicit auxiliary request-parameter overrides.
         */
        request_params?: {
          [k: string]: unknown;
        };
        /**
         * The explicit auxiliary output-budget override.
         */
        max_output_tokens?: number | null;
        mode: 'explicit';
      };
  /**
   * The independent automatic title model policy.
   */
  titleModel?:
    | {
        mode: 'session';
      }
    | {
        /**
         * The catalog model reference.
         */
        model: string;
        /**
         * The selected Model Profile; the model's `default_profile` is used
         * when absent.
         */
        profile?: ModelProfileId | null;
        /**
         * The explicit auxiliary request-parameter overrides.
         */
        request_params?: {
          [k: string]: unknown;
        };
        /**
         * The explicit auxiliary output-budget override.
         */
        max_output_tokens?: number | null;
        mode: 'explicit';
      };
  classificationModel?: AuxiliaryModelPolicy;
  btwModel?: AuxiliaryModelPolicy;
}
/**
 * The redacted client-facing projection of one resolved model invocation.
 *
 * It carries no credential, no adapter object, no provider HTTP client, and
 * no synchronization identity. The effective request parameters *are*
 * exposed: they are provider-owned configuration a model-control client
 * needs, and they can never contain credential material because a
 * credential is never a request parameter.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelInvocationView".
 */
export interface ModelInvocationView1 {
  /**
   * An authored Model identity. Its spelling has no provider or wire semantics.
   */
  model: string;
  /**
   * The model interaction protocol an adapter must speak.
   */
  protocol: 'openai_chat_completions' | 'openai_responses' | 'anthropic_messages';
  /**
   * The model's context window in tokens.
   */
  contextWindow: number;
  /**
   * The model's configured maximum output tokens.
   */
  modelMaxOutputTokens: number;
  /**
   * The effective output budget.
   */
  maxOutputTokens: number;
  /**
   * The selected Model Profile, when the model declares any.
   */
  profile?: ModelProfileId | null;
  /**
   * Whether reasoning is semantically enabled.
   */
  reasoningEnabled: boolean;
  /**
   * The effective opaque provider request parameters.
   */
  requestParams?: {
    [k: string]: unknown;
  };
  capabilities: ModelCapabilities;
  declaredCapabilities: ModelCapabilities1;
}
/**
 * Derived availability of the existing native Session model authority.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientModelStatus".
 */
export interface RuntimeClientModelStatus {
  available: boolean;
  body_deferred: boolean;
  /**
   * Existing native Session binding revision; absent for an unbound read source.
   */
  binding_revision?: string | null;
}
/**
 * The complete state of one conversation's list.
 *
 * This is the persistence format: it is what a successful mutation
 * publishes, and it is what [`ConversationTodoList::rebuilt`] reads back.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TodoSnapshot".
 */
export interface TodoSnapshot {
  /**
   * Every task, tombstones included, in creation order.
   */
  tasks?: TodoTask[];
  /**
   * The id the next created task will receive.
   */
  next_id: string;
}
/**
 * One task of the conversation's list.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TodoTask".
 */
export interface TodoTask {
  /**
   * The task id, unique within the current list generation.
   *
   * `clear` resets the allocator, so an id names one task for as long as
   * the list it belongs to lives — not for as long as the conversation
   * does.
   */
  id: string;
  /**
   * The imperative one-line subject.
   */
  subject: string;
  /**
   * Optional long-form detail.
   */
  description?: string | null;
  /**
   * Optional present-continuous label, shown while the task is in
   * progress.
   */
  active_form?: string | null;
  /**
   * The lifecycle status.
   */
  status: 'pending' | 'in_progress' | 'completed' | 'deleted';
  /**
   * The ids this task waits on, ascending and deduplicated.
   */
  blocked_by?: string[];
  /**
   * Optional free-form owner label.
   */
  owner?: string | null;
  /**
   * Optional free-form metadata. An empty record is dropped entirely.
   */
  metadata?: {
    [k: string]: unknown;
  } | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientGoalView".
 */
export interface RuntimeClientGoalView {
  current?: RuntimeClientGoalSnapshot | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientGoalSnapshot".
 */
export interface RuntimeClientGoalSnapshot {
  reference: GoalRef;
  objective: string;
  objective_deferred: boolean;
  phase: GoalPhase;
  blocked_reason?: string | null;
  blocked_reason_deferred: boolean;
  autonomous_round_budget: number;
  autonomous_rounds_consumed: number;
  origin: RuntimeClientGoalOrigin;
  last_round_message_id?: MessageId | null;
  last_round_message_id_deferred: boolean;
  fields: RuntimeClientGoalFieldRef[];
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientGoalFieldRef".
 */
export interface RuntimeClientGoalFieldRef {
  reference: GoalRef;
  /**
   * Path in the existing indexed native `GoalView` value, not a credential.
   */
  path: string[];
}
/**
 * One Runtime Client event pushed on the observation stream.
 *
 * The envelope carries the cursor the event was published at, so a client
 * resuming after a cursor always knows the exact stream position of every
 * observation. Notifications structurally carry no request id.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientProtocolEvent".
 */
export interface RuntimeClientProtocolEvent {
  /**
   * The external cursor of the Runtime Client observation stream.
   *
   * A cursor identifies one position in the externally visible
   * `RuntimeClientEvent` sequence: cursor `C` means "every Runtime Client
   * event through event `C` has been applied to the snapshot". It is:
   *
   * - monotonic within the Runtime Client observation stream;
   * - **not** an alias of `u64`;
   * - **not** the mailbox [`InboundSequence`](crate::runtime::inbound::InboundSequence);
   * - **not** [`RuntimeEventEnvelope::sequence`](crate::events::types::RuntimeEventEnvelope);
   * - **not** any future Event Journal sequence;
   * - owned by the runtime observation stream, so it survives attachment
   *   detach/reconnect;
   * - independent of its numeric representation: protocol versioning never
   *   derives anything from cursor encoding.
   *
   * Cursor allocation is committed by exactly one linearization owner (the
   * Runtime Client projection) together with event publication; overflow
   * fails explicitly and never wraps.
   */
  cursor: string;
  /**
   * The typed external event payload.
   */
  event:
    | {
        events: RuntimeClientEvent[];
        metadata: ProjectionMetadata;
        type: 'projection_transition';
      }
    | {
        transition: string;
        checkpoint: RuntimeClientCheckpoint;
        type: 'checkpoint_advanced';
      }
    | {
        position: number;
        attempt_id: AttemptId;
        message_id: MessageId;
        block_index: ContentBlockIndex;
        kind: string;
        base_length: string;
        end_length: string;
        content_revision: string;
        inline_delta?: string | null;
        content_ref: ContentRef;
        type: 'block_content_advanced';
      }
    | {
        projection_head?: RuntimeClientProjectionHeadView | null;
        transcript: RuntimeClientTranscriptPage;
        trace: TracePage;
        occupancy?: RuntimeClientContextOccupancy | null;
        todos?: TodoSnapshot | null;
        type: 'read_domains_updated';
      }
    | {
        type: 'trace_changed';
      }
    | {
        eligibility: AdoptionEligibility;
        type: 'configuration_adoption_eligibility_changed';
      }
    | {
        view: RuntimeClientGoalView;
        type: 'goal_changed';
      }
    | {
        workflows: WorkflowSnapshot1;
        type: 'workflows_updated';
      }
    | {
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        /**
         * The immutable model snapshot the attempt froze at admission.
         */
        model?: AttemptModelView | null;
        /**
         * Native frozen admission evidence, absent when unavailable.
         */
        execution_settings?: AdmittedSettings | null;
        type: 'attempt_started';
      }
    | {
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        /**
         * The platform-level settlement.
         */
        outcome:
          | {
              /**
               * The normalized finish reason.
               */
              finish_reason:
                | {
                    type: 'stop';
                  }
                | {
                    type: 'tool_calls';
                  }
                | {
                    type: 'length';
                  }
                | {
                    type: 'content_filter';
                  }
                | {
                    type: 'refusal';
                  }
                | {
                    /**
                     * The original provider reason, preserved for diagnostics.
                     */
                    reason: string;
                    type: 'other';
                  };
              /**
               * The original reason exceeds its finite inline preview.
               */
              finish_reason_deferred?: boolean;
              /**
               * Exact allowlisted reason bytes at the published native cut.
               */
              finish_reason_source?: JournalFieldRef | null;
              type: 'completed';
            }
          | {
              /**
               * Why the attempt was cancelled.
               */
              reason:
                | 'user_requested'
                | 'runtime_shutdown'
                | 'parent_cancelled'
                | 'subagent_execution_deadline_exceeded';
              type: 'cancelled';
            }
          | {
              type: 'timed_out';
            }
          | {
              /**
               * Which limit was exceeded.
               */
              limit: 'max_turns' | 'max_tool_calls' | 'max_runtime_seconds';
              type: 'limit_exceeded';
            }
          | {
              /**
               * The normalized client-visible failure of one attempt.
               *
               * This is the external projection of the internal [`AttemptFailure`]
               * ([`crate::events::types::AttemptFailure`]): provider-specific fields
               * (such as the raw provider error code) never appear here.
               */
              error:
                | {
                    /**
                     * Error classes the runtime distinguishes for retry/termination decisions.
                     * Provider SDK error structs never cross this boundary.
                     */
                    kind:
                      | 'invalid_request'
                      | 'authentication'
                      | 'rate_limit'
                      | 'timeout'
                      | 'transport'
                      | 'provider_error'
                      | 'context_window_exceeded'
                      | 'cancelled'
                      | 'unsupported'
                      | 'malformed_tool_proposal'
                      | 'generation_degenerated'
                      | 'generation_budget_exceeded';
                    /**
                     * The normalized human-readable message.
                     */
                    message: string;
                    /**
                     * The retry hint, when the provider reported one.
                     */
                    retry_after_ms?: string | null;
                    type: 'model';
                  }
                | {
                    /**
                     * The normalized runtime error.
                     */
                    error:
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'internal';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'invalid_state';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'unsupported';
                        }
                      | {
                          /**
                           * The tool name the model called.
                           */
                          name: string;
                          type: 'unknown_tool';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'durable_store';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'contract_violation';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'context_preparation_failed';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'context_compaction_failed';
                        }
                      | {
                          /**
                           * The policy's bounded rejection reason.
                           */
                          reason: string;
                          type: 'pre_step_rejected';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'pre_step_policy_failed';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'tool_result_observation_failed';
                        }
                      | {
                          /**
                           * The bounded recovery diagnostic: which durable evidence settled
                           * the attempt and what remained indeterminate.
                           */
                          message: string;
                          type: 'restart_interrupted';
                        }
                      | {
                          /**
                           * Human-readable diagnostic message.
                           */
                          message: string;
                          type: 'deferred_context_rejected';
                        };
                    type: 'runtime';
                  };
              /**
               * The normalized diagnostic exceeds its finite inline preview.
               */
              error_deferred?: boolean;
              /**
               * Normalized diagnostic source; never a raw provider error.
               */
              error_source?: JournalFieldRef | null;
              type: 'failed';
            };
        type: 'attempt_settled';
      }
    | {
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        /**
         * The exact folded number of completed turns.
         */
        turn: number;
        type: 'attempt_turn_updated';
      }
    | {
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        usage: ModelUsage1;
        type: 'attempt_usage_updated';
      }
    | {
        owner: string;
        cut: ObservationCollectionCut;
        head: RuntimeClientInteractionHeader[];
        type: 'pending_head_changed';
      }
    | {
        interaction: RuntimeClientInteractionHeader1;
        type: 'interaction_pending';
      }
    | {
        interaction: InteractionRef1;
        /**
         * The exact terminal rendezvous outcome.
         */
        outcome:
          | {
              kind: ToolDeadlineKind;
              type: 'deadline_expired';
            }
          | {
              /**
               * A typed response to one native interaction.
               */
              response:
                | {
                    response: ReviewResponse;
                    type: 'review';
                  }
                | {
                    /**
                     * The finite approval decision.  It has no tool arguments.
                     */
                    decision:
                      | {
                          type: 'allow';
                        }
                      | {
                          /**
                           * A bounded client-facing reason.
                           */
                          reason: string;
                          type: 'deny';
                        };
                    type: 'approval';
                  }
                | {
                    /**
                     * A submitted answer set or explicit decline.
                     */
                    response:
                      | {
                          type: 'submitted';
                          value: QuestionnaireSubmission;
                        }
                      | {
                          type: 'declined';
                        };
                    type: 'questionnaire';
                  };
              type: 'responded';
            }
          | {
              /**
               * The first-winner cancellation cause from the owning attempt.
               */
              reason:
                | 'user_requested'
                | 'runtime_shutdown'
                | 'parent_cancelled'
                | 'subagent_execution_deadline_exceeded';
              type: 'cancelled';
            };
        type: 'interaction_settled';
      }
    | {
        interaction: InteractionRef2;
        type: 'interaction_removed';
      }
    | {
        audit: RuntimeClientTranscriptInteractionRequested;
        /**
         * The cursor domain of durable transcript paging.
         */
        transcript_cursor: string;
        type: 'interaction_audit_requested';
      }
    | {
        audit: RuntimeClientTranscriptInteractionSettled;
        /**
         * The cursor domain of durable transcript paging.
         */
        transcript_cursor: string;
        type: 'interaction_audit_settled';
      }
    | {
        context: RuntimeClientContextView1;
        /**
         * The owning attempt for automatic compaction; absent for manual
         * idle maintenance.
         */
        attempt_id?: AttemptId | null;
        type: 'context_compaction_started';
      }
    | {
        context: RuntimeClientContextView2;
        /**
         * The owning attempt for automatic compaction; absent for manual
         * idle maintenance.
         */
        attempt_id?: AttemptId | null;
        /**
         * The runtime-owned failure diagnostic.
         */
        error: string;
        type: 'context_compaction_failed';
      }
    | {
        /**
         * The owning attempt for automatic compaction; absent for manual
         * idle maintenance.
         */
        attempt_id?: AttemptId | null;
        context: RuntimeClientContextView3;
        type: 'context_compacted';
      }
    | {
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        /**
         * Identifies a committed canonical message block.
         */
        message_id: string;
        type: 'assistant_message_started';
      }
    | {
        position: number;
        foreground_position: number;
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        /**
         * Identifies a committed canonical message block.
         */
        message_id: string;
        /**
         * A stable index identifying one content block within the ordered content
         * list of the canonical message being assembled.
         *
         * Streaming facts (text deltas, reasoning deltas, provider continuation
         * state, tool-call content) reference the block they belong to by this
         * index, so interleaved and multiple blocks remain unambiguous without
         * exposing any provider-specific block id type.
         */
        block_index: number;
        call: RuntimeClientToolCall;
        type: 'tool_call_started';
      }
    | {
        position: number;
        foreground_position: number;
        /**
         * Exact native assembled representation; clients must not reserialize JSON numbers.
         */
        arguments_json: string;
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        /**
         * Identifies a committed canonical message block.
         */
        message_id: string;
        /**
         * A stable index identifying one content block within the ordered content
         * list of the canonical message being assembled.
         *
         * Streaming facts (text deltas, reasoning deltas, provider continuation
         * state, tool-call content) reference the block they belong to by this
         * index, so interleaved and multiple blocks remain unambiguous without
         * exposing any provider-specific block id type.
         */
        block_index: number;
        call: RuntimeClientToolCall1;
        arguments_ref?: ContentRef | null;
        type: 'tool_call_assembled';
      }
    | {
        settlement_head: RuntimeClientPublicationSettlementRecord[];
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        audit: PublicationAudit1;
        /**
         * The cursor domain of durable transcript paging.
         */
        transcript_cursor: string;
        type: 'assistant_publication_settled';
      }
    | {
        settlement_head: RuntimeClientPublicationSettlementRecord[];
        attempt_id: AttemptId;
        settlement: RuntimeClientPublicationSettlement;
        /**
         * Bounded retained prefix, never a claim that deferred content is empty.
         */
        audit?: PublicationAudit2 | null;
        transcript_cursor: RuntimeClientTranscriptCursor;
        read_cut: ObservationReadCut1;
        content: ContentBinding[];
        collections: CollectionFrontier[];
        type: 'assistant_publication_deferred_settled';
      }
    | {
        attempt_id: AttemptId;
        previous_locator: CanonicalContentLocator;
        occurrence: RuntimeClientToolOccurrence;
        metadata_ref?: ContentRef | null;
        type: 'tool_occurrence_rebound';
      }
    | {
        attempt_id: AttemptId;
        occurrence: RuntimeClientToolOccurrence;
        result: ToolExecutionResult;
        result_deferred: boolean;
        result_metadata_ref?: ContentRef | null;
        type: 'tool_result_committed';
      }
    | {
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        occurrence?: RuntimeClientToolOccurrence | null;
        metadata_ref?: ContentRef | null;
        /**
         * The logical tool-call identity.
         */
        tool_call_id?: ToolCallId | null;
        /**
         * The canonical tool identity.
         */
        tool_id?: ToolId | null;
        type: 'tool_execution_started';
      }
    | {
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        occurrence?: RuntimeClientToolOccurrence | null;
        metadata_ref?: ContentRef | null;
        /**
         * The logical tool-call identity.
         */
        tool_call_id?: ToolCallId | null;
        /**
         * The canonical tool identity.
         */
        tool_id?: ToolId | null;
        /**
         * The detached runtime execution instance for background work;
         * `None` for foreground executions (no fake id is invented).
         */
        execution_id?: ToolExecutionId | null;
        progress: ToolProgress1;
        type: 'tool_execution_progress';
      }
    | {
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        occurrence?: RuntimeClientToolOccurrence | null;
        metadata_ref?: ContentRef | null;
        /**
         * The logical tool-call identity.
         */
        tool_call_id?: ToolCallId | null;
        /**
         * The canonical tool identity.
         */
        tool_id?: ToolId | null;
        result: ToolExecutionResult3;
        result_source?: JournalFieldRef | null;
        result_metadata_ref?: ContentRef | null;
        type: 'tool_execution_settled';
      }
    | {
        attempt_id?: AttemptId | null;
        descriptor: RuntimeClientMessageDescriptor;
        head: RuntimeClientMessageDescriptor[];
        transcript_cursor?: RuntimeClientTranscriptCursor | null;
        evicted_message_id?: MessageId | null;
        type: 'message_descriptor_committed';
      }
    | {
        /**
         * Exact display-head eviction; canonical history remains authoritative and readable.
         */
        evicted_message_id?: MessageId | null;
        /**
         * The committing attempt, when one is active.
         */
        attempt_id?: AttemptId | null;
        /**
         * The canonical conversation message.
         *
         * The `role` discriminator is stable: `user`, `assistant`, `tool`.
         * No additional top-level role exists.
         */
        message:
          | (UserMessageBlock & {
              role: 'user';
            })
          | (AssistantMessageBlock & {
              role: 'assistant';
            })
          | (ToolMessageBlock & {
              role: 'tool';
            });
        /**
         * The durable transcript position, absent for hidden Context facts.
         */
        transcript_cursor?: RuntimeClientTranscriptCursor | null;
        type: 'message_committed';
      }
    | {
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        /**
         * The turn number of the request preparation.
         */
        turn: number;
        status: AgentStatusView1;
        /**
         * The composition this admission pushed out of the bounded window,
         * when the window was already full.
         *
         * `None` when nothing left the window — including for a replayed
         * observation of a composition the window already holds, which
         * admits nothing and therefore evicts nothing. A client removes
         * exactly this identity before applying the identity-keyed
         * admission, so replay stays idempotent and can never displace an
         * unrelated composition.
         */
        evicted_status_message_id?: MessageId | null;
        evicted_status_locator?: CanonicalContentLocator | null;
        type: 'agent_status_composed';
      }
    | {
        /**
         * Complete committed pending projection, in durable sequence order.
         */
        pending: InboundItemView[];
        type: 'pending_inbound_changed';
      }
    | {
        /**
         * A conversation-scoped inbound sequence number.
         *
         * The sequence identifies one item of the conversation's inbound ordering
         * domain. It is **not**
         * [`RuntimeEventEnvelope`](crate::events::types::RuntimeEventEnvelope)`::sequence`
         * and is never allocated from the Event Journal sequence; the durable
         * Pending Inbound Inbox owns allocation, and the first successful
         * acceptance of a conversation receives `1`.
         */
        sequence: string;
        message: UserMessageBlock2;
        /**
         * The durable transcript position allocated at acceptance, absent
         * for hidden Context-kind inbound.
         */
        transcript_cursor?: RuntimeClientTranscriptCursor | null;
        type: 'inbound_enqueued';
      }
    | {
        /**
         * A conversation-scoped inbound sequence number.
         *
         * The sequence identifies one item of the conversation's inbound ordering
         * domain. It is **not**
         * [`RuntimeEventEnvelope`](crate::events::types::RuntimeEventEnvelope)`::sequence`
         * and is never allocated from the Event Journal sequence; the durable
         * Pending Inbound Inbox owns allocation, and the first successful
         * acceptance of a conversation receives `1`.
         */
        watermark: string;
        /**
         * The number of drained items.
         */
        count: number;
        /**
         * The drained message identities in inbound sequence order.
         */
        message_ids: MessageId[];
        type: 'inbound_drained';
      }
    | {
        domain: string;
        cut: ObservationCollectionCut;
        type: 'collection_changed';
      }
    | {
        position: number;
        evicted_job_id?: ToolExecutionId | null;
        job: RuntimeClientJob1;
        type: 'job_updated';
      }
    | {
        position: number;
        evicted_agent_id?: AgentId | null;
        agent: RuntimeClientAgent1;
        type: 'agent_updated';
      }
    | {
        capabilities: CapabilityView1;
        type: 'capability_updated';
      }
    | {
        plugins?: EffectivePlugins | null;
        model?: SessionModelView | null;
        model_status: RuntimeClientModelStatus;
        approval_mode: ApprovalMode;
        capabilities: CapabilityView2;
        resources: RuntimeClientResourcesView1;
        type: 'resource_generation_updated';
      }
    | {
        /**
         * The redacted session model state after the update.
         */
        model?: SessionModelView | null;
        model_status: RuntimeClientModelStatus;
        type: 'session_model_changed';
      }
    | {
        type: 'runtime_shutdown';
      }
    | {
        /**
         * The operation that failed persistently.
         */
        operation: string;
        /**
         * The human-readable failure diagnostic.
         */
        diagnostic: string;
        type: 'runtime_durability_failed';
      };
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowSnapshot".
 */
export interface WorkflowSnapshot1 {
  revision: string;
  runs: WorkflowRunView[];
  omitted_runs: number;
}
/**
 * Normalized token accounting for one generation.
 *
 * Providers do not expose identical token metrics; this is the stable
 * common core. Provider SDK usage objects never appear here.
 * Missing provider metrics remain absent; a reported zero is `Some(0)`.
 * These are native u64 quantities, not byte offsets. Observation transports
 * use the existing exact decimal codec without narrowing provider evidence.
 */
export interface ModelUsage1 {
  /**
   * Input tokens consumed by the request.
   */
  input_tokens?: string | null;
  /**
   * Output tokens produced by the response.
   */
  output_tokens?: string | null;
  /**
   * Total tokens, where the provider reports or can derive them.
   */
  total_tokens?: string | null;
  /**
   * Optional normalized usage details.
   */
  details?: UsageDetails | null;
}
/**
 * Finite display identity of an actionable native request. A missing body is
 * an explicit deferred source, never an empty or reconstructed Questionnaire.
 */
export interface RuntimeClientInteractionHeader1 {
  /**
   * Exact native source offset, stable for this pending lifetime.
   */
  source_offset: number;
  interaction: InteractionRef;
  source: InteractionSource;
  request_id: InteractionId;
  conversation_id: ConversationId;
  execution: InteractionExecution;
  kind: RuntimeClientInteractionKind;
  approval?: RuntimeClientApprovalSummary | null;
  body?: InteractionRequest | null;
  body_complete: boolean;
  workflow_node?: WorkflowNodeInstance | null;
  workflow_node_deferred: boolean;
  workflow_node_path?: string[] | null;
}
/**
 * The root-facing address of a conversation-local interaction.
 *
 * `InteractionId` is allocated inside one conversation/attempt domain and is
 * intentionally not globally unique. The pair is the only identity that
 * crosses a Runtime Client or parent/child routing boundary.
 */
export interface InteractionRef1 {
  /**
   * The conversation-owned semantic interaction domain.
   */
  conversation_id: string;
  /**
   * The interaction identity allocated by that conversation's coordinator.
   */
  interaction_id: string;
}
/**
 * The root-facing address of a conversation-local interaction.
 *
 * `InteractionId` is allocated inside one conversation/attempt domain and is
 * intentionally not globally unique. The pair is the only identity that
 * crosses a Runtime Client or parent/child routing boundary.
 */
export interface InteractionRef2 {
  /**
   * The conversation-owned semantic interaction domain.
   */
  conversation_id: string;
  /**
   * The interaction identity allocated by that conversation's coordinator.
   */
  interaction_id: string;
}
/**
 * The bounded requested audit projection.
 */
export interface RuntimeClientTranscriptInteractionRequested {
  /**
   * Identifies one durable runtime event.
   */
  event_id: string;
  /**
   * Event timestamp.
   */
  timestamp: string;
  /**
   * Owning Attempt; absent for native Scheduled work.
   */
  attempt_id?: AttemptId | null;
  /**
   * Owning turn; absent for native Scheduled work.
   */
  turn_id?: TurnId | null;
  /**
   * Identifies one process-owned native human interaction.
   *
   * Interaction identities are derived from the conversation runtime's
   * non-reused [`AttemptId`] domain plus an ordinal owned by the
   * interaction coordinator.  They are deliberately not allocated from
   * a process-local counter alone: a response retained by a client from a
   * crashed process can therefore never name a different interaction in a
   * restarted runtime.
   */
  interaction_id: string;
  /**
   * Bounded durable subject.
   */
  subject:
    | {
        review: ReviewSpecification;
        type: 'review';
      }
    | {
        /**
         * Caller correlation for one native invocation. Capability source is
         * independently represented by [`ToolOrigin`].
         */
        invocation_id:
          | {
              call_id: ToolCallId;
              caller: 'agent';
            }
          | {
              node: WorkflowNodeInstance;
              caller: 'workflow';
            };
        /**
         * Identifies a tool definition in the capability set.
         */
        tool_id: string;
        /**
         * The model-facing tool name.
         */
        tool_name: string;
        /**
         * The digest of the canonical model-issued arguments.
         */
        arguments_digest: string;
        /**
         * The bounded policy explanation shown to the client.
         */
        reason: string;
        type: 'approval';
      }
    | {
        invocation_id: ToolInvocationId;
        requester: InteractionRequester;
        questionnaire: QuestionnaireSpecification;
        type: 'questionnaire';
      };
}
/**
 * The bounded settled audit projection.
 */
export interface RuntimeClientTranscriptInteractionSettled {
  /**
   * Identifies one durable runtime event.
   */
  event_id: string;
  /**
   * Event timestamp.
   */
  timestamp: string;
  /**
   * Owning Attempt; absent for native Scheduled work.
   */
  attempt_id?: AttemptId | null;
  /**
   * Owning turn; absent for native Scheduled work.
   */
  turn_id?: TurnId | null;
  /**
   * Identifies one process-owned native human interaction.
   *
   * Interaction identities are derived from the conversation runtime's
   * non-reused [`AttemptId`] domain plus an ordinal owned by the
   * interaction coordinator.  They are deliberately not allocated from
   * a process-local counter alone: a response retained by a client from a
   * crashed process can therefore never name a different interaction in a
   * restarted runtime.
   */
  interaction_id: string;
  /**
   * Bounded durable settlement.
   */
  settlement:
    | {
        response: ReviewResponse;
        type: 'reviewed';
      }
    | {
        kind: ToolDeadlineKind;
        type: 'deadline_expired';
      }
    | {
        type: 'approved';
      }
    | {
        /**
         * The bounded client-facing denial reason.
         */
        reason: string;
        type: 'denied';
      }
    | {
        submission: QuestionnaireSubmission1;
        type: 'questionnaire_submitted';
      }
    | {
        type: 'questionnaire_declined';
      }
    | {
        /**
         * The first-winner cancellation cause.
         */
        reason:
          | 'user_requested'
          | 'runtime_shutdown'
          | 'parent_cancelled'
          | 'subagent_execution_deadline_exceeded';
        type: 'cancelled';
      };
}
/**
 * Authoritative context state at this lifecycle boundary.
 */
export interface RuntimeClientContextView1 {
  /**
   * Independent current historical projection, including inherited activations.
   */
  projection_head?: RuntimeClientProjectionHeadView | null;
  /**
   * Latest manual operation admitted in this runtime incarnation. Never inferred
   * from a newer canonical checkpoint. Absence is insufficient evidence.
   */
  manual_compaction?: RuntimeClientManualCompaction | null;
  /**
   * Latest pre-commit diagnostic; cleared when the next compaction starts.
   */
  compaction_error?: string | null;
  /**
   * Latest native measured request or committed compacted-context estimate.
   */
  occupancy?: RuntimeClientContextOccupancy | null;
  /**
   * Whether the runtime currently owns a context-compaction operation.
   * This is live operation state, not inferred from token usage.
   */
  compaction_in_progress: boolean;
  /**
   * Runtime Client projection statistic: the number of committed
   * compaction completions folded into this read model. The compaction
   * generation remains the conversation-owned identity.
   */
  compaction_count: number;
  /**
   * The latest committed compaction metadata, when compaction occurred.
   */
  latest_compaction?: RuntimeClientCompactionView | null;
}
/**
 * Authoritative context state at this lifecycle boundary.
 */
export interface RuntimeClientContextView2 {
  /**
   * Independent current historical projection, including inherited activations.
   */
  projection_head?: RuntimeClientProjectionHeadView | null;
  /**
   * Latest manual operation admitted in this runtime incarnation. Never inferred
   * from a newer canonical checkpoint. Absence is insufficient evidence.
   */
  manual_compaction?: RuntimeClientManualCompaction | null;
  /**
   * Latest pre-commit diagnostic; cleared when the next compaction starts.
   */
  compaction_error?: string | null;
  /**
   * Latest native measured request or committed compacted-context estimate.
   */
  occupancy?: RuntimeClientContextOccupancy | null;
  /**
   * Whether the runtime currently owns a context-compaction operation.
   * This is live operation state, not inferred from token usage.
   */
  compaction_in_progress: boolean;
  /**
   * Runtime Client projection statistic: the number of committed
   * compaction completions folded into this read model. The compaction
   * generation remains the conversation-owned identity.
   */
  compaction_count: number;
  /**
   * The latest committed compaction metadata, when compaction occurred.
   */
  latest_compaction?: RuntimeClientCompactionView | null;
}
/**
 * The context diagnostics after this committed compaction.
 */
export interface RuntimeClientContextView3 {
  /**
   * Independent current historical projection, including inherited activations.
   */
  projection_head?: RuntimeClientProjectionHeadView | null;
  /**
   * Latest manual operation admitted in this runtime incarnation. Never inferred
   * from a newer canonical checkpoint. Absence is insufficient evidence.
   */
  manual_compaction?: RuntimeClientManualCompaction | null;
  /**
   * Latest pre-commit diagnostic; cleared when the next compaction starts.
   */
  compaction_error?: string | null;
  /**
   * Latest native measured request or committed compacted-context estimate.
   */
  occupancy?: RuntimeClientContextOccupancy | null;
  /**
   * Whether the runtime currently owns a context-compaction operation.
   * This is live operation state, not inferred from token usage.
   */
  compaction_in_progress: boolean;
  /**
   * Runtime Client projection statistic: the number of committed
   * compaction completions folded into this read model. The compaction
   * generation remains the conversation-owned identity.
   */
  compaction_count: number;
  /**
   * The latest committed compaction metadata, when compaction occurred.
   */
  latest_compaction?: RuntimeClientCompactionView | null;
}
/**
 * The tool-call identity and metadata known at start.
 */
export interface RuntimeClientToolCall {
  id?: ToolCallId | null;
  tool_id?: ToolId | null;
  name: string;
  arguments?: unknown;
  metadata_ref?: ContentRef | null;
}
/**
 * The fully assembled tool call.
 */
export interface RuntimeClientToolCall1 {
  id?: ToolCallId | null;
  tool_id?: ToolId | null;
  name: string;
  arguments?: unknown;
  metadata_ref?: ContentRef | null;
}
/**
 * The bounded immutable audit of the settled stream.
 */
export interface PublicationAudit1 {
  /**
   * Identifies one user-facing publication stream (Issue #108, FND-03).
   *
   * A publication stream is the durable release plane of exactly one
   * provider request: it is the identity under which committed-for-release
   * frames are staged, under which the publication terminal marker
   * commits, and under which the stream settles exactly once as canonical,
   * unaccepted, or incomplete. It is deliberately distinct from
   * [`RequestId`] (the provider execution fact) and from [`MessageId`]
   * (the canonical conversation fact), because publication durability is
   * a separate plane from both.
   */
  stream_id: string;
  /**
   * Identifies one attempt to execute an agent manifest.
   */
  attempt_id: string;
  /**
   * Identifies one turn within an attempt.
   */
  turn_id: string;
  /**
   * Identifies one actual provider-neutral model request.
   *
   * A request identity is distinct from an attempt, turn, retry ordinal,
   * and Event Journal sequence. Primary requests derive the `request:`
   * namespace from immutable [`RequestIdentity`](crate::model::snapshot::RequestIdentity).
   * Auxiliary requests allocate the separate `req_` `UUIDv7` namespace with
   * [`RequestId::generate_auxiliary`]. Each physical retry is a distinct
   * request. Identity alone grants no execution permission.
   */
  request_id: string;
  /**
   * Identifies a committed canonical message block.
   */
  message_id: string;
  /**
   * Which of the two audit settlements this is.
   */
  kind: 'unaccepted' | 'incomplete';
  /**
   * The consolidated committed-for-release content, in block order.
   */
  content: PublicationAuditBlock[];
  /**
   * When the audit terminalized.
   */
  settled_at: string;
}
/**
 * One bounded immutable audit of a settled non-canonical publication.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "PublicationAudit".
 */
export interface PublicationAudit2 {
  /**
   * Identifies one user-facing publication stream (Issue #108, FND-03).
   *
   * A publication stream is the durable release plane of exactly one
   * provider request: it is the identity under which committed-for-release
   * frames are staged, under which the publication terminal marker
   * commits, and under which the stream settles exactly once as canonical,
   * unaccepted, or incomplete. It is deliberately distinct from
   * [`RequestId`] (the provider execution fact) and from [`MessageId`]
   * (the canonical conversation fact), because publication durability is
   * a separate plane from both.
   */
  stream_id: string;
  /**
   * Identifies one attempt to execute an agent manifest.
   */
  attempt_id: string;
  /**
   * Identifies one turn within an attempt.
   */
  turn_id: string;
  /**
   * Identifies one actual provider-neutral model request.
   *
   * A request identity is distinct from an attempt, turn, retry ordinal,
   * and Event Journal sequence. Primary requests derive the `request:`
   * namespace from immutable [`RequestIdentity`](crate::model::snapshot::RequestIdentity).
   * Auxiliary requests allocate the separate `req_` `UUIDv7` namespace with
   * [`RequestId::generate_auxiliary`]. Each physical retry is a distinct
   * request. Identity alone grants no execution permission.
   */
  request_id: string;
  /**
   * Identifies a committed canonical message block.
   */
  message_id: string;
  /**
   * Which of the two audit settlements this is.
   */
  kind: 'unaccepted' | 'incomplete';
  /**
   * The consolidated committed-for-release content, in block order.
   */
  content: PublicationAuditBlock[];
  /**
   * When the audit terminalized.
   */
  settled_at: string;
}
/**
 * Exact immutable source evidence captured by the native audit transaction.
 */
export interface ObservationReadCut1 {
  read: ConversationReadCut;
  ledger_through: string;
  transcript_total: string;
  surface_revision: SurfaceRevision;
  inherited_through: TranscriptCursor;
}
/**
 * A bounded structured progress notification of one tool execution.
 *
 * Progress is an execution fact, never canonical message history. All
 * fields are optional; an empty `ToolProgress` is a bare tick. The progress
 * message text is bounded by [`MAX_PROGRESS_MESSAGE_BYTES`].
 *
 * [`MAX_PROGRESS_MESSAGE_BYTES`]: crate::tools::limits::MAX_PROGRESS_MESSAGE_BYTES
 */
export interface ToolProgress1 {
  /**
   * A short human-readable progress message, when there is one.
   */
  message?: string | null;
  /**
   * Completed units, when a total is known.
   */
  completed?: number | null;
  /**
   * Total units, when known.
   */
  total?: number | null;
}
/**
 * The normalized outcome of one tool execution.
 *
 * `ToolMessageBlock` composes this type instead of duplicating its fields,
 * keeping one source of truth for tool results.
 */
export interface ToolExecutionResult3 {
  /**
   * Immutable native Workflow identity on the existing outer result.
   * Historical identity only: no execution state or continuation authority.
   * Its retention is exactly that of this result, never a separate registry.
   */
  workflow?: WorkflowToolIdentity | null;
  /**
   * Typed execution status, including unknown external outcomes.
   */
  status:
    | {
        type: 'success';
      }
    | {
        /**
         * Human-readable error message.
         */
        error: string;
        type: 'failed';
      }
    | {
        /**
         * The policy or human-readable approval reason.
         */
        reason: string;
        type: 'denied';
      }
    | {
        /**
         * Why the execution was cancelled.
         */
        reason:
          | 'user_requested'
          | 'runtime_shutdown'
          | 'parent_cancelled'
          | 'subagent_execution_deadline_exceeded';
        /**
         * Whether cancellation won before executor start or while execution
         * was already in flight.
         */
        phase: 'before_start' | 'during_execution';
        type: 'cancelled';
      }
    | {
        type: 'timed_out';
      }
    | {
        /**
         * A producer-owned diagnostic describing why certainty is
         * unavailable. It is rendered into the bounded model-facing
         * projection and is never parsed to decide semantics; the typed
         * variant itself is the certainty claim.
         */
        detail: string;
        type: 'outcome_unknown';
      };
  /**
   * Tool-owned result content.
   *
   * This content is TOOL-OWNED: [`ToolResultContent::Json`] is arbitrary
   * tool-owned structured data, and the runtime never infers semantics
   * from its property names. rustX reserves no ordinary JSON field names;
   * runtime-owned facts live in the typed fields of this struct. A
   * provider-independent, bounded model-facing representation is produced
   * by [`Self::model_facing_projection`]; producers do not append runtime
   * status or managed-output continuation text here.
   */
  content?: ToolResultContent[];
  /**
   * Execution duration in integer milliseconds (stable for persistence).
   */
  duration_ms: string;
  /**
   * Process exit code where the tool executed a process.
   */
  exit_code?: number | null;
  /**
   * Immutable managed Artifact references produced by the execution.
   */
  artifacts?: FileReference[];
  /**
   * Runtime-owned explicit declarations of mutable native Session files.
   * Only successful canonical Tool results authorize delivery cards.
   */
  deliveries?: SessionFileReference[];
  /**
   * Truncation metadata where output was truncated.
   */
  truncation?: TruncationState | null;
  /**
   * Runtime-owned managed textual-output continuation metadata: where
   * the complete — or honestly partial — textual output of this result
   * lives in the conversation's managed tool-output store (Issue #86).
   * Absent for results whose output fits the model-facing content.
   *
   * This is the one typed source of truth for complete-vs-partial
   * managed output; producers never encode these facts as magic
   * properties of tool-owned JSON, and generic runtime publication code
   * consumes only this typed field, never arbitrary JSON keys.
   */
  managed_output?: ManagedOutputContinuation | null;
}
/**
 * The structured Agent Status view of one composition.
 *
 * Derived from the exact composed status the model path consumed: the
 * structured sections and the canonical rendered representation originate
 * from the same composition, so a client never parses the rendered text
 * to recover structure and never triggers a second composition.
 *
 * The view also carries the runtime facts that place the composition in
 * conversation order: the eligible
 * [`opportunities`](Self::opportunities), each with the durable identity it
 * was established against. Placement is a runtime fact because only the
 * runtime knows it, and it is frozen where it is determined rather than
 * reconstructed downstream; how a client draws a status at that place is
 * presentation and stays entirely outside this type.
 */
export interface AgentStatusView1 {
  /**
   * Identifies one attempt to execute an agent manifest.
   */
  attempt_id: string;
  /**
   * The turn number of the request preparation.
   */
  turn: number;
  /**
   * The canonical Agent Status User message described by this view. It is
   * the stable identity of the composition: observing the same
   * `status_message_id` twice describes one composition, never two.
   */
  status_message_id?: MessageId | null;
  source_locator?: CanonicalContentLocator | null;
  request_id?: RequestId | null;
  rendered_ref?: ContentRef | null;
  opportunities: AgentStatusOpportunityView;
  /**
   * The ordered structured sections.
   */
  sections: RuntimeClientStatusSection[];
  /**
   * The canonical rendered representation, derived from the same
   * composition as the sections.
   *
   * It exists for diagnostics and for proving that a client and the model
   * saw one composition. It is **not** a presentation source: a client
   * renders [`sections`](Self::sections) and never parses this text back
   * into structure.
   */
  rendered: string;
}
/**
 * Inbound information supplied to the current agent.
 *
 * A `UserMessageBlock` does not necessarily mean a human spoke: it is the
 * canonical home for anything inbound, including messages from other agents
 * (with [`UserSource::Agent`] provenance). Compaction summaries instead belong
 * to immutable model-context checkpoints. An inbound block
 * must never become `AssistantMessageBlock` or `ToolMessageBlock`, which are
 * reserved for output and actions of the current agent.
 */
export interface UserMessageBlock2 {
  /**
   * Identifies a committed canonical message block.
   */
  id: string;
  /**
   * The inbound content.
   */
  content: UserContentBlock[];
  /**
   * Provenance: who supplied the inbound information.
   */
  source:
    | 'human'
    | {
        agent: {
          /**
           * Identifies an Agent independently of its finite activations.
           * For a continuable child, this is the durable Agent-domain identity of
           * its child Conversation and remains unchanged across resume.
           */
          agent_id: string;
        };
      }
    | 'fleet'
    | 'external_system'
    | 'runtime'
    | {
        extension: {
          /**
           * The stable logical key of one certified extension.
           *
           * Package/content attestation is intentionally not part of this type.  A
           * package may be upgraded while preserving its logical ordering identity;
           * the assembly generation records the attestation separately.
           */
          contributor: string;
        };
      };
  /**
   * Typed kind of inbound information.
   */
  kind?:
    | {
        goal_continuation: GoalRef;
      }
    | 'message'
    | {
        context: ContextKind;
      };
  /**
   * The persisted UTC instant associated with the inbound message, when
   * the producer supplied one.
   *
   * An ordinary asynchronously delivered inbound message
   * ([`InboundKind::Message`]) carries the persisted instant of its
   * delivery; the producer supplies the original timestamp explicitly and
   * no wall-clock time is fabricated. Messages without a producer timestamp
   * remain representable: the field
   * defaults to `None` on deserialization and is omitted from the
   * canonical encoding while absent.
   */
  timestamp?: string | null;
}
/**
 * The external background execution read model.
 *
 * Projected from the authoritative [`ConversationBackgroundRegistry`]
 * ([`crate::tools::background::ConversationBackgroundRegistry`]); the
 * container shape belongs to the Runtime Client protocol while the
 * lifecycle, progress, and result leaf types are stable runtime-owned
 * value contracts. No internal task handles or process ids ever appear.
 */
export interface RuntimeClientJob1 {
  deferred_fields?: string[][];
  /**
   * Native Bash presentation, derived from the canonical invocation.
   */
  bash?: BashPresentation | null;
  /**
   * The detached runtime execution identity.
   */
  job_id: string;
  /**
   * The canonical tool identity.
   */
  tool_id?: ToolId | null;
  /**
   * The model-facing tool name.
   */
  tool_name: string;
  /**
   * The public lifecycle of one background execution.
   *
   * Terminal states are absorbing. The allowed public transitions are:
   *
   * ```text
   * Starting  → Running
   * Starting  → Cancelling
   * Starting  → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
   * Running   → Cancelling
   * Running   → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
   * Starting / Running / Cancelling → PublishingTerminal
   * Cancelling → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
   * PublishingTerminal → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
   * ```
   *
   * The terminal vocabulary claims only facts the executor proved: `Failed`
   * is a *known* failure, `Denied` is policy/approval refusal (not `Failed`),
   * `TimedOut` is a proven deadline settlement, and
   * `OutcomeUnknown` means the executor could not prove the external outcome
   * at all. A terminal lifecycle never claims a stronger external outcome
   * than the canonical [`ToolExecutionStatus`] of the stored result proves —
   * in particular an unknown outcome is never labelled `Failed`, and a
   * cancellation request that could not be confirmed physically never
   * becomes `Cancelled`.
   *
   * [`BackgroundLifecycle::PublishingTerminal`] is the honest non-terminal
   * state in which the executor has returned its terminal candidate and the
   * registry now owns durable terminal publication; it is never `Running`:
   * the executor has returned and the runner now owns the durable settlement
   * continuation, retaining the settlement candidate until publication
   * reaches a terminal outcome. An internal unpublished prepared
   * state implements dispatch atomicity but never leaks as an accepted
   * execution.
   */
  state:
    | 'starting'
    | 'running'
    | 'cancelling'
    | 'publishing_terminal'
    | 'succeeded'
    | 'failed'
    | 'denied'
    | 'cancelled'
    | 'timed_out'
    | 'outcome_unknown';
  /**
   * The latest bounded progress, when any was reported.
   */
  progress?: ToolProgress | null;
  /**
   * The bounded terminal result, when terminal.
   */
  result?: ToolExecutionResult | null;
}
/**
 * One durable child Agent, keyed by `agent_id` across every activation.
 *
 * The registry owns lifecycle arbitration. Journal ownership facts rebuild
 * identity and activation correlation; canonical child history owns content.
 * `activation_id` identifies the latest activation, while `current_activation`
 * is absent when inactive or reserving the next activation. Neither replaces
 * the stable Agent identity. Activity is bounded observation, never authority.
 */
export interface RuntimeClientAgent1 {
  deferred_fields?: string[][];
  title: string;
  parent_agent_id: AgentId;
  /**
   * Identifies one finite activation owned by an Agent, or one finite
   * Workflow child execution.
   *
   * Repeated native Agent activations have distinct `SubagentId`s while
   * retaining the same durable `AgentId`. This is not an OS pid: process
   * identity or PID reuse can never establish durable ownership or proof
   * of physical settlement.
   */
  activation_id: string;
  /**
   * Identifies an Agent independently of its finite activations.
   * For a continuable child, this is the durable Agent-domain identity of
   * its child Conversation and remains unchanged across resume.
   */
  agent_id: string;
  /**
   * The child's own durable conversation identity.
   */
  child_conversation_id: string;
  /**
   * The canonical named-agent identity frozen at start (Issue #144).
   */
  agent: string;
  /**
   * The deterministic definition digest frozen at start (Issue #144).
   *
   * A client observing an already-running child sees the definition it
   * actually started with, so a later configuration adoption that redefines the
   * same agent name can never be mistaken for a change to that child.
   */
  definition_digest: string;
  /**
   * The deterministic **effective execution profile** digest frozen at
   * child start (Issue #258).
   *
   * It distinguishes two children of one named agent that an authorized
   * invocation override specialized differently — the same
   * `definition_digest`, different effective tools, Skills, or extensions.
   * It is committed with durable ownership, so it survives a restart
   * unchanged and a recovery-projected child reports the same value a live
   * one does.
   *
   * It is a bounded correlation identity and nothing more: no effective
   * selection, prompt, Skill body, or materialization detail is projected
   * with it, and no authority decision reads it.
   */
  profile_digest: string;
  /**
   * The authoritative lifecycle state.
   */
  state: ('admitting' | 'active' | 'stopping' | 'inactive') | 'unavailable';
  current_activation?: SubagentId | null;
  activation_state: SubagentState;
  /**
   * The bounded terminal failure/cancellation diagnostic, once known.
   *
   * A successful child's answer content never appears here (Issue
   * #178): the durable terminal inbound publication is the one result
   * channel.
   */
  detail?: string | null;
  observation: SubagentObservation;
  /**
   * The redacted execution profile frozen at child start (Issue #178);
   * `None` for recovery-projected records. Named `execution_profile`
   * on the wire: the bare `profile` key was the obsolete pre-#144
   * profile-name field and stays retired.
   */
  execution_profile?: SubagentExecutionProfile | null;
  /**
   * When the ownership committed; clients derive elapsed time from it.
   */
  started_at: string;
  /**
   * The Session execution binding this Agent froze at creation.
   */
  workspace: string;
}
/**
 * The deterministic active capability projection.
 */
export interface CapabilityView1 {
  /**
   * Configured Tool intent before effective model gating.
   */
  configured_tools: RuntimeClientTool[];
  /**
   * The active monotonic capability revision.
   */
  revision: string;
  /**
   * The deterministic active Tool catalog in registry order. Model
   * requests and execution use exactly this set.
   */
  tools?: RuntimeClientTool[];
  /**
   * The complete available Tool catalog, including inactive Tools. The
   * active set above is an explicit subset; availability never implies
   * model activation.
   */
  available_tools?: RuntimeClientTool[];
  /**
   * The deterministic model-visible Skill catalog ordered by Skill name.
   * Skills hidden by `disable-model-invocation` remain runtime-owned but
   * are omitted here. Every entry includes the canonical absolute host
   * path of its `SKILL.md`.
   */
  skills?: RuntimeClientSkill[];
  /**
   * The typed availability of every evaluated optional capability
   * source, in deterministic source-identity order (Issue #81).
   */
  sources?: CapabilitySourceView[];
}
/**
 * The active capability projection this generation was composed
 * against.
 */
export interface CapabilityView2 {
  /**
   * Configured Tool intent before effective model gating.
   */
  configured_tools: RuntimeClientTool[];
  /**
   * The active monotonic capability revision.
   */
  revision: string;
  /**
   * The deterministic active Tool catalog in registry order. Model
   * requests and execution use exactly this set.
   */
  tools?: RuntimeClientTool[];
  /**
   * The complete available Tool catalog, including inactive Tools. The
   * active set above is an explicit subset; availability never implies
   * model activation.
   */
  available_tools?: RuntimeClientTool[];
  /**
   * The deterministic model-visible Skill catalog ordered by Skill name.
   * Skills hidden by `disable-model-invocation` remain runtime-owned but
   * are omitted here. Every entry includes the canonical absolute host
   * path of its `SKILL.md`.
   */
  skills?: RuntimeClientSkill[];
  /**
   * The typed availability of every evaluated optional capability
   * source, in deterministic source-identity order (Issue #81).
   */
  sources?: CapabilitySourceView[];
}
/**
 * The active runtime resource projection after the commit.
 */
export interface RuntimeClientResourcesView1 {
  /**
   * Facts copied from the same immutable generation as this revision.
   * Deferred inspection facts are loaded through finite native field pages.
   */
  inspection?: CapabilityInspection | null;
  /**
   * Identifies one immutable process-local runtime resource generation.
   *
   * This is deliberately separate from `ContextGeneration` (one context
   * assembly provenance set) and `CapabilityRevision` (one executable
   * capability set). A resource-only change may advance this revision while
   * retaining an identical capability revision.
   */
  revision?: string;
  /**
   * The runtime-loaded project instruction files, root-most to
   * workspace, in the exact order the runtime concatenated them.
   */
  context_files?: RuntimeClientContextFile[];
  /**
   * Whether an immutable agent profile/persona is frozen into this
   * generation. The persona text itself is runtime-owned request input.
   */
  agent_profile?: boolean;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ProjectionMetadata".
 */
export interface ProjectionMetadata {
  read_cut: CheckpointReadCut;
  collections: CollectionFrontier[];
  content: ContentBinding[];
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentStatistics".
 */
export interface AgentStatistics {
  statistics: ConversationStatistics;
  occupancy?: RuntimeClientContextOccupancy | null;
  duration: AgentDuration;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentDuration".
 */
export interface AgentDuration {
  /**
   * Sum of this child's closed working intervals, excluding inactive gaps.
   */
  settled_ms: string;
  /**
   * Last interval, if its terminal event has not been observed.
   */
  active?: AgentActiveInterval | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentActiveInterval".
 */
export interface AgentActiveInterval {
  started_at: string;
  /**
   * Last durable evidence. Non-running children freeze here, including recovery.
   */
  observed_at: string;
  /**
   * Authoritative live lifecycle, never inferred from a missing terminal.
   */
  running: boolean;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ServerCapabilities".
 */
export interface ServerCapabilities {
  upload_policy: UploadPolicy;
  multi_session: boolean;
  single_writable_controller: boolean;
  headless_interactions: boolean;
  /**
   * Whether this connection holds transport-granted delivery access.
   * Reporting it grants nothing; the native seam checks the capability.
   */
  delivery_access: boolean;
  experimental_methods: string[];
}
/**
 * Transport flow control, independent of image and control-frame limits.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "UploadPolicy".
 */
export interface UploadPolicy {
  max_concurrent_transfers: number;
  max_chunk_bytes: number;
}
/**
 * Bounded authoritative metadata for one Session.
 *
 * The graph is deliberately not embedded here. Callers that need the graph
 * use the bounded tree page seam below, so `/session`, switch results, and
 * restart metadata never materialize every historical node.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionSnapshot".
 */
export interface SessionSnapshot {
  /**
   * Session identity.
   */
  id: string;
  /**
   * The user-defined display name, when this Session has one.
   *
   * A Session is born unnamed. The name is display metadata a user
   * chooses, never an identity: nothing resolves a Session by it, and an
   * unnamed Session is a complete, ordinary Session.
   */
  name?: string | null;
  /**
   * Automatic label, with local or inherited generation provenance.
   */
  generated_title?: SessionGeneratedTitle | null;
  /**
   * Creation instant.
   */
  created_at: string;
  /**
   * Last metadata/active-node publication instant.
   */
  updated_at: string;
  /**
   * The active node selected in this Session.
   */
  active_node: string;
  /**
   * The conversation owned by the active node.
   */
  active_conversation_id: string;
  /**
   * Number of persisted nodes, useful metadata for a bounded tree view.
   */
  node_count: number;
}
/**
 * Saved automatic display metadata, independent of a user's chosen name.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionGeneratedTitle".
 */
export interface SessionGeneratedTitle {
  text: string;
  provenance: SessionTitleProvenance;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EditorPage".
 */
export interface EditorPage {
  locator: EditorLocator;
  block_index: number;
  chunk?: ContentChunk | null;
  upload?: UploadReceipt | null;
  next_block?: number | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ContentChunk".
 */
export interface ContentChunk {
  content_ref: ContentRef;
  offset: number;
  data_base64: string;
  end_offset: number;
  readable_end: number;
  next_offset?: number | null;
  complete: boolean;
}
/**
 * Native display metadata, shared by exact identity reads and catalog rows.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionSummary".
 */
export interface SessionSummary {
  /**
   * Native execution ownership generation, encoded as an exact decimal integer.
   * Changes only when active-node ownership is replaced; never a display timestamp.
   */
  ownership_generation: string;
  /**
   * Canonical durable Session cwd, projected without loading a runtime.
   */
  cwd: string;
  /**
   * Session identity.
   */
  id: string;
  /**
   * The user-defined display name, when this Session has one.
   */
  name?: string | null;
  /**
   * Automatic label; manual name takes precedence when both are present.
   */
  generated_title?: SessionGeneratedTitle | null;
  /**
   * The first ordinary user message of this Session's root lineage,
   * bounded to one line. It labels a row without a saved name/title, and
   * it is the client-facing projection of the persisted
   * `display_preview`: the catalog stores the derived line so listing,
   * pagination, and search never open a conversation store. `None` is
   * legitimate — no ordinary user message exists yet, or the first one
   * has no renderable text — and yields the client-side identity
   * fallback.
   */
  preview?: string | null;
  /**
   * Latest committed human-message time, or creation time for a new Session.
   */
  updated_at: string;
  /**
   * Active node in the session.
   */
  active_node: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionNodeObservation".
 */
export interface SessionNodeObservation {
  ordinal: string;
  id: SessionNodeId;
  parent?: SessionNodeId | null;
  conversation_id: ConversationId;
  origin: SessionNodeOriginObservation;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionNodeValueChunk".
 */
export interface SessionNodeValueChunk {
  reference: SessionNodeValueRef;
  offset: number;
  bytes_base64: string;
  readable_end: number;
  next_offset?: number | null;
}
/**
 * One historical native Surface boundary, with a finite immutable canonical locator.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionUserMessageBoundaryObservation".
 */
export interface SessionUserMessageBoundaryObservation {
  surface_revision: SurfaceRevision;
  locator: CanonicalContentLocator;
  message?: UserMessageBlock | null;
  block_count: string;
  content: ContentBinding[];
  collections: CollectionFrontier[];
}
/**
 * Confirmation metadata; counts include the complete native ownership graph.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientSessionDeletePreview".
 */
export interface RuntimeClientSessionDeletePreview {
  session_id: SessionId;
  /**
   * Display name, truncated to at most 256 Unicode scalar values.
   */
  name?: string | null;
  target_revision: string;
  owned_node_count: number;
  owned_conversation_count: number;
  owned_child_count: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CheckpointValueChunk".
 */
export interface CheckpointValueChunk {
  representation: CheckpointValueRepresentation;
  cut: CheckpointReadCut;
  domain: string;
  item_offset: number;
  path: string[];
  offset: number;
  bytes_base64: string;
  next_offset?: number | null;
  readable_end: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectiveValueChunk".
 */
export interface EffectiveValueChunk {
  reference: EffectiveFieldRef;
  offset: number;
  end_offset: number;
  data_base64: string;
  next_offset?: number | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectivePage".
 */
export interface EffectivePage {
  revision: string;
  field: EffectiveFieldName;
  kind: string;
  total: number;
  items: EffectivePageItem[];
  next_offset?: number | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectivePageItem".
 */
export interface EffectivePageItem {
  index: number;
  value?: unknown;
  reference?: EffectiveFieldRef | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "BoundedEffectiveConfiguration".
 */
export interface BoundedEffectiveConfiguration {
  revision: string;
  inline?: EffectiveConfiguration | null;
  fields: EffectiveField[];
  collections: EffectiveCollection[];
}
/**
 * Redacted, immutable facts read at the runtime configuration publication lock.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectiveConfiguration".
 */
export interface EffectiveConfiguration {
  process_bindings?: AppServerPolicy | null;
  application?: ConfigurationApplication | null;
  adopted_binding: string;
  /**
   * The source manifest the adopted context generation was resolved from.
   * Independently published units — execution policy, shared capacity and
   * the Model Catalog — may be newer; `document` and `provenance` carry
   * their values, and `application` says which revision each applied.
   */
  source_revisions: {
    [k: string]: string;
  };
  generation: RuntimeResourceRevision;
  document: RuntimeLayer;
  root_agent: AgentProfileDocument;
  context: ContextPolicyDocument;
  model_timeout: ModelTimeoutPolicyDocument;
  tool_deadline: ToolDeadlinePolicyDocument;
  child_capacity: SubagentsDocument;
  approval_mode: ApprovalMode;
  provenance: {
    [k: string]: Origin;
  };
  resources: CapabilityInspection;
  available_tools: ToolDefinition[];
  session_model?: SessionModelConfig | null;
  effective_model: SessionModelView;
  admitted_attempt?: AdmittedConfiguration | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConfigurationApplication".
 */
export interface ConfigurationApplication {
  /**
   * The application-scope key. A Session application carries the Session
   * identity here; a source application carries `SourceTarget::
   * application_scope`. It is an application key, never a source owner.
   */
  scope: string;
  /**
   * The authored source owners this application composes, lowest authority
   * first. The User document always participates; a Workspace-rooted capture
   * also names the exact canonical configuration directory it was taken
   * from. This is the only fact that answers which authoring surface owns a
   * configuration failure; it is never derived from `scope`.
   */
  sources: SourceTarget[];
  version: string;
  desired: ApplicationIdentity;
  units: {
    capabilities?: UnitApplication;
    execution_policy?: UnitApplication;
    instructions?: UnitApplication;
    process_bindings?: UnitApplication;
    provider?: UnitApplication;
    shared_capacity?: UnitApplication;
  };
  candidate?: AvailableConfiguration | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AvailableConfiguration".
 */
export interface AvailableConfiguration {
  identity: ApplicationIdentity;
  expected_binding: string;
  impact: CacheImpact;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeLayer".
 */
export interface RuntimeLayer {
  providers?: {
    [k: string]: ProviderView;
  } | null;
  models?: {
    [k: string]: Model;
  } | null;
  schema_version?: number | null;
  app_server?: AppServerPolicy | null;
  agent_id?: AgentId | null;
  approval_mode?: ApprovalMode | null;
  agent?: AgentProfileLayer | null;
  context?: ContextLayer | null;
  model_timeout_policy?: TimeoutLayer | null;
  tool_deadline_policy?: ToolDeadlineLayer | null;
  mcp_tool_policies?: {
    [k: string]: InvocationPolicyDocument;
  } | null;
  native_tools?: NativeToolsLayer | null;
  /**
   * Tool environment entries. Native authoring holds the literal values
   * ([`AuthoredEnvironment`]); every projection that leaves native authority
   * holds the identities alone ([`EnvironmentIdentities`]), because a
   * literal Tool environment value is a secret on the same terms as a
   * Provider credential.
   */
  environment?: string[] | null;
  subagents?: SubagentsLayer | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ProviderView".
 */
export interface ProviderView {
  base_url: string;
  credential: CredentialSourceView;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentProfileLayer".
 */
export interface AgentProfileLayer {
  description?: string | null;
  instructions?: string | null;
  model?: ModelLayer | null;
  tools?: ToolsLayer | null;
  skills?: AgentSkillSelection | null;
  plugins?: PluginsLayer | null;
  agents?: SubagentName[] | null;
  workflows?: WorkflowId[] | null;
  agents_md?: AgentProjectInstructionsDocument | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolsLayer".
 */
export interface ToolsLayer {
  builtin?: string[] | null;
  sources?: {
    [k: string]: SourceToolSelection;
  } | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "PluginsLayer".
 */
export interface PluginsLayer {
  agent_status?: AgentStatusExtensionDocument1 | null;
  todo?: TodoExtensionDocument1 | null;
  goal?: GoalExtensionDocument | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "NativeToolsLayer".
 */
export interface NativeToolsLayer {
  read?: NativePolicyOverrideDocument | null;
  write?: NativePolicyOverrideDocument | null;
  edit?: NativePolicyOverrideDocument | null;
  glob?: NativePolicyOverrideDocument | null;
  grep?: NativePolicyOverrideDocument | null;
  bash?: NativePolicyOverrideDocument | null;
}
/**
 * The static current-runtime context policy document.
 *
 * There is deliberately no context window here: the window belongs to the
 * selected model and is derived per attempt from that attempt's immutable
 * model snapshot.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ContextPolicyDocument".
 */
export interface ContextPolicyDocument {
  /**
   * Tokens permanently reserved out of whichever model window is in
   * force.
   */
  reserveTokens: string;
  /**
   * Tokens of recent conversation history kept uncompressed.
   */
  keepRecentTokens: string;
  /**
   * The summary/output safety cap applied to the summary invocation
   * through the runtime-owned protected max-output field.
   */
  summaryOutputCap?: number | null;
}
/**
 * The resolved native model request timeout policy.
 *
 * Milliseconds keep the configuration human-readable while the runtime
 * receives a typed [`ModelTimeoutPolicy`] containing only finite
 * [`Duration`] values.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelTimeoutPolicyDocument".
 */
export interface ModelTimeoutPolicyDocument {
  /**
   * Maximum time to observe the first generation progress.
   */
  responseStartTimeoutMs?: string;
  /**
   * Maximum time between generation/liveness events after generation has
   * begun.
   */
  streamIdleTimeoutMs?: string;
}
/**
 * The current-runtime document of the tool execution-liveness deadline
 * policy (Issue #204).
 *
 * Milliseconds keep the configuration human-readable while the runtime
 * receives a typed
 * [`ToolExecutionDeadlinePolicy`](crate::tools::deadline::ToolExecutionDeadlinePolicy)
 * containing only finite [`Duration`] values.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolDeadlinePolicyDocument".
 */
export interface ToolDeadlinePolicyDocument {
  /**
   * The total maximum execution lifetime of one admitted foreground Tool
   * call, measured from its executor-start frontier. Progress never
   * extends it.
   */
  hardDeadlineMs?: string;
  /**
   * The optional idle-liveness window: the maximum time one started
   * execution may go without meaningful executor progress evidence.
   * Omitted means no idle watchdog. The window applies only to
   * executions whose executor declares meaningful progress capability;
   * all other executions run under the hard deadline only.
   */
  idleLivenessMs?: string | null;
}
/**
 * The resolved native representation of the named-subagent plane.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SubagentsDocument".
 */
export interface SubagentsDocument {
  /**
   * Runtime-global child capacity resolved from User < Workspace as one object.
   * Safe-boundary configuration publication updates the registry policy only
   * when no admitted child owns it; existing child specs remain frozen.
   */
  maxConcurrent?: number;
}
/**
 * The canonical runtime/tool contract of one registered tool.
 *
 * The definition is owned by the tool plane's registry, which pairs it with
 * an executor. The three policy axes are independent:
 *
 * - [`ToolExecutionPolicy`] decides who owns the execution: foreground work
 *   is attempt-owned and settles before the attempt continues, background
 *   work is conversation-owned and detached after accepted dispatch.
 * - [`ToolConcurrencyPolicy`] decides how calls of one batch are scheduled
 *   relative to each other.
 * - [`ToolApprovalPolicy`] decides whether an eligible invocation needs a
 *   native human approval before the executor starts.
 *
 * The `input_schema` is the original canonical JSON Schema document owned by
 * the tool. The runtime validates it at registration and never mutates it:
 * model-selectable invocation metadata is added only to the compiled
 * model-facing definition.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolDefinition".
 */
export interface ToolDefinition {
  /**
   * Identifies a tool definition in the capability set.
   */
  id: string;
  /**
   * Stable model-facing tool name used when emitting tool calls.
   */
  name: string;
  /**
   * Human-readable description shown to the model.
   */
  description: string;
  /**
   * The original canonical JSON Schema document describing the accepted
   * tool-call arguments. Tool-owned and never mutated by the runtime.
   */
  input_schema: {
    [k: string]: unknown;
  };
  /**
   * Who owns an invocation of this tool: the attempt (foreground) or the
   * conversation (background). Required: a missing policy is never
   * silently interpreted.
   */
  execution_policy: 'foreground_only' | 'background_only' | 'model_selectable';
  /**
   * How calls of this tool within one batch are scheduled relative to
   * each other.
   */
  concurrency_policy: 'sequential' | 'parallel';
  /**
   * Whether execution requires a native approval interaction.
   */
  approval_policy?: 'never' | 'always';
  /**
   * Replay policy; `Never` is the safe default.
   */
  replay_policy?: 'never' | 'idempotent';
  /**
   * Where a tool comes from.
   */
  origin:
    | 'builtin'
    | {
        mcp: {
          server_id: McpServerId;
        };
      }
    | {
        managed_python: {
          package: string;
        };
      };
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AdmittedConfiguration".
 */
export interface AdmittedConfiguration {
  approval_mode: ApprovalMode;
  model_timeout?: ModelTimeoutPolicyDocument | null;
  tool_deadline?: ToolDeadlinePolicyDocument | null;
  attempt: AttemptId;
  generation: RuntimeResourceRevision;
  model: SessionModelView;
  resources: CapabilityInspection;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectiveField".
 */
export interface EffectiveField {
  name: EffectiveFieldName;
  value?: unknown;
  reference?: EffectiveFieldRef | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "EffectiveCollection".
 */
export interface EffectiveCollection {
  field: EffectiveFieldName;
  total: number;
  kind: string;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "McpProbeResult".
 */
export interface McpProbeResult {
  id: McpServerId;
  revision: string;
  outcome: McpProbeOutcome;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceFieldPage".
 */
export interface SourceFieldPage {
  revision: string;
  field: SourceFieldName;
  kind: string;
  total: string;
  items: SourcePageItem[];
  next_offset?: number | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourcePageItem".
 */
export interface SourcePageItem {
  index: string;
  value?: unknown;
  reference?: SourceFieldRef | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceValueChunk".
 */
export interface SourceValueChunk {
  reference: SourceFieldRef;
  offset: number;
  end_offset: number;
  data_base64: string;
  next_offset?: number | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "BoundedSourceSettings".
 */
export interface BoundedSourceSettings {
  target: SourceTarget;
  revision: string;
  inline?: SourceSettings | null;
  fields: SourceField[];
  collections: SourceCollection[];
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceSettings".
 */
export interface SourceSettings {
  target: SourceTarget;
  process_policy_impacts: {
    [k: string]: ProcessPolicyImpact;
  };
  process_bindings?: AppServerPolicy | null;
  application?: ConfigurationApplication | null;
  /**
   * Native current-file approval resolution. None when prospective configuration is invalid.
   */
  prospective_approval_mode?: ApprovalMode | null;
  /**
   * Current-file analysis, separate from the published runtime. Never prepares sources.
   */
  prospective_resources?: CapabilityInspection | null;
  prospective_diagnostic?: string | null;
  /**
   * Exact CAS token for a currently absent resource identity.
   */
  absent_resource_revision: string;
  resource_revisions: {
    [k: string]: string;
  };
  /**
   * Read-only native source resolution; never a Session adopted binding.
   */
  resolved?: RuntimeLayer | null;
  /**
   * The catalog a Session created in this Workspace binds, exactly as its
   * `session/models` then serves it. Absent for the User target. Clients
   * select pre-Session models only from here, never from `resolved`.
   */
  session_models?: SessionModelsView | null;
  provenance: {
    [k: string]: Origin;
  };
  user: SourceView;
  workspace?: SourceView | null;
  /**
   * The latest automatic repair observation of the User source in this
   * process. Absent when the finite ledger retains none — never needed,
   * withdrawn, or forgotten — which never proves the source valid; the
   * views' diagnostics report that.
   */
  user_repair?: ConfigRepairReceipt | null;
  /**
   * The latest automatic repair observation of the target Workspace source.
   */
  workspace_repair?: ConfigRepairReceipt | null;
  /**
   * The native repair-ledger sequence `user_repair` and `workspace_repair`
   * are current at, read with them under one lock. It orders this read
   * against the `sequence` of each `configuration/sourceRepairChanged`
   * for the same source: the higher one is the newer observation.
   */
  repair_sequence: string;
  user_resource_root: string;
  workspace_resource_root?: string | null;
  runtime_root: string;
  user_mcp: SourceView2;
  workspace_mcp?: SourceView2 | null;
  agents: AgentSourceView[];
}
/**
 * The safe public catalog view served to Runtime Clients.
 *
 * A client selects a model and a Model Profile from this view; it never
 * reads `rustx.toml` itself and never sees a credential, an adapter, or a
 * provider HTTP client.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelCatalogView".
 */
export interface ModelCatalogView {
  /**
   * Every selectable model in deterministic reference order.
   */
  models?: CatalogModelView[];
}
/**
 * One selectable model of the public catalog view.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CatalogModelView".
 */
export interface CatalogModelView {
  /**
   * An authored Model identity. Its spelling has no provider or wire semantics.
   */
  model: string;
  /**
   * The model interaction protocol an adapter must speak.
   */
  protocol: 'openai_chat_completions' | 'openai_responses' | 'anthropic_messages';
  /**
   * The model context window in tokens.
   */
  contextWindow: number;
  /**
   * The configured maximum output tokens.
   */
  maxOutputTokens: number;
  declaredCapabilities: ModelCapabilities4;
  effectiveCapabilities: ModelCapabilities5;
  /**
   * The declared Model Profiles in deterministic order.
   */
  profiles?: ModelProfileView[];
  /**
   * The profile selected when an invocation does not choose one.
   */
  defaultProfile?: ModelProfileId | null;
  /**
   * The redacted client-facing description of a credential source.
   *
   * This is safe to place in Runtime Client results: it carries the source
   * *kind* and, for an environment reference, the variable *name*. It never
   * carries a credential value.
   */
  credentialSource:
    | {
        type: 'literal';
      }
    | {
        /**
         * The environment variable name (never its value).
         */
        variable: string;
        type: 'environment';
      };
}
/**
 * The capabilities the catalog claims.
 */
export interface ModelCapabilities4 {
  /**
   * The accepted input modalities.
   */
  inputModalities: Modality[];
  /**
   * The produced output modalities.
   */
  outputModalities: Modality[];
  /**
   * Whether the model can be given tool definitions and can call them.
   */
  toolCalls: boolean;
  /**
   * Whether the model semantically supports reasoning.
   */
  reasoning: boolean;
  /**
   * Native API-enforced JSON Schema output for this model, endpoint and protocol.
   * An omitted claim is conservative; ordinary text requests never opt in implicitly.
   */
  jsonSchemaOutput?: boolean;
}
/**
 * The capabilities the runtime can actually deliver today.
 */
export interface ModelCapabilities5 {
  /**
   * The accepted input modalities.
   */
  inputModalities: Modality[];
  /**
   * The produced output modalities.
   */
  outputModalities: Modality[];
  /**
   * Whether the model can be given tool definitions and can call them.
   */
  toolCalls: boolean;
  /**
   * Whether the model semantically supports reasoning.
   */
  reasoning: boolean;
  /**
   * Native API-enforced JSON Schema output for this model, endpoint and protocol.
   * An omitted claim is conservative; ordinary text requests never opt in implicitly.
   */
  jsonSchemaOutput?: boolean;
}
/**
 * One selectable Model Profile of the public catalog view.
 *
 * The profile's provider request parameters are provider-owned wire
 * configuration a client never needs to select a profile; the runtime
 * semantics it carries are exposed.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelProfileView".
 */
export interface ModelProfileView {
  /**
   * The identity of one Model Profile declared by a model.
   *
   * The runtime assigns no meaning to the name: `fast`, `precise`, `creative`,
   * `thinking-32k`, and `deep` are all just names whose behaviour is exactly
   * the profile's declared reasoning state, output default, and
   * `request_params`.
   */
  id: string;
  /**
   * Whether the profile semantically enables reasoning.
   */
  reasoningEnabled: boolean;
  /**
   * The profile's default output budget, when it declares one.
   */
  maxOutputTokens?: number | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceView".
 */
export interface SourceView {
  path: string;
  revision: string;
  authored?: RuntimeLayer | null;
  diagnostic?: string | null;
}
/**
 * Bounded, redacted native fact about one automatic repair of one source.
 *
 * It describes authored bytes only: it never claims that any configuration
 * was prepared, applied or adopted.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ConfigRepairReceipt".
 */
export interface ConfigRepairReceipt {
  /**
   * Opaque identity of this repair occurrence within its App Server
   * authority: one committed (or possibly committed) publication, or one
   * unresolved episode. Rereading or redelivering the receipt keeps it;
   * another commit gets another, even of identical bytes and changes.
   */
  id: string;
  target: SourceTarget;
  status: ConfigRepairStatus;
  /**
   * The exact source revision the repair was planned from.
   */
  before_revision: string;
  /**
   * The committed revision, or the revision observed after an uncertain
   * publication. Absent when nothing was published.
   */
  after_revision?: string | null;
  /**
   * Every eligible change, including those beyond `changes`.
   */
  changed_count: number;
  changes: ConfigRepairChange[];
  /**
   * Whether `changes` omits entries beyond [`MAX_RECEIPT_CHANGES`].
   */
  truncated: boolean;
  /**
   * Whether an exact private copy of the original bytes exists.
   */
  backup_available: boolean;
  /**
   * Why the source is unresolved or uncertain; never source excerpts.
   */
  diagnostic?: string | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceView2".
 */
export interface SourceView2 {
  path: string;
  revision: string;
  authored?: {
    [k: string]: McpView;
  } | null;
  diagnostic?: string | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "McpView".
 */
export interface McpView {
  definition: McpAuthoring;
  retained_env: string[];
  retained_headers: string[];
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentSourceView".
 */
export interface AgentSourceView {
  scope: SourceScope;
  name: SubagentName;
  source: SourceView3;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceView3".
 */
export interface SourceView3 {
  path: string;
  revision: string;
  authored?: AgentProfileDocument | null;
  diagnostic?: string | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceField".
 */
export interface SourceField {
  name: SourceFieldName;
  value?: unknown;
  reference?: SourceFieldRef | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SourceCollection".
 */
export interface SourceCollection {
  field: SourceFieldName;
  total: number;
  kind: string;
}
/**
 * The Workflow files physically owned by one source scope.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowLibrary".
 */
export interface WorkflowLibrary {
  target: SourceTarget;
  /**
   * The canonical directory owning this scope's Workflow files.
   */
  directory: string;
  /**
   * The CAS token of an absent file: the expected revision of a create or
   * copy destination.
   */
  absent_revision: string;
  /**
   * Files in deterministic byte order of their basenames.
   */
  files: WorkflowFileSummary[];
  /**
   * A directory-level failure; `files` is then empty, never partial.
   */
  diagnostic?: WorkflowDiagnostic | null;
}
/**
 * One file physically owned by the listed scope.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowFileSummary".
 */
export interface WorkflowFileSummary {
  /**
   * The file's basename, lossily decoded for presentation.
   */
  file_name: string;
  /**
   * The native identity; absent when the filename is not a Workflow id.
   */
  id?: WorkflowId | null;
  /**
   * The location relative to the scope's owning directory, e.g.
   * `.agents/workflows/<id>.yaml`.
   */
  relative_path: string;
  /**
   * Exact byte revision, the CAS token of edits, copies and deletion.
   */
  revision: string;
  byte_length: string;
  /**
   * Last modification in milliseconds since the Unix epoch, when known.
   */
  modified_ms?: number | null;
  /**
   * The bounded authored description of a parseable document.
   */
  description?: string | null;
  /**
   * Root-block node count of a parseable document.
   */
  nodes?: number | null;
  /**
   * Why this file is not a valid Workflow program.
   */
  diagnostic?: WorkflowDiagnostic | null;
  /**
   * Informational: a Workspace file whose id also names a User (Global)
   * file, which it shadows in this Workspace. Edits never redirect.
   */
  shadows_global: boolean;
}
/**
 * One bounded native diagnostic of a Workflow file or candidate.
 *
 * `path` is the native authored field path (`block.nodes.<id>.…`, `$` for the
 * document) so a client can select the owning block, node and field. Reasons
 * and corrections are static native text; `detail` is the bounded compiler
 * rejection. Parser messages, unknown field names and authored values are
 * never copied here.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowDiagnostic".
 */
export interface WorkflowDiagnostic {
  path: string;
  category: string;
  reason: string;
  correction?: string | null;
  detail?: string | null;
  line?: number | null;
  column?: number | null;
}
/**
 * The complete authorized source of one exact Workflow file.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowSource".
 */
export interface WorkflowSource {
  target: SourceTarget;
  id: WorkflowId;
  path: string;
  relative_path: string;
  /**
   * The exact byte revision; the absent revision when the file is missing.
   */
  revision: string;
  present: boolean;
  modified_ms?: number | null;
  /**
   * The parsed native definition. Absent for a missing, unparseable or
   * browser-unrepresentable file, whose bytes stay untouched on disk.
   */
  document?: WorkflowDefinition | null;
  /**
   * The first structural (parse or compile) or representability failure.
   */
  diagnostic?: WorkflowDiagnostic | null;
  vocabulary: WorkflowVocabulary;
}
/**
 * The native names an author can choose from in this scope: discovered
 * named Agent profiles and the builtin Tools a Tool node can select.
 * Choices, never admission: the native check decides.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowVocabulary".
 */
export interface WorkflowVocabulary {
  profiles: SubagentName[];
  builtin_tools: string[];
}
/**
 * One offline check of one exact candidate document.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowCheckReport".
 */
export interface WorkflowCheckReport {
  structure: WorkflowStructure;
  dependencies: WorkflowDependencies;
  /**
   * The redacted static explanation, when requested of a valid candidate.
   */
  explanation?: WorkflowInspection2 | null;
}
/**
 * Static program facts. Literals and task text are deliberately not exported.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "WorkflowInspection2".
 */
export interface WorkflowInspection2 {
  identity: WorkflowToolIdentity;
  stage: string;
  input_schema: unknown;
  output_schema: unknown;
  configured_timeout_ms: string;
  conservative_steps: number;
  conservative_agent_runs: number;
  conservative_model_calls: number;
  conservative_retained_bytes: number;
  caps: {
    [k: string]: number;
  };
  blocks: {
    [k: string]: BlockInspection;
  };
  tools: ExactToolSelector[];
  profiles: SubagentName[];
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "BlockInspection".
 */
export interface BlockInspection {
  entry: string;
  input_schema: unknown;
  output_schema: unknown;
  nodes: {
    [k: string]: NodeInspection;
  };
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "NodeInspection".
 */
export interface NodeInspection {
  kind: string;
  edges: [unknown, unknown][];
  bindings: {
    [k: string]: unknown;
  };
  profile?: SubagentName | null;
  /**
   * Selector identities only; provider parameters may contain secrets.
   */
  model?: ModelPolicyInspection | null;
  /**
   * The node's invocation override, when it carries one.
   */
  invocation_override?: AgentOverrideInspection | null;
  selector?: ExactToolSelector | null;
  output_schema?: unknown;
  result_part?: number | null;
  result_kind?: string | null;
  branches: string[];
  max_iterations?: number | null;
  review_subject?: string | null;
}
/**
 * The bounded projection of one Agent node's trusted static invocation
 * override.
 *
 * Presence is the fact this projection has to carry, so each dimension stays
 * an `Option`: `None` is "this dimension keeps the role's default" and
 * `Some` is "this dimension is replaced by exactly this". Only identities
 * are exported — capability selectors, Skill names, and the composed
 * extension names — never a Skill body, a prompt, a credential, or a
 * materialization secret.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentOverrideInspection".
 */
export interface AgentOverrideInspection {
  tools?: AgentToolSelection[] | null;
  skills?: string[] | null;
  extensions?: string[] | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "BoundedSessionApplication".
 */
export interface BoundedSessionApplication {
  scope: string;
  sources: SourceTarget[];
  version: string;
  inline?: ConfigurationApplication | null;
  reference?: SessionApplicationRef | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionApplicationChunk".
 */
export interface SessionApplicationChunk {
  reference: SessionApplicationRef;
  offset: number;
  end_offset: number;
  data_base64: string;
  next_offset?: number | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionSettingsChunk".
 */
export interface SessionSettingsChunk {
  reference: SessionSettingsRef;
  offset: number;
  end_offset: number;
  data_base64: string;
  next_offset?: number | null;
}
/**
 * Bounded delivery of the Session catalog's immutable settings read cut.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "BoundedSessionSettings".
 */
export interface BoundedSessionSettings {
  revision: string;
  cwd: string;
  inline?: SessionPersistentState | null;
  reference?: SessionSettingsRef | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "Failure".
 */
export interface Failure {
  jsonrpc: JsonRpcVersion;
  id?: AppServerRequestId | null;
  error: RpcError;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RpcError".
 */
export interface RpcError {
  code: number;
  message: string;
  data?: ErrorData | null;
}
/**
 * Heavy inspection detail for one exact record identity.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceDetail".
 */
export interface TraceDetail {
  /**
   * Historical projection content, distinct from committed execution messages.
   */
  projected_messages: TraceRequestMessage[];
  /**
   * Echoes the requested identity so a late reply can be fenced.
   */
  id: string;
  kind: TraceKind;
  request?: TraceRequestDetail | null;
  tool?: TraceToolDetail | null;
  /**
   * Canonical messages in native order (an adoption can contain a batch).
   */
  messages: TraceMessageDetail[];
  truncated: boolean;
}
/**
 * The exact historical request, reconstructed from frozen native authority.
 *
 * Every value comes from this request's own immutable Request Snapshot and
 * the historical Conversation Surface revision it froze. Current model
 * configuration, the current tool catalog and the current system prompt are
 * never consulted, so a request reopened after the Session was reconfigured
 * still shows what was actually sent.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceRequestDetail".
 */
export interface TraceRequestDetail {
  /**
   * Frozen physical/request coordinates and provider correlation labels.
   */
  tool_associations: ActualRequestToolAssociation[];
  tool_associations_truncated: boolean;
  /**
   * Typed historical context, never the current domain projection.
   */
  contributions: TraceContributionMetadata[];
  request_id: RequestId;
  attempt_id: AttemptId;
  step_id: TurnId;
  retry_number: number;
  assistant_message_id: MessageId;
  model: string;
  protocol: string;
  max_output_tokens: number;
  context_window_tokens: string;
  reasoning_enabled: boolean;
  profile?: string | null;
  /**
   * Allowlisted provider-neutral sampling options; see the options
   * allowlist for exactly which keys may appear.
   */
  options: TraceRequestOption[];
  /**
   * How many configured request parameters were outside the allowlist.
   * Their names are not disclosed; the count keeps the omission visible.
   */
  omitted_option_count: number;
  effective_system_prompt: TraceText;
  predecessor: TraceRequestPredecessor;
  /**
   * Absent for no predecessor or unavailable content; empty is a real value.
   */
  previous_system_prompt?: TraceText | null;
  /**
   * The reconstructed provider-neutral request context, in wire order.
   */
  messages: TraceRequestMessage[];
  messages_truncated: boolean;
  /**
   * The exact historical Tool definitions this request carried.
   */
  tools: TraceToolDefinition[];
  tools_truncated: boolean;
  usage?: ModelUsage | null;
  failure?: TraceRequestFailure | null;
  generation?: TraceGeneration | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ActualRequestToolAssociation".
 */
export interface ActualRequestToolAssociation {
  call: ActualRequestCallCoordinate;
  result: RequestMessageOccurrence;
  source: CanonicalCallSource;
  /**
   * Absent only when both occurrences are covered by native continuation
   * and no correlation field is transmitted in this actual request.
   */
  wire_id?: string | null;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ActualRequestCallCoordinate".
 */
export interface ActualRequestCallCoordinate {
  message: RequestMessageOccurrence;
  block: number;
}
/**
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CanonicalCallSource".
 */
export interface CanonicalCallSource {
  assistant: CanonicalMessageRef;
  block: ContentBlockIndex;
}
/**
 * The exact producer and typed data accepted for one historical request.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceContributionMetadata".
 */
export interface TraceContributionMetadata {
  message_id: MessageId;
  producer: ContextContributorIdentity;
  metadata: ContextKind;
  presentation?: TraceContributionPresentation | null;
}
/**
 * One allowlisted provider-neutral request option.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceRequestOption".
 */
export interface TraceRequestOption {
  name: string;
  value: TraceJson;
}
/**
 * The historical model-facing Tool definition of one request.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolDefinition".
 */
export interface TraceToolDefinition {
  tool_id: ToolId;
  name: string;
  description: TraceText;
  input_schema: TraceJson;
}
/**
 * The terminal failure of one actual request.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceRequestFailure".
 */
export interface TraceRequestFailure {
  kind: ModelErrorKind;
  message: TraceText;
}
/**
 * Heavy Tool inspection detail.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolDetail".
 */
export interface TraceToolDetail {
  call_id: ToolCallId;
  tool_id: ToolId;
  name?: string | null;
  lifecycle: TraceToolLifecycle;
  /**
   * Exact structured arguments from the canonical `ToolCall` proposal.
   */
  arguments?: TraceJson | null;
  /**
   * Program source identified by a native Tool contract, when there is one.
   */
  source?: TraceToolSource | null;
  /**
   * The historical definition this call's own request carried.
   */
  definition?: TraceToolDefinition | null;
  result?: TraceToolResult | null;
}
/**
 * Program source carried by an argument field of a native Tool contract.
 *
 * This exists only where a rustX native Tool identity states unambiguously
 * that one named argument is a program. It is never inferred from a value's
 * shape, a filename, or a file extension.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolSource".
 */
export interface TraceToolSource {
  /**
   * The argument field the native contract identifies as program source.
   */
  field: string;
  text: TraceText;
  /**
   * Highlighting language, present only when the native contract fixes
   * it. A tool whose contract does not fix a language leaves this absent
   * rather than guessing from the source.
   */
  language?: string | null;
}
/**
 * The canonical execution result of one Tool call.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolResult".
 */
export interface TraceToolResult {
  outcome: TraceToolOutcome;
  /**
   * Typed status detail: the error, denial reason, or cancellation reason.
   */
  detail?: TraceText | null;
  blocks: TraceContentBlock[];
  blocks_truncated: boolean;
  duration_ms: string;
  exit_code?: number | null;
  attachments: TraceArtifact[];
  /**
   * Tool-owned output truncation recorded by the execution itself.
   */
  truncation?: TraceToolTruncation | null;
  /**
   * Runtime-owned managed-output continuation metadata, when present.
   */
  managed_output?: TraceManagedOutput | null;
}
/**
 * Tool-recorded output truncation, distinct from Trace's own bounds.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceToolTruncation".
 */
export interface TraceToolTruncation {
  truncated: boolean;
  original_bytes?: string | null;
}
/**
 * Managed textual-output continuation metadata.
 *
 * The recorded locator is an inspectable execution fact. It is presentation
 * data only and confers no filesystem, execution, or recovery authority.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceManagedOutput".
 */
export interface TraceManagedOutput {
  /**
   * Whether the store holds this result's complete textual output, so the
   * bounded result content is a preview rather than the whole record.
   */
  complete: boolean;
  /**
   * Whether any managed output file exists at all.
   */
  available: boolean;
  /**
   * Exact native locator, when output storage owns one.
   */
  locator?: string | null;
  /**
   * The bounded advisory output-storage diagnostic, when one was recorded.
   */
  diagnostic?: TraceText | null;
}
/**
 * One canonical accepted message, projected for inspection.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceMessageDetail".
 */
export interface TraceMessageDetail {
  message_id: MessageId;
  role: TraceMessageRole;
  source?: string | null;
  blocks: TraceContentBlock[];
  truncated: boolean;
}
/**
 * The external view of the Agent Status opportunity set.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "AgentStatusOpportunityView".
 */
export interface AgentStatusOpportunityView1 {
  /**
   * The `FreshInbound` opportunity that produced this status, when one is
   * present. Future delivery opportunities can be added alongside it
   * without making this member structurally mandatory.
   */
  fresh_inbound?: FreshInboundOpportunityView | null;
  /**
   * The complete settled tool batch that made this existing primary step
   * eligible, when present.
   */
  post_tool_batch?: PostToolBatchOpportunityView | null;
}
/**
 * A selected non-Request record's finite inspection evidence, never a complete body.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceInspectionHead".
 */
export interface TraceInspectionHead {
  id: string;
  kind: TraceKind;
  record: TraceRecord;
  content: TraceInspectionContent;
}
/**
 * A bounded prefix of the selected original adoption, Assistant or Compaction's canonical rows.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceInspectionMessages".
 */
export interface TraceInspectionMessages {
  domain: string;
  total: string;
  items: TraceInspectionMessage[];
  next_offset?: number | null;
}
/**
 * Original C identity, finite presentation and authoritative per-field/per-block read evidence.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceInspectionMessage".
 */
export interface TraceInspectionMessage {
  locator: CanonicalContentLocator;
  identity?: MessageId | null;
  preview?: TracePreview | null;
  source?: UserSource | null;
  ordinary_input: boolean;
  context_kind?: TraceContextKind | null;
  attachments: TraceArtifact[];
  fields: TraceFieldSource[];
  collections: CollectionFrontier[];
}
/**
 * Bounded System Prompt presentation for one actual request.
 *
 * The complete prompt is deliberately absent: a page of 32 requests would
 * otherwise carry 32 complete system prompts. The exact historical value
 * stays in [`TraceRequestDetail::effective_system_prompt`].
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TraceSystemPromptPresentation".
 */
export interface TraceSystemPromptPresentation1 {
  state: TraceSystemPromptState;
  /**
   * One-line preview of the prompt this request introduced. Absent for
   * `Unchanged`, where the preceding request's row already carries it.
   * A present but empty preview records an empty historical prompt.
   */
  preview?: TracePreview | null;
}
/**
 * Finite metadata for one original model name at its native request owner.
 * The original model JSON remains readable from the enclosing domain/item/path[].
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientModelName".
 */
export interface RuntimeClientModelName {
  request_id?: RequestId | null;
  request_id_source?: JournalFieldRef | null;
  source_sequence?: string | null;
  encoded_bytes: number;
  value?: string | null;
}
/**
 * One pending interaction projected to the root Runtime Client.
 *
 * The request remains the originating conversation's immutable request. The
 * routed address and source only make that request understandable and
 * answerable at the shared human-facing surface.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RoutedInteraction".
 */
export interface RoutedInteraction {
  interaction: InteractionRef3;
  /**
   * Presentation metadata for the root interaction surface.
   *
   * This is projection data only. It grants no authority to the root router,
   * and it is never copied into either conversation's canonical history.
   */
  source:
    | {
        type: 'primary';
      }
    | {
        /**
         * Identifies one finite activation owned by an Agent, or one finite
         * Workflow child execution.
         *
         * Repeated native Agent activations have distinct `SubagentId`s while
         * retaining the same durable `AgentId`. This is not an OS pid: process
         * identity or PID reuse can never establish durable ownership or proof
         * of physical settlement.
         */
        subagent_id: string;
        /**
         * The child conversation that owns the interaction.
         */
        child_conversation_id: string;
        /**
         * The canonical typed name of one admitted subagent definition.
         *
         * The keyspace is deliberately narrow: lowercase ASCII letters, digits,
         * `-`, and `_`, starting with a letter. A name is the model-facing routing
         * token, the durable ownership identity, and the Runtime Client projection
         * identity, so an ambiguous or shell-shaped spelling is rejected at the
         * configuration boundary rather than normalized later.
         */
        agent_name: string;
        type: 'subagent';
      };
  request: InteractionRequest1;
}
/**
 * The root-facing address of a conversation-local interaction.
 *
 * `InteractionId` is allocated inside one conversation/attempt domain and is
 * intentionally not globally unique. The pair is the only identity that
 * crosses a Runtime Client or parent/child routing boundary.
 */
export interface InteractionRef3 {
  /**
   * The conversation-owned semantic interaction domain.
   */
  conversation_id: string;
  /**
   * The interaction identity allocated by that conversation's coordinator.
   */
  interaction_id: string;
}
/**
 * One live interaction request projected to a Runtime Client.
 */
export interface InteractionRequest1 {
  /**
   * Identifies one process-owned native human interaction.
   *
   * Interaction identities are derived from the conversation runtime's
   * non-reused [`AttemptId`] domain plus an ordinal owned by the
   * interaction coordinator.  They are deliberately not allocated from
   * a process-local counter alone: a response retained by a client from a
   * crashed process can therefore never name a different interaction in a
   * restarted runtime.
   */
  id: string;
  /**
   * The conversation that owns the interaction.
   */
  conversation_id: string;
  /**
   * One truthful native interaction owner. Scheduled work has no model turn.
   */
  execution:
    | {
        attempt_id: AttemptId;
        turn: number;
        type: 'attempt';
      }
    | {
        run: WorkflowRunId;
        type: 'scheduled';
      };
  /**
   * The bounded interaction facts.
   */
  kind:
    | {
        review: ReviewSpecification;
        subject_digest: string;
        type: 'review';
      }
    | {
        /**
         * Caller correlation for one native invocation. Capability source is
         * independently represented by [`ToolOrigin`].
         */
        invocation_id:
          | {
              call_id: ToolCallId;
              caller: 'agent';
            }
          | {
              node: WorkflowNodeInstance;
              caller: 'workflow';
            };
        /**
         * Identifies a tool definition in the capability set.
         */
        tool_id: string;
        /**
         * The safe model-facing tool name.
         */
        tool_name: string;
        /**
         * The registry-resolved tool origin.
         */
        origin:
          | 'builtin'
          | {
              mcp: {
                server_id: McpServerId;
              };
            }
          | {
              managed_python: {
                package: string;
              };
            };
        /**
         * The resolved execution ownership of one canonical invocation.
         *
         * The runtime resolves the effective mode from the tool's declared
         * [`ToolExecutionPolicy`] and (for `ModelSelectable`) the required
         * `execution_mode` invocation field, and delivers it to the executor as
         * canonical invocation data. The field is consumed before business-argument
         * validation and never reaches an executor. No policy resolution happens
         * inside executors.
         */
        mode: 'foreground' | 'background';
        /**
         * The already validated business arguments.  This is descriptive
         * data only; it is never accepted back as replacement input.
         */
        arguments: {
          [k: string]: unknown;
        };
        /**
         * The native policy's bounded explanation for asking.
         */
        reason: string;
        type: 'approval';
      }
    | {
        invocation_id: ToolInvocationId;
        requester: InteractionRequester1;
        questionnaire: QuestionnaireSpecification1;
        type: 'questionnaire';
      };
}
/**
 * The canonical, provider-independent identity of whoever asked the human.
 *
 * These are the registry-resolved facts of the tool invocation that reached
 * the interaction boundary — never a display string, never an rmcp value, and
 * never something a Runtime Client infers from the prompt text. The same
 * immutable value flows through the live request, the Event Journal subject,
 * the Runtime Client projection, and the TUI, so an MCP-originated prompt can
 * always name the server that asked:
 *
 * ```text
 * ToolOrigin::Mcp { server_id: "github" } + tool_name "create_issue"
 *     -> "Requested by MCP server: github / Tool: create_issue"
 * ToolOrigin::Builtin + tool_name "ask_user"
 *     -> the native wording, never labelled as MCP
 * ```
 *
 * It is deliberately orthogonal to
 * [`InteractionSource`](crate::runtime::interaction::InteractionSource):
 * *where* an interaction came from (primary or subagent) and *who* requested
 * it (native tool or MCP server) are two independent facts and are never
 * collapsed into one field.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InteractionRequester".
 */
export interface InteractionRequester2 {
  /**
   * Identifies a tool definition in the capability set.
   */
  tool_id: string;
  /**
   * The safe model-facing tool name.
   */
  tool_name: string;
  /**
   * The registry-resolved tool origin, which carries MCP server identity.
   */
  origin:
    | 'builtin'
    | {
        mcp: {
          server_id: McpServerId;
        };
      }
    | {
        managed_python: {
          package: string;
        };
      };
}
/**
 * The complete immutable questionnaire shown to a client.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "QuestionnaireSpecification".
 */
export interface QuestionnaireSpecification2 {
  /**
   * One to four related blocking questions.
   */
  questions: QuestionSpecification[];
}
/**
 * One invocation-scoped replacement of selected child capability
 * dimensions.
 *
 * The same value is produced by model-generated `subagent` arguments and by
 * trusted static Workflow program data. The launch boundary determines
 * authority; both share this vocabulary and the `AgentProfile` resolver.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SubagentInvocationOverride".
 */
export interface SubagentInvocationOverride1 {
  tools?: ToolSelectionDocument;
  /**
   * The complete replacement Skill selection, when this dimension is
   * overridden. Absent uses the definition's selection.
   */
  skills?: string[];
  plugins?: NativeAgentExtensionSelection;
}
/**
 * Shared Agent and Workflow Agent override selection. Extensions are composed
 * by their native owner and never enter this ordinary capability vocabulary.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolSelectionDocument".
 */
export interface ToolSelectionDocument2 {
  builtin?: string[];
  sources?: {
    [k: string]: SourceToolSelection;
  };
}
/**
 * The **presence-aware** closed selection an invocation override expresses.
 *
 * Complete profiles and invocation selections share omission semantics:
 * omitted members compose no extension. The outer invocation dimension is
 * presence-aware: absent retains the named default; present replaces it.
 * This wire record preserves the invocation protocol's camelCase spelling
 * and rejects explicit null members.
 *
 * The record stays closed exactly like the authored document: an unknown
 * extension name is rejected by `deny_unknown_fields`, so a misspelled or
 * not-yet-implemented extension fails deterministically instead of being
 * silently ignored.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "NativeAgentExtensionSelection".
 */
export interface NativeAgentExtensionSelection1 {
  agentStatus?: AgentStatusExtensionDocument;
  todo?: TodoExtensionDocument;
  goal?: GoalExtensionDocument;
}
/**
 * Configuration of the bounded Time status contributor.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TimeStatusConfig".
 */
export interface TimeStatusConfig1 {
  /**
   * Whether Time participates in an available Agent Status opportunity.
   */
  enabled?: boolean;
  /**
   * The optional IANA timezone used only by Time presentation. Omitting it
   * renders UTC.
   */
  timezone?: string | null;
}
/**
 * Configuration of the bounded Background status contributor.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "BackgroundStatusConfig".
 */
export interface BackgroundStatusConfig1 {
  /**
   * Whether Background participates in an available Agent Status opportunity.
   */
  enabled?: boolean;
}
/**
 * The closed authored composition surface of native Agent Extensions.
 *
 * Every member is a concrete named extension in a closed record.
 * Unknown extension names are rejected. Runtime TOML layers use the separate
 * `snake_case` authoring boundary; resource and wire documents retain their
 * own serialization contract.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "NativeAgentExtensionsDocument".
 */
export interface NativeAgentExtensionsDocument1 {
  agent_status?: AgentStatusExtensionDocument2;
  todo?: TodoExtensionDocument2;
  goal?: GoalExtensionDocument1;
}
/**
 * The original native filesystem root, independent of the viewing lineage.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SessionFileScope".
 */
export interface SessionFileScope1 {
  /**
   * Original Conversation. Copies retain this identity verbatim.
   */
  conversation_id: string;
  /**
   * Device/inode identity of the authorized root, encoded losslessly.
   */
  device: string;
  /**
   * Root inode; never silently rebound after root replacement.
   */
  inode: string;
}
/**
 * A token measurement of a model input, with explicit provenance.
 *
 * This is a Layer 0 value contract. The Context Engine owns the accounting
 * behavior that decides when a measurement is valid, but the measurement
 * itself is shared by runtime events, context projections, and the Runtime
 * Client read model.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "TokenMeasurement".
 */
export interface TokenMeasurement1 {
  /**
   * The measured or estimated input token count.
   */
  input_tokens: string;
  /**
   * How the measurement was obtained.
   */
  source: 'provider_reported' | 'provider_anchored' | 'estimated';
}
/**
 * Finite execution provenance. A deferred original identity remains available
 * through the source-owned response metadata domain at its immutable cut.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ObservedResponseOrigin".
 */
export interface ObservedResponseOrigin1 {
  conversation_id: ConversationId;
  attempt_id?: AttemptId | null;
  attempt_id_source?: AttemptIdentitySource | null;
  closing_message_id?: MessageId | null;
}
/**
 * Exact rustX canonical ownership of a Tool call, independent of provider IDs.
 * Message identity is remapped by lineage; the canonical content position stays fixed.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ToolCallOccurrenceRef".
 */
export interface ToolCallOccurrenceRef1 {
  /**
   * Identifies a committed canonical message block.
   */
  assistant_message_id: string;
  /**
   * A stable index identifying one content block within the ordered content
   * list of the canonical message being assembled.
   *
   * Streaming facts (text deltas, reasoning deltas, provider continuation
   * state, tool-call content) reference the block they belong to by this
   * index, so interleaved and multiple blocks remain unambiguous without
   * exposing any provider-specific block id type.
   */
  block_index: number;
}
/**
 * The semantic capability set of a model, an adapter/protocol, or the
 * runtime.
 *
 * Modalities are structured sets rather than one boolean per modality, so
 * adding a modality never reshapes the contract. The client-visible
 * capability is always an [`intersection`](ModelCapabilities::intersect) of
 * the model claim, the adapter/protocol capability, and the current runtime
 * capability — a raw catalog claim is never advertised.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "ModelCapabilities".
 */
export interface ModelCapabilities6 {
  /**
   * The accepted input modalities.
   */
  inputModalities: Modality[];
  /**
   * The produced output modalities.
   */
  outputModalities: Modality[];
  /**
   * Whether the model can be given tool definitions and can call them.
   */
  toolCalls: boolean;
  /**
   * Whether the model semantically supports reasoning.
   */
  reasoning: boolean;
  /**
   * Native API-enforced JSON Schema output for this model, endpoint and protocol.
   * An omitted claim is conservative; ordinary text requests never opt in implicitly.
   */
  jsonSchemaOutput?: boolean;
}
/**
 * The inbound mailbox diagnostics of the Runtime Client projection.
 *
 * A read-only view of authoritative mailbox state: it validates the
 * Issue #22 ordering contract and supports reconnect, and it can never
 * drain or mutate the mailbox.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "InboundDiagnostics".
 */
export interface InboundDiagnostics1 {
  /**
   * The currently pending inbound items in runtime-assigned inbound
   * sequence order.
   */
  pending?: InboundItemView[];
  /**
   * The latest observed finite drain boundary, when any drain occurred.
   */
  last_drain?: InboundDrainView | null;
}
/**
 * The latest live-activity projection of one subagent child.
 *
 * Owned and advanced by the child; the parent registry stores the newest
 * revision it has seen. Strictly increasing `revision` per applied
 * transition makes stale or reordered deliveries detectable and droppable.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SubagentObservation".
 */
export interface SubagentObservation1 {
  /**
   * The child-owned projection revision; strictly increasing per applied
   * transition.
   */
  revision: string;
  /**
   * Exact child-owned current Attempt; absent before start and after settlement.
   */
  attempt_id?: AttemptId | null;
  /**
   * What the child is observably doing right now.
   */
  activity:
    | {
        type: 'awaiting_activity';
      }
    | {
        /**
         * Identifies one actual provider-neutral model request.
         *
         * A request identity is distinct from an attempt, turn, retry ordinal,
         * and Event Journal sequence. Primary requests derive the `request:`
         * namespace from immutable [`RequestIdentity`](crate::model::snapshot::RequestIdentity).
         * Auxiliary requests allocate the separate `req_` `UUIDv7` namespace with
         * [`RequestId::generate_auxiliary`]. Each physical retry is a distinct
         * request. Identity alone grants no execution permission.
         */
        request_id: string;
        /**
         * The retry ordinal of THIS request: 0 for a first attempt, `n`
         * for the request the nth scheduled retry armed.
         */
        retry: number;
        type: 'model';
      }
    | {
        /**
         * The scheduled retry ordinal.
         */
        retry: number;
        type: 'retrying_model';
      }
    | {
        active: number;
        representative?: SubagentToolRepresentative | null;
        type: 'tool_group';
      }
    | {
        type: 'compacting';
      }
    | {
        /**
         * What the child waits on.
         */
        on:
          | {
              type: 'review';
            }
          | {
              /**
               * The tool awaiting approval.
               */
              tool_id?: ToolId | null;
              type: 'approval';
            }
          | {
              type: 'questionnaire';
            };
        type: 'waiting';
      };
  /**
   * When the latest applied transition was folded (child-side clock).
   */
  last_activity_at?: string | null;
  counters: SubagentActivityCounters;
}
/**
 * Cumulative activity counters of one child, kept across activity changes
 * so a consumer can observe throughput without folding every transition.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "SubagentActivityCounters".
 */
export interface SubagentActivityCounters1 {
  /**
   * Model requests started.
   */
  model_requests: number;
  /**
   * Model retry schedules observed (cumulative count).
   */
  model_retries: number;
  /**
   * Tool executions finished (completed plus failed).
   */
  tool_executions: number;
}
/**
 * The context diagnostics carried by the Runtime Client snapshot.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientContextView".
 */
export interface RuntimeClientContextView4 {
  /**
   * Independent current historical projection, including inherited activations.
   */
  projection_head?: RuntimeClientProjectionHeadView | null;
  /**
   * Latest manual operation admitted in this runtime incarnation. Never inferred
   * from a newer canonical checkpoint. Absence is insufficient evidence.
   */
  manual_compaction?: RuntimeClientManualCompaction | null;
  /**
   * Latest pre-commit diagnostic; cleared when the next compaction starts.
   */
  compaction_error?: string | null;
  /**
   * Latest native measured request or committed compacted-context estimate.
   */
  occupancy?: RuntimeClientContextOccupancy | null;
  /**
   * Whether the runtime currently owns a context-compaction operation.
   * This is live operation state, not inferred from token usage.
   */
  compaction_in_progress: boolean;
  /**
   * Runtime Client projection statistic: the number of committed
   * compaction completions folded into this read model. The compaction
   * generation remains the conversation-owned identity.
   */
  compaction_count: number;
  /**
   * The latest committed compaction metadata, when compaction occurred.
   */
  latest_compaction?: RuntimeClientCompactionView | null;
}
/**
 * The deterministic capability projection.
 *
 * Projected from the active [`CapabilitySnapshot`]
 * ([`crate::capabilities::CapabilitySnapshot`]) plus the
 * coordinator-owned availability state (Issue #81): the revision, the
 * active Tool catalog, the complete available Tool catalog, the deterministic
 * model-visible Skill catalog, and the typed per-source availability. No
 * executors, environment paths, package-manager state, or private dependency
 * internals appear.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "CapabilityView".
 */
export interface CapabilityView3 {
  /**
   * Configured Tool intent before effective model gating.
   */
  configured_tools: RuntimeClientTool[];
  /**
   * The active monotonic capability revision.
   */
  revision: string;
  /**
   * The deterministic active Tool catalog in registry order. Model
   * requests and execution use exactly this set.
   */
  tools?: RuntimeClientTool[];
  /**
   * The complete available Tool catalog, including inactive Tools. The
   * active set above is an explicit subset; availability never implies
   * model activation.
   */
  available_tools?: RuntimeClientTool[];
  /**
   * The deterministic model-visible Skill catalog ordered by Skill name.
   * Skills hidden by `disable-model-invocation` remain runtime-owned but
   * are omitted here. Every entry includes the canonical absolute host
   * path of its `SKILL.md`.
   */
  skills?: RuntimeClientSkill[];
  /**
   * The typed availability of every evaluated optional capability
   * source, in deterministic source-identity order (Issue #81).
   */
  sources?: CapabilitySourceView[];
}
/**
 * The client-visible projection of one immutable runtime resource
 * generation.
 *
 * It carries identity and provenance, never content: a client that wants
 * the text of a project context file reads the file, exactly as it would
 * read a `SKILL.md`. Publishing the bytes here would create a second copy
 * of request input inside a conversation projection.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientResourcesView".
 */
export interface RuntimeClientResourcesView2 {
  /**
   * Facts copied from the same immutable generation as this revision.
   * Deferred inspection facts are loaded through finite native field pages.
   */
  inspection?: CapabilityInspection | null;
  /**
   * Identifies one immutable process-local runtime resource generation.
   *
   * This is deliberately separate from `ContextGeneration` (one context
   * assembly provenance set) and `CapabilityRevision` (one executable
   * capability set). A resource-only change may advance this revision while
   * retaining an identical capability revision.
   */
  revision?: string;
  /**
   * The runtime-loaded project instruction files, root-most to
   * workspace, in the exact order the runtime concatenated them.
   */
  context_files?: RuntimeClientContextFile[];
  /**
   * Whether an immutable agent profile/persona is frozen into this
   * generation. The persona text itself is runtime-owned request input.
   */
  agent_profile?: boolean;
}
/**
 * One live-published requested interaction audit projection.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientTranscriptInteractionRequested".
 */
export interface RuntimeClientTranscriptInteractionRequested1 {
  /**
   * Identifies one durable runtime event.
   */
  event_id: string;
  /**
   * Event timestamp.
   */
  timestamp: string;
  /**
   * Owning Attempt; absent for native Scheduled work.
   */
  attempt_id?: AttemptId | null;
  /**
   * Owning turn; absent for native Scheduled work.
   */
  turn_id?: TurnId | null;
  /**
   * Identifies one process-owned native human interaction.
   *
   * Interaction identities are derived from the conversation runtime's
   * non-reused [`AttemptId`] domain plus an ordinal owned by the
   * interaction coordinator.  They are deliberately not allocated from
   * a process-local counter alone: a response retained by a client from a
   * crashed process can therefore never name a different interaction in a
   * restarted runtime.
   */
  interaction_id: string;
  /**
   * Bounded durable subject.
   */
  subject:
    | {
        review: ReviewSpecification;
        type: 'review';
      }
    | {
        /**
         * Caller correlation for one native invocation. Capability source is
         * independently represented by [`ToolOrigin`].
         */
        invocation_id:
          | {
              call_id: ToolCallId;
              caller: 'agent';
            }
          | {
              node: WorkflowNodeInstance;
              caller: 'workflow';
            };
        /**
         * Identifies a tool definition in the capability set.
         */
        tool_id: string;
        /**
         * The model-facing tool name.
         */
        tool_name: string;
        /**
         * The digest of the canonical model-issued arguments.
         */
        arguments_digest: string;
        /**
         * The bounded policy explanation shown to the client.
         */
        reason: string;
        type: 'approval';
      }
    | {
        invocation_id: ToolInvocationId;
        requester: InteractionRequester;
        questionnaire: QuestionnaireSpecification;
        type: 'questionnaire';
      };
}
/**
 * One live-published settled interaction audit projection.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientTranscriptInteractionSettled".
 */
export interface RuntimeClientTranscriptInteractionSettled1 {
  /**
   * Identifies one durable runtime event.
   */
  event_id: string;
  /**
   * Event timestamp.
   */
  timestamp: string;
  /**
   * Owning Attempt; absent for native Scheduled work.
   */
  attempt_id?: AttemptId | null;
  /**
   * Owning turn; absent for native Scheduled work.
   */
  turn_id?: TurnId | null;
  /**
   * Identifies one process-owned native human interaction.
   *
   * Interaction identities are derived from the conversation runtime's
   * non-reused [`AttemptId`] domain plus an ordinal owned by the
   * interaction coordinator.  They are deliberately not allocated from
   * a process-local counter alone: a response retained by a client from a
   * crashed process can therefore never name a different interaction in a
   * restarted runtime.
   */
  interaction_id: string;
  /**
   * Bounded durable settlement.
   */
  settlement:
    | {
        response: ReviewResponse;
        type: 'reviewed';
      }
    | {
        kind: ToolDeadlineKind;
        type: 'deadline_expired';
      }
    | {
        type: 'approved';
      }
    | {
        /**
         * The bounded client-facing denial reason.
         */
        reason: string;
        type: 'denied';
      }
    | {
        submission: QuestionnaireSubmission1;
        type: 'questionnaire_submitted';
      }
    | {
        type: 'questionnaire_declined';
      }
    | {
        /**
         * The first-winner cancellation cause.
         */
        reason:
          | 'user_requested'
          | 'runtime_shutdown'
          | 'parent_cancelled'
          | 'subagent_execution_deadline_exceeded';
        type: 'cancelled';
      };
}
/**
 * Finite proposal metadata; exact provider identifiers and arguments can be deferred.
 *
 * This interface was referenced by `ProtocolTypes`'s JSON-Schema
 * via the `definition` "RuntimeClientToolCall".
 */
export interface RuntimeClientToolCall2 {
  id?: ToolCallId | null;
  tool_id?: ToolId | null;
  name: string;
  arguments?: unknown;
  metadata_ref?: ContentRef | null;
}
