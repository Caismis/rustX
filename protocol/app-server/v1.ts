// Generated from Rust App Server DTOs. Run pnpm generate in protocol/app-server. Do not edit.

/**
 * Complete public wire surface used by schema and client generation.
 */
export type ProtocolMessage = Request | Response | Notification;
/**
 * A single public method space, with no nested Runtime Client envelope.
 */
export type Request = {
  jsonrpc: JsonRpcVersion;
  id: RequestId;
} & Request1;
export type JsonRpcVersion = '2.0';
/**
 * Integer request IDs use the JavaScript safe-integer range at admission.
 */
export type RequestId = string | number;
export type Request1 =
  | {
      method: 'settings/defaults';
      params: {
        target: AttachmentTarget;
        scope: DefaultScope;
      };
    }
  | {
      method: 'settings/saveDefault';
      params: {
        target: AttachmentTarget;
        scope: DefaultScope;
        expected_revision: string;
        setting: DefaultTarget;
      };
    }
  | {
      method: 'session/unload';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'session/transcript';
      params: {
        target: AttachmentTarget;
        before?: RuntimeClientTranscriptCursor | null;
        limit: number;
      };
    }
  | {
      method: 'settings/model';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'settings/models';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'settings/setModel';
      params: {
        target: AttachmentTarget;
        config: SessionModelConfig;
      };
    }
  | {
      method: 'settings/setApprovalMode';
      params: {
        target: AttachmentTarget;
        mode: ApprovalMode;
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
      method: 'background/status';
      params: {
        target: AttachmentTarget;
        execution_id: ToolExecutionId;
      };
    }
  | {
      method: 'background/cancel';
      params: {
        target: AttachmentTarget;
        execution_id: ToolExecutionId;
      };
    }
  | {
      method: 'subagent/status';
      params: {
        target: AttachmentTarget;
        subagent_id: SubagentId;
      };
    }
  | {
      method: 'subagent/cancel';
      params: {
        target: AttachmentTarget;
        subagent_id: SubagentId;
      };
    }
  | {
      method: 'subagent/disposeWorkspace';
      params: {
        target: AttachmentTarget;
        subagent_id: SubagentId;
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
      method: 'session/name';
      params: {
        session_id: SessionId;
        name: string;
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
      method: 'session/fork';
      params: {
        session_id: SessionId;
        node_id?: SessionNodeId | null;
        surface_revision: SurfaceRevision;
        boundary?: MessageId | null;
      };
    }
  | {
      method: 'session/branch';
      params: {
        session_id: SessionId;
        node_id: SessionNodeId;
        surface_revision: SurfaceRevision;
        boundary: MessageId;
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
      method: 'session/detach';
      params: {
        target: AttachmentTarget;
      };
    }
  | {
      method: 'session/snapshot';
      params: {
        target: AttachmentTarget;
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
        content: UserContentBlock[];
      };
    }
  | {
      method: 'turn/steer';
      params: {
        target: AttachmentTarget;
        content: UserContentBlock[];
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
      method: 'settings/read';
      params: {
        session_id: SessionId;
      };
    }
  | {
      method: 'settings/replace';
      params: {
        session_id: SessionId;
        expected_revision: string;
        settings: SessionPersistentState;
      };
    }
  | {
      method: 'resources/reload';
      params: {
        target: AttachmentTarget;
      };
    };
/**
 * Identifies one user-facing local Session.
 */
export type SessionId = string;
/**
 * Identifies a durable conversation.
 */
export type ConversationId = string;
/**
 * Process-local live composition identity. Never a durable or transport identity.
 */
export type RuntimeIncarnationId = string;
/**
 * The identity of one Runtime Client attachment.
 *
 * Distinct from [`ConversationId`], [`AttemptId`], [`RuntimeClientCursor`],
 * and request ids: one attachment is one client session, and reconnecting
 * always receives a new attachment identity.
 */
export type AttachmentId = string;
export type DefaultScope = 'user';
/**
 * The native setting to capture at the save operation boundary.
 */
export type DefaultTarget = 'model_selection' | 'approval_mode';
/**
 * The cursor domain of durable transcript paging.
 */
export type RuntimeClientTranscriptCursor = string;
/**
 * The identity of one reasoning profile declared by a model.
 *
 * The runtime assigns no meaning to the name: `off`, `on`, `low`,
 * `thinking-32k`, and `deep` are all just names whose wire behaviour is
 * exactly the profile's configured `request_params`.
 */
export type ReasoningProfileId = string;
/**
 * The runtime-wide control state for tool approval behavior.
 *
 * `Policy` consults each resolved Tool's
 * [`ToolApprovalPolicy`](crate::tools::types::ToolApprovalPolicy).
 * `FullAccess` changes only the effective approval result to `Never`; it
 * never changes availability, activation, execution ownership, concurrency,
 * or tool authority.
 */
export type ApprovalMode = 'policy' | 'full_access';
/**
 * Typed Runtime Client control; an existing-state mutation always names its observation.
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
 * Identifies one detached runtime execution instance of a background
 * tool.
 *
 * `ToolExecutionId` is distinct from `ToolCallId`: a `ToolCallId`
 * identifies the logical model-issued call, while a `ToolExecutionId`
 * identifies the runtime execution instance and may outlive the
 * attempt that created it. Allocation is conversation-owned and
 * monotonic (`exec_1`, `exec_2`, ...); cross-conversation uniqueness is
 * not required because the background registry is conversation-scoped.
 */
export type ToolExecutionId = string;
/**
 * Identifies one conversation-owned asynchronous one-shot subagent
 * (Issue #60).
 *
 * `SubagentId` is the logical lifecycle/delegation identity of a child
 * rustX runtime. It is deliberately not an OS pid: a pid is ephemeral
 * process state and is never durable identity, and pid reuse after a
 * restart can never prove that a surviving process is the previously
 * owned child.
 */
export type SubagentId = string;
/**
 * Identifies one linear lineage node inside a Session graph.
 */
export type SessionNodeId = string;
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
export type SurfaceRevision = string;
/**
 * Identifies a committed canonical message block.
 */
export type MessageId = string;
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
export type RuntimeClientCursor = string;
/**
 * A content block inside a `UserMessageBlock`.
 */
export type UserContentBlock =
  | {
      /**
       * The text content.
       */
      text: string;
      type: 'text';
    }
  | {
      /**
       * Durable artifact identity of the image.
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
       * Durable artifact identity of the file.
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
 * A typed response to one native interaction.
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
 * The configured workflow identity.
 *
 * This is both the catalog key and the eventual model-facing Tool name. It
 * is deliberately not repeated inside YAML.
 */
export type WorkflowId = string;
export type ReviewDecision =
  | {
      type: 'accepted';
    }
  | {
      feedback: string;
      type: 'rejected';
    };
/**
 * Success and failure are exclusive, including on deserialization.
 */
export type Response = Success | Failure;
export type MethodResult =
  | {
      document: DefaultDocument;
      type: 'defaults';
    }
  | {
      result: SaveDefaultResult;
      type: 'default_saved';
    }
  | {
      type: 'unloaded';
    }
  | {
      model: SessionModelView;
      type: 'model';
    }
  | {
      catalog: ModelCatalogView;
      type: 'models';
    }
  | {
      effective_approval_mode: ApprovalMode;
      pending_approval_mode?: ApprovalMode | null;
      revision: string;
      type: 'approval_mode';
    }
  | {
      capabilities: CapabilityView;
      type: 'capabilities';
    }
  | {
      context: RuntimeClientContextView;
      type: 'context';
    }
  | {
      page: RuntimeClientTranscriptPage;
      type: 'transcript';
    }
  | {
      view: GoalView;
      type: 'goal';
    }
  | {
      execution: RuntimeClientBackgroundExecution;
      type: 'background';
    }
  | {
      subagent: RuntimeClientSubagent;
      type: 'subagent';
    }
  | {
      subagent: RuntimeClientSubagent;
      outcome: RuntimeClientSubagentWorkspaceDisposalOutcome;
      type: 'workspace_disposed';
    }
  | {
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
      session: SessionSnapshot;
      editor_content?: UserContentBlock[] | null;
      durability_diagnostic?: string | null;
      type: 'session_transition';
    }
  | {
      sessions: SessionSummary[];
      next_offset?: number | null;
      type: 'sessions';
    }
  | {
      nodes: SessionNode[];
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
      boundaries: SessionUserMessageBoundary[];
      next_offset?: number | null;
      type: 'boundaries';
    }
  | {
      result: RuntimeClientSessionDeletionResult;
      type: 'deletion';
    }
  | {
      target: AttachmentTarget;
      snapshot: RuntimeClientSnapshot;
      cursor: RuntimeClientCursor;
      type: 'attached';
    }
  | {
      snapshot: RuntimeClientSnapshot;
      cursor: RuntimeClientCursor;
      type: 'snapshot';
    }
  | {
      type: 'detached';
    }
  | {
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
      revision: string;
      settings: SessionPersistentState;
      type: 'settings';
    }
  | {
      revision: string;
      type: 'settings_replaced';
    }
  | {
      resource_revision: string;
      capability_revision: CapabilityRevision;
      type: 'resources_reloaded';
    };
/**
 * A fully qualified catalog model reference: `provider-id/model-id`.
 *
 * The first `/` separates the provider from the model. The model ID itself
 * may contain additional `/` characters, as is common for Hugging Face
 * identities such as `Qwen/Qwen3`, but no model-ID segment may be empty.
 *
 * This is the explicit model-identity domain of the runtime. Concatenated
 * strings never travel through the runtime in its place: a reference either
 * resolves to exactly one catalog model or it fails.
 */
export type ModelRef = string;
/**
 * Already-captured finite mutation for the disk writer and published result.
 * Clients request a `DefaultTarget`; they never supply this as a save input.
 */
export type DefaultValue =
  | {
      selection: ModelDefault;
      field: 'model_selection';
    }
  | {
      mode: ApprovalMode;
      field: 'approval_mode';
    };
export type SettingsBoundary =
  | 'launch_capture'
  | 'next_admission'
  | 'safe_boundary'
  | 'resource_publication'
  | 'frozen_admission'
  | 'client_local'
  | 'next_launch';
/**
 * One semantic content modality of a model capability set.
 */
export type Modality = 'text' | 'image' | 'file';
/**
 * Identifies an MCP server bound to the runtime.
 */
export type McpServerId = string;
/**
 * The semantic family of one admitted model-visible context fact.
 */
export type ContextKind =
  | {
      goal_status: GoalSnapshot;
    }
  | 'runtime_tool_observation'
  | 'extension_environment'
  | {
      agent_status: AgentStatusGenerationMetadata;
    };
/**
 * Durable phase, independent of automatic continuation activation.
 */
export type GoalPhase = 'active' | 'paused' | 'blocked' | 'complete';
/**
 * Trusted origin supplied by runtime, never by model arguments.
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
 * Identifies one attempt to execute an agent manifest.
 */
export type AttemptId = string;
/**
 * The stable identity of one code-owned Agent Status module.
 *
 * This identity belongs to the canonical message layer because an active
 * Agent Status message must carry enough durable information for a later
 * Surface scan to identify the modules it contains. It is intentionally a
 * closed enum rather than extension metadata or a generic key/value field.
 */
export type AgentStatusModuleId = 'time' | 'background' | 'todo';
/**
 * A content block inside an `AssistantMessageBlock`.
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
       * Durable artifact identity of the image.
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
 * A content block inside a tool result.
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
       * Durable artifact identity of the file.
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
       * Durable artifact identity of the image.
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
 * One consolidated block of an immutable publication audit.
 *
 * Consolidation is what keeps the audit bounded: a stream that staged ten
 * thousand frames leaves one audit object whose size is the released output,
 * never O(number-of-frames) permanent staging rows.
 */
export type PublicationAuditBlock =
  | {
      /**
       * The output block.
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
       * The output block.
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
       * The output block.
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
       * The output block.
       */
      block_index: number;
      /**
       * The proposal identity.
       */
      call_id: string;
      /**
       * The registry-facing tool identity the model named.
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
export type ReviewSubject =
  | {
      content: unknown;
      candidate?: CandidateReference | null;
      type: 'plan';
    }
  | {
      reference: CandidateReference;
      inspection_path: string;
      type: 'candidate';
    };
/**
 * Identifies one tool call issued by the current agent.
 */
export type ToolCallId = string;
/**
 * Caller correlation for one native invocation. Capability source is
 * independently represented by [`ToolOrigin`].
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
 * The bounded set of text shapes rustX can validate deterministically.
 *
 * Only formats rustX can prove are supported. A schema asking for a format
 * outside this set is refused by its producer rather than accepted and then
 * silently unvalidated.
 */
export type TextFormat = 'date' | 'date_time' | 'uri';
/**
 * Canonical finite binary64 bits: lowercase hex, positive zero only; no NaN or infinity.
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
 */
export type ToolDeadlineKind = 'hard' | 'idle';
/**
 * The public result of disposing a retained subagent workspace.
 */
export type RuntimeClientSubagentWorkspaceDisposalOutcome =
  'disposed' | 'already_disposed' | 'disposal_pending' | 'no_retained_workspace';
/**
 * Bounded external outcomes shared by App Server and local presentation.
 */
export type RuntimeClientSessionDeletionResult =
  | {
      preview: RuntimeClientSessionDeletePreview;
      status: 'preview';
    }
  | {
      session_id: string;
      status: 'deleted';
    }
  | {
      session_id: string;
      status: 'stale';
    }
  | {
      session_id: string;
      reason: RuntimeClientSessionDeletionBlocker;
      status: 'blocked';
    }
  | {
      session_id: string;
      status: 'committed_cleanup_pending';
    }
  | {
      session_id: string;
      status: 'committed_durability_uncertain';
    }
  | {
      session_id: string;
      status: 'not_found';
    };
/**
 * Bounded safety summary. Resource identities and storage diagnostics stay native.
 */
export type RuntimeClientSessionDeletionBlocker =
  | {
      kind: 'current_session';
    }
  | {
      kind: 'in_use';
    }
  | {
      resource_count: number;
      kind: 'workspace';
    }
  | {
      kind: 'invalid_ownership';
    };
/**
 * Which native evidence is available for the canonical settings sections.
 */
export type SettingsEvidence = ('live_session' | 'frozen_child') | 'historical_partial';
export type SettingOrigin =
  | {
      kind: 'builtin';
    }
  | {
      document: string;
      kind: 'user';
    }
  | {
      document: string;
      kind: 'project';
    }
  | {
      kind: 'cli';
    };
/**
 * Identifies one immutable process-local runtime resource generation.
 *
 * This is deliberately separate from `ContextGeneration` (one context
 * assembly provenance set) and `CapabilityRevision` (one executable
 * capability set). A resource-only change may advance this revision while
 * retaining an identical capability revision.
 */
export type RuntimeResourceRevision = string;
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
export type WorkflowWait =
  | 'tool'
  | 'agent'
  | 'capacity'
  | 'workspace'
  | 'questionnaire'
  | 'approval'
  | 'review'
  | 'settlement';
/**
 * Bounded block/node observation; live state stays in the executor.
 */
export type WorkflowExecutionOutcome =
  'completed' | 'failed' | 'cancelled' | 'denied' | 'timed_out' | 'outcome_unknown';
export type WorkflowNodeKind =
  'block' | 'agent' | 'tool' | 'branch' | 'parallel' | 'review' | 'loop' | 'return';
/**
 * Identifies a tool definition in the capability set.
 */
export type ToolId = string;
/**
 * Normal finite Loop completion; neither case claims business verification passed.
 */
export type WorkflowLoopExit = 'satisfied' | 'exhausted';
/**
 * The canonical conversation message.
 *
 * The `role` discriminator is stable: `user`, `assistant`, `tool`.
 * No additional top-level role exists.
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
 * One ordered block of an in-flight Assistant message.
 */
export type InFlightBlock =
  | {
      /**
       * The canonical block index.
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
       * The canonical block index.
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
       * The canonical block index.
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
       * The canonical block index.
       */
      block_index: number;
      /**
       * Identifies one tool call issued by the current agent.
       */
      call_id: string;
      /**
       * Identifies a tool definition in the capability set.
       */
      tool_id: string;
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
 * One structured Agent Status section of the external view.
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
      executions: RuntimeClientBackgroundExecution[];
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
export type AgentIdentity =
  | {
      kind: 'main';
    }
  | {
      kind: 'named';
      name: SubagentName;
    };
/**
 * The canonical typed name of one admitted subagent definition.
 *
 * The keyspace is deliberately narrow: lowercase ASCII letters, digits,
 * `-`, and `_`, starting with a letter. A name is the model-facing routing
 * token, the durable ownership identity, and the Runtime Client projection
 * identity, so an ambiguous or shell-shaped spelling is rejected at the
 * configuration boundary rather than normalized later.
 */
export type SubagentName = string;
/**
 * Where a tool comes from.
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
 * A configured MCP source or a canonical Managed Python package.
 * Namespace parsing happens only at the authoring boundary; runtime dispatch
 * matches these variants, never display strings.
 */
export type ToolSourceId = string;
export type NativeExtension = 'agent_status' | 'todo' | 'goal';
/**
 * Typed native facts retained with the generation, never emitted per model turn.
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
      kind: 'disabled_skill_absent';
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
 */
export type SourceResolutionFailure =
  | {
      kind: 'undefined';
    }
  | {
      kind: 'inactive';
      detail: SourceActivation;
    }
  | {
      kind: 'unprepared';
    }
  | {
      kind: 'unavailable';
      detail: {};
    };
/**
 * Effective host-owned authority, not a writable configuration vocabulary.
 */
export type SourceActivation = 'unconfigured' | 'disabled' | 'enabled' | 'untrusted';
/**
 * Closed scope-ineligible capability identities, requiring no string parsing.
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
export type WorkflowInspection =
  | {
      status: 'enabled';
    }
  | {
      status: 'disabled';
      diagnostics: WorkflowAdmissionDiagnostic[];
    };
export type WorkflowDependencyFailure =
  | {
      kind: 'not_admitted';
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
 * One exact executable identity, used by Workflow Tool leaves and allowlists.
 * Source-wide Agent capability selection cannot be authored in this type.
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
export type SourceInspection =
  | {
      activation: SourceActivation;
      status: 'inactive';
    }
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
 */
export type SkillDiagnostic =
  | {
      /**
       * The source whose root is absent.
       */
      source: 'global' | 'workspace' | 'explicit';
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
      source: 'global' | 'workspace' | 'explicit';
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
      source: 'global' | 'workspace' | 'explicit';
      /**
       * Every root configured for that source, in canonical order.
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
      source: 'global' | 'workspace' | 'explicit';
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
      source: 'global' | 'workspace' | 'explicit';
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
      source: 'global' | 'workspace' | 'explicit';
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
      effective_source: 'global' | 'workspace' | 'explicit';
      /**
       * The effective package's `SKILL.md` location.
       */
      effective_location: string;
      /**
       * The source whose package was shadowed.
       */
      shadowed_source: 'global' | 'workspace' | 'explicit';
      /**
       * The shadowed package's `SKILL.md` location.
       */
      shadowed_location: string;
      kind: 'shadowed';
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
 */
export type InboundSequence = string;
/**
 * A monotonic revision counter for the capability set observed by an attempt.
 *
 * A running attempt snapshots one immutable `CapabilityRevision` when it
 * starts and keeps it for its entire lifetime. The revision is a counter,
 * not a provider-specific string: every capability mutation atomically swaps
 * the whole capability set and increments the revision.
 */
export type CapabilityRevision = string;
export type ErrorData =
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
      expected: string;
      actual: string;
      kind: 'stale_settings';
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
export type Notification = {
  jsonrpc: JsonRpcVersion;
} & Notification1;
export type Notification1 =
  | {
      method: 'session/event';
      params: {
        target: AttachmentTarget;
        cursor: RuntimeClientCursor;
        event: RuntimeClientEvent;
      };
    }
  | {
      method: 'session/resyncRequired';
      params: {
        target: AttachmentTarget;
        after_cursor: RuntimeClientCursor;
        earliest_serviceable: RuntimeClientCursor;
      };
    }
  | {
      method: 'session/closed';
      params: {
        target: AttachmentTarget;
      };
    };
/**
 * One externally visible Runtime Client observation.
 *
 * The stable `type` discriminator is the protocol contract; unknown
 * fields are rejected.
 */
export type RuntimeClientEvent =
  | {
      view: GoalView;
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
             * The normalized client-visible failure.
             */
            error:
              | {
                  /**
                   * The normalized model error kind.
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
                  retry_after_ms?: number | null;
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
      interaction: RoutedInteraction1;
      type: 'interaction_pending';
    }
  | {
      interaction: InteractionRef2;
      /**
       * The exact terminal rendezvous outcome.
       */
      outcome:
        | {
            type: 'review_invalidated';
          }
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
      interaction: InteractionRef3;
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
       * The latest desired mode when it is pending reconciliation.
       */
      pending_approval_mode?: ApprovalMode | null;
      /**
       * The monotonic control-plane revision.
       */
      revision: string;
      type: 'approval_mode_changed';
    }
  | {
      /**
       * The owning attempt for automatic compaction; absent for manual
       * idle maintenance.
       */
      attempt_id?: AttemptId | null;
      type: 'context_compaction_started';
    }
  | {
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
      context: RuntimeClientContextView2;
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
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      /**
       * Identifies a committed canonical message block.
       */
      message_id: string;
      /**
       * The output block the delta belongs to.
       */
      block_index: number;
      /**
       * The incremental text.
       */
      delta: string;
      type: 'assistant_text_delta';
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
      /**
       * The reasoning block the delta belongs to.
       */
      block_index: number;
      /**
       * The incremental reasoning text.
       */
      delta: string;
      type: 'assistant_reasoning_delta';
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
      /**
       * The refusal block the delta belongs to.
       */
      block_index: number;
      /**
       * The incremental refusal text.
       */
      delta: string;
      type: 'assistant_refusal_delta';
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
      /**
       * The tool-call content block being assembled.
       */
      block_index: number;
      call: ToolCallStart;
      type: 'tool_call_started';
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
      /**
       * The tool-call content block being assembled.
       */
      block_index: number;
      /**
       * Identifies one tool call issued by the current agent.
       */
      call_id: string;
      /**
       * The incremental JSON argument fragment.
       */
      arguments_delta: string;
      type: 'tool_call_arguments_delta';
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
      /**
       * The tool-call content block that completed.
       */
      block_index: number;
      call: ToolCall1;
      type: 'tool_call_assembled';
    }
  | {
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
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      /**
       * Identifies one tool call issued by the current agent.
       */
      tool_call_id: string;
      /**
       * Identifies a tool definition in the capability set.
       */
      tool_id: string;
      type: 'tool_execution_started';
    }
  | {
      /**
       * Identifies one attempt to execute an agent manifest.
       */
      attempt_id: string;
      /**
       * Identifies one tool call issued by the current agent.
       */
      tool_call_id: string;
      /**
       * Identifies a tool definition in the capability set.
       */
      tool_id: string;
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
      /**
       * Identifies one tool call issued by the current agent.
       */
      tool_call_id: string;
      /**
       * Identifies a tool definition in the capability set.
       */
      tool_id: string;
      result: ToolExecutionResult3;
      type: 'tool_execution_settled';
    }
  | {
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
      type: 'agent_status_composed';
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
      message: UserMessageBlock3;
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
      execution: RuntimeClientBackgroundExecution1;
      type: 'background_execution_updated';
    }
  | {
      subagent: RuntimeClientSubagent1;
      type: 'subagent_updated';
    }
  | {
      capabilities: CapabilityView2;
      type: 'capability_updated';
    }
  | {
      capabilities: CapabilityView3;
      resources: RuntimeClientResourcesView1;
      type: 'resource_generation_updated';
    }
  | {
      model: SessionModelView1;
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
 * Every attached operation addresses all routing domains explicitly.
 */
export interface AttachmentTarget {
  session_id: SessionId;
  conversation_id: ConversationId;
  runtime_incarnation: RuntimeIncarnationId;
  attachment_id: AttachmentId;
}
/**
 * The authoritative mutable model configuration of one conversation
 * session.
 *
 * This one type is the session's state, the `model_get` result, and the
 * `model_set` parameter: an update is a whole-state replacement, never an
 * ambiguous JSON patch.
 */
export interface SessionModelConfig {
  /**
   * The selected catalog model.
   */
  model: string;
  /**
   * The selected reasoning profile; the model default is used when
   * absent.
   */
  reasoningProfile?: ReasoningProfileId | null;
  /**
   * The session request-parameter overrides.
   */
  requestParams?: {
    [k: string]: unknown;
  };
  /**
   * The session output-budget override; the model's configured maximum is
   * used when absent.
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
         * The selected reasoning profile; the model default is used when
         * absent.
         */
        reasoning_profile?: ReasoningProfileId | null;
        /**
         * The explicit summary request-parameter overrides.
         */
        request_params?: {
          [k: string]: unknown;
        };
        /**
         * The explicit summary output-budget override.
         */
        max_output_tokens?: number | null;
        mode: 'explicit';
      };
}
/**
 * Exact observation required by every mutation of an existing Goal.
 */
export interface GoalRef {
  id: string;
  revision: string;
}
export interface InitializeParams {
  protocol_version: number;
  client: ClientIdentity;
  presentation: PresentationCapabilities;
}
export interface ClientIdentity {
  name: string;
  version: string;
}
/**
 * Rendering hints never confer execution or interaction authority.
 */
export interface PresentationCapabilities {
  images: boolean;
  questionnaires: boolean;
  reviews: boolean;
}
/**
 * Explicit Session context/selections, never materialized effective configuration.
 * Cwd, explicit project document, model and narrowing Skill/Tool selections
 * survive cold load. User bindings, source defaults/policy and live resources do
 * not. `None` is deliberately distinct from `Some(vec![])` for exact Tools.
 */
export interface SessionPersistentState {
  cwd: string;
  config?: string | null;
  model?: SessionModelConfig | null;
  skill_paths?: string[];
  no_automatic_skills?: boolean;
  no_builtin_tools?: boolean;
  no_direct_tools?: boolean;
  tools?: string[] | null;
  exclude_tools?: string[] | null;
}
/**
 * The root-facing address of a conversation-local interaction.
 *
 * `InteractionId` is allocated inside one conversation/attempt domain and is
 * intentionally not globally unique. The pair is the only identity that
 * crosses a Runtime Client or parent/child routing boundary.
 */
export interface InteractionRef {
  /**
   * Identifies a durable conversation.
   */
  conversation_id: string;
  /**
   * The interaction identity allocated by that conversation's coordinator.
   */
  interaction_id: string;
}
export interface ReviewResponse {
  instance: WorkflowNodeInstance;
  subject_digest: string;
  decision: ReviewDecision;
}
/**
 * One concrete node visit in an owning block instance.
 */
export interface WorkflowNodeInstance {
  block: WorkflowBlockInstance;
  node: string;
  visit: number;
}
/**
 * A concrete block instance. Root/Parallel append zero; Loop appends its one-based iteration.
 */
export interface WorkflowBlockInstance {
  run: WorkflowRunId;
  definition: WorkflowDefinitionPath;
  invocations: number[];
}
/**
 * Runtime-owned identity, independent of model `ToolCall` text.
 */
export interface WorkflowRunId {
  /**
   * Identifies a durable conversation.
   */
  conversation_id: string;
  /**
   * Native admitted attempt, unique across process recovery.
   */
  attempt_id: string;
  /**
   * WorkflowRuntime-owned invocation ordinal, allocated at run admission.
   */
  invocation: string;
}
/**
 * Static source location; never a concrete execution authority.
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
 */
export interface IntegerAnswer {
  /**
   * The typed value.
   */
  value: ('0' | string) & string;
}
/**
 * A typed boolean answer.
 */
export interface BooleanAnswer {
  /**
   * The canonical business value, never a `Yes`/`No` display string.
   */
  value: boolean;
}
/**
 * One declared option selected for a single-choice question.
 */
export interface OptionAnswer {
  /**
   * The zero-based index into the question's declared options.
   */
  option_index: number;
}
/**
 * Several declared options selected for a multi-choice question.
 */
export interface OptionsAnswer {
  /**
   * Zero-based option indices in ascending canonical order.
   */
  option_indices: number[];
}
/**
 * A custom answer entered by the user.
 */
export interface CustomAnswer {
  /**
   * The bounded user-entered answer.
   */
  answer: string;
}
export interface Success {
  jsonrpc: JsonRpcVersion;
  id: RequestId;
  result: MethodResult;
}
export interface DefaultDocument {
  scope: DefaultScope;
  document: string;
  /**
   * SHA-256 of exact bytes; the missing document has a distinct revision.
   */
  revision: string;
  model?: ModelDefault | null;
  approval_mode?: ApprovalMode | null;
}
/**
 * Only the two primary selection fields; never request parameters or credentials.
 */
export interface ModelDefault {
  model: ModelRef;
  reasoning_profile?: ReasoningProfileId | null;
}
export interface SaveDefaultResult {
  scope: DefaultScope;
  document: string;
  revision: string;
  changed: DefaultValue;
  live_unchanged: boolean;
  applies_at: SettingsBoundary;
}
/**
 * The redacted client-facing projection of the session model state.
 */
export interface SessionModelView {
  configured: SessionModelConfig1;
  effective: ModelInvocationView;
  /**
   * The resolved summary policy.
   */
  summary:
    | {
        mode: 'session';
      }
    | {
        /**
         * A fully qualified catalog model reference: `provider-id/model-id`.
         *
         * The first `/` separates the provider from the model. The model ID itself
         * may contain additional `/` characters, as is common for Hugging Face
         * identities such as `Qwen/Qwen3`, but no model-ID segment may be empty.
         *
         * This is the explicit model-identity domain of the runtime. Concatenated
         * strings never travel through the runtime in its place: a reference either
         * resolves to exactly one catalog model or it fails.
         */
        model: string;
        /**
         * The protocol of the binding.
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
         * The selected reasoning profile, when the model declares any.
         */
        reasoningProfile?: ReasoningProfileId | null;
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
   * The selected reasoning profile; the model default is used when
   * absent.
   */
  reasoningProfile?: ReasoningProfileId | null;
  /**
   * The session request-parameter overrides.
   */
  requestParams?: {
    [k: string]: unknown;
  };
  /**
   * The session output-budget override; the model's configured maximum is
   * used when absent.
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
         * The selected reasoning profile; the model default is used when
         * absent.
         */
        reasoning_profile?: ReasoningProfileId | null;
        /**
         * The explicit summary request-parameter overrides.
         */
        request_params?: {
          [k: string]: unknown;
        };
        /**
         * The explicit summary output-budget override.
         */
        max_output_tokens?: number | null;
        mode: 'explicit';
      };
}
/**
 * The resolved effective primary invocation.
 */
export interface ModelInvocationView {
  /**
   * A fully qualified catalog model reference: `provider-id/model-id`.
   *
   * The first `/` separates the provider from the model. The model ID itself
   * may contain additional `/` characters, as is common for Hugging Face
   * identities such as `Qwen/Qwen3`, but no model-ID segment may be empty.
   *
   * This is the explicit model-identity domain of the runtime. Concatenated
   * strings never travel through the runtime in its place: a reference either
   * resolves to exactly one catalog model or it fails.
   */
  model: string;
  /**
   * The protocol of the binding.
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
   * The selected reasoning profile, when the model declares any.
   */
  reasoningProfile?: ReasoningProfileId | null;
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
}
/**
 * The safe public catalog view served to Runtime Clients.
 *
 * A client selects a model and a reasoning profile from this view; it never
 * reads `models.toml` itself and never sees a credential, an adapter, or a
 * provider HTTP client.
 */
export interface ModelCatalogView {
  /**
   * Every selectable model in deterministic reference order.
   */
  models?: CatalogModelView[];
}
/**
 * One selectable model of the public catalog view.
 */
export interface CatalogModelView {
  /**
   * A fully qualified catalog model reference: `provider-id/model-id`.
   *
   * The first `/` separates the provider from the model. The model ID itself
   * may contain additional `/` characters, as is common for Hugging Face
   * identities such as `Qwen/Qwen3`, but no model-ID segment may be empty.
   *
   * This is the explicit model-identity domain of the runtime. Concatenated
   * strings never travel through the runtime in its place: a reference either
   * resolves to exactly one catalog model or it fails.
   */
  model: string;
  /**
   * The protocol an adapter speaks to this model.
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
   * The declared reasoning profiles in deterministic order.
   */
  reasoningProfiles?: ReasoningProfileView[];
  /**
   * The profile selected when a session does not choose one.
   */
  defaultReasoningProfile?: ReasoningProfileId | null;
  /**
   * The redacted credential source of the model's provider.
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
}
/**
 * One selectable reasoning profile of the public catalog view.
 *
 * Only the identity and the semantic enabled state are exposed: the
 * profile's provider request parameters are provider-owned wire config that
 * a client never needs to select a profile.
 */
export interface ReasoningProfileView {
  /**
   * The identity of one reasoning profile declared by a model.
   *
   * The runtime assigns no meaning to the name: `off`, `on`, `low`,
   * `thinking-32k`, and `deep` are all just names whose wire behaviour is
   * exactly the profile's configured `request_params`.
   */
  id: string;
  /**
   * Whether the profile semantically enables reasoning.
   */
  enabled: boolean;
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
 */
export interface CapabilityView {
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
 */
export interface RuntimeClientTool {
  /**
   * The canonical tool identity.
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
        /**
         * Source activation decision, containing no credential material.
         */
        activation: 'unconfigured' | 'disabled' | 'enabled' | 'untrusted';
        type: 'inactive';
      }
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
 * The context diagnostics carried by the Runtime Client snapshot.
 */
export interface RuntimeClientContextView {
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
 * Public metadata for one committed compaction.
 *
 * Every field is derived from already-committed conversation state. The
 * view names the canonical summary message by identity; its content is an
 * ordinary Ledger fact in [`RuntimeClientSnapshot::messages`].
 */
export interface RuntimeClientCompactionView {
  /**
   * The compaction generation maintained in the current Conversation
   * Surface head.
   */
  generation: string;
  /**
   * Identifies a committed canonical message block.
   */
  summary_message_id: string;
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
  tokens_before: TokenMeasurement;
  /**
   * The deterministic estimate of the rebuilt request context.
   */
  estimated_tokens_after: number;
}
/**
 * The pre-compaction input measurement and its provenance.
 */
export interface TokenMeasurement {
  /**
   * The measured or estimated input token count.
   */
  input_tokens: number;
  /**
   * How the measurement was obtained.
   */
  source: 'provider_reported' | 'provider_anchored' | 'estimated';
}
/**
 * One bounded newest-or-older page of derived transcript history.
 */
export interface RuntimeClientTranscriptPage {
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
 * One derived transcript item and its stable durable cursor.
 */
export interface RuntimeClientTranscriptEntry {
  /**
   * The cursor domain of durable transcript paging.
   */
  cursor: string;
  /**
   * The typed item resolved from a canonical durable owner.
   */
  item:
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
         * Durable Event Journal event identity.
         */
        event_id: string;
        /**
         * Event timestamp.
         */
        timestamp: string;
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        /**
         * Owning turn.
         */
        turn_id: string;
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
               * Caller-neutral invocation correlation.
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
               * The registry-resolved tool identity.
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
         * Durable Event Journal event identity.
         */
        event_id: string;
        /**
         * Event timestamp.
         */
        timestamp: string;
        /**
         * Identifies one attempt to execute an agent manifest.
         */
        attempt_id: string;
        /**
         * Owning turn.
         */
        turn_id: string;
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
              type: 'review_invalidated';
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
 * Inbound information supplied to the current agent.
 *
 * A `UserMessageBlock` does not necessarily mean a human spoke: it is the
 * canonical home for anything inbound, including messages from other agents
 * (with [`UserSource::Agent`] provenance) and runtime compaction summaries
 * (with [`InboundKind::CompactionSummary`] kind). It
 * must never become `AssistantMessageBlock` or `ToolMessageBlock`, which are
 * reserved for output and actions of the current agent.
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
           * Identity of the sending agent.
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
           * The rustX-derived logical extension identity.
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
        compaction_summary: CompactionSummaryMetadata;
      }
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
   * no wall-clock time is fabricated. Derived M4 compaction summaries
   * ([`InboundKind::CompactionSummary`]) never carry one. Older or
   * derived messages without a timestamp remain representable: the field
   * defaults to `None` on deserialization and is omitted from the
   * canonical encoding while absent.
   */
  timestamp?: string | null;
}
/**
 * The cumulative native file-operation facts of one compaction summary
 * (Issue #140).
 *
 * This is the typed canonical authority for *which files the retired history
 * read and which files it modified*. It is derived deterministically from
 * the canonical tool calls of the selected retired span — native
 * `read(path)` contributes a read, native `edit(path)` and `write(path)`
 * contribute a modification — merged with the metadata of every earlier
 * compaction summary inside that same span. It records conversation facts,
 * never current filesystem state: a path stays listed even when the file has
 * since been deleted, and the rendered `<read-files>`/`<modified-files>`
 * sections of the summary text are a model-visible projection of this value,
 * never its source.
 *
 * The fields are private so every value, including one decoded from durable
 * JSON, satisfies the canonical invariants: both lists are unique and in
 * ascending byte order, and `read_files ∩ modified_files = ∅` (modification
 * wins over read).
 *
 * ```compile_fail
 * use rustx::message::types::CompactionSummaryMetadata;
 *
 * fn mutate(metadata: &mut CompactionSummaryMetadata) {
 *     metadata.read_files = Vec::new();
 * }
 * ```
 */
export interface CompactionSummaryMetadata {
  read_files?: string[];
  modified_files?: string[];
}
/**
 * Authoritative bounded durable record.
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
 */
export interface ToolCall {
  /**
   * Identity of this specific call, referenced by the matching
   * `ToolMessageBlock` and tool-result events.
   */
  id: string;
  /**
   * Identity of the tool being called within the capability set.
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
 */
export interface ToolMessageBlock {
  /**
   * Identifies a committed canonical message block.
   */
  id: string;
  /**
   * Identity of the tool call this block answers.
   */
  tool_call_id: string;
  /**
   * Identity of the executed tool.
   */
  tool_id: string;
  result: ToolExecutionResult;
}
/**
 * The normalized execution result.
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
  duration_ms: number;
  /**
   * Process exit code where the tool executed a process.
   */
  exit_code?: number | null;
  /**
   * Durable artifact/file references produced by the execution.
   */
  artifacts?: FileReference[];
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
 */
export interface WorkflowToolIdentity {
  workflow_id: WorkflowId;
  program_digest: string;
}
/**
 * A reference to a file artifact.
 */
export interface FileReference {
  /**
   * Durable artifact identity of the file.
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
 */
export interface TruncationState {
  /**
   * Whether the result content was truncated.
   */
  truncated: boolean;
  /**
   * Size of the untruncated output in bytes, when known.
   */
  original_bytes?: number | null;
}
/**
 * The bounded immutable publication audit.
 */
export interface PublicationAudit {
  /**
   * The settled publication stream.
   */
  stream_id: string;
  /**
   * Identifies one attempt to execute an agent manifest.
   */
  attempt_id: string;
  /**
   * The turn that owned the stream.
   */
  turn_id: string;
  /**
   * The provider request the stream published.
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
export interface ReviewSpecification {
  instance: WorkflowNodeInstance;
  subject: ReviewSubject;
  context: ReviewFact[];
}
/**
 * Historical source identity. Possession of this value grants no access.
 */
export interface CandidateReference {
  run: WorkflowRunId;
  version: string;
  content: string;
}
export interface ReviewFact {
  value: unknown;
  candidate?: CandidateReference | null;
}
/**
 * The canonical identity of the tool that asked. Durable evidence of
 * *who* asked, so an audit of an MCP elicitation names its server.
 */
export interface InteractionRequester {
  /**
   * The canonical registry-resolved tool identity.
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
 * The current read model; observing it never arms continuation.
 */
export interface GoalView {
  current?: GoalSnapshot | null;
  armed: boolean;
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
export interface RuntimeClientBackgroundExecution {
  /**
   * Identifies one detached runtime execution instance of a background
   * tool.
   *
   * `ToolExecutionId` is distinct from `ToolCallId`: a `ToolCallId`
   * identifies the logical model-issued call, while a `ToolExecutionId`
   * identifies the runtime execution instance and may outlive the
   * attempt that created it. Allocation is conversation-owned and
   * monotonic (`exec_1`, `exec_2`, ...); cross-conversation uniqueness is
   * not required because the background registry is conversation-scoped.
   */
  execution_id: string;
  /**
   * The canonical tool identity.
   */
  tool_id: string;
  /**
   * The model-facing tool name.
   */
  tool_name: string;
  /**
   * The authoritative lifecycle state.
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
  result?: ToolExecutionResult1 | null;
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
  duration_ms: number;
  /**
   * Process exit code where the tool executed a process.
   */
  exit_code?: number | null;
  /**
   * Durable artifact/file references produced by the execution.
   */
  artifacts?: FileReference[];
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
 * The Runtime Client view of one subagent child (Issue #60).
 *
 * A read-model materialization of the authoritative registry snapshot:
 * every field is derived, and the durable ownership/terminal events —
 * never this view — are the recovery authority.
 *
 * Since Issue #178 the view also carries the child's live activity
 * projection (`observation`), its redacted execution profile
 * (`execution_profile`), and its start time (`started_at`). These are
 * observation-plane facts: the lifecycle `state` remains the only
 * authority on whether the child is alive, settling, or settled.
 */
export interface RuntimeClientSubagent {
  /**
   * Identifies one conversation-owned asynchronous one-shot subagent
   * (Issue #60).
   *
   * `SubagentId` is the logical lifecycle/delegation identity of a child
   * rustX runtime. It is deliberately not an OS pid: a pid is ephemeral
   * process state and is never durable identity, and pid reuse after a
   * restart can never prove that a surviving process is the previously
   * owned child.
   */
  subagent_id: string;
  /**
   * The child agent identity (the provenance its answer carries).
   */
  child_agent_id: string;
  /**
   * Identifies a durable conversation.
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
   * actually started with, so a later resource reload that redefines the
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
  state:
    | 'running'
    | 'cancelling'
    | 'publishing_terminal'
    | 'succeeded'
    | 'failed'
    | 'cancelled'
    | 'interrupted';
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
  workspace: RuntimeClientSubagentWorkspace;
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
   * What the child is observably doing right now.
   */
  activity:
    | {
        type: 'awaiting_activity';
      }
    | {
        /**
         * The in-flight provider request.
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
        /**
         * Identifies one tool call issued by the current agent.
         */
        tool_call_id: string;
        /**
         * The executed tool.
         */
        tool_id: string;
        /**
         * The latest bounded progress notification, when the tool
         * reported any.
         */
        progress?: ToolProgress | null;
        type: 'tool';
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
              tool_id: string;
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
 * model identity and reasoning selection. Credentials, endpoints, provider
 * bindings, and every other binding internal are never projected.
 */
export interface SubagentExecutionProfile {
  /**
   * The effective fully qualified model reference (`provider/model`).
   */
  model: string;
  /**
   * The selected reasoning profile, when the model declares any.
   */
  reasoning_profile?: ReasoningProfileId | null;
  /**
   * Whether the selected profile semantically enables reasoning.
   */
  reasoning_enabled: boolean;
}
/**
 * The model-independent project workspace facts.
 */
export interface RuntimeClientSubagentWorkspace {
  /**
   * Present when the Workflow run, rather than this child, owns the lease.
   */
  borrowed_from?: WorkflowRunId | null;
  /**
   * The authoritative logical project workspace used by the child.
   */
  logical_workspace: string;
  /**
   * The closed shared/isolated execution facts.
   */
  isolation:
    | {
        type: 'shared';
      }
    | {
        /**
         * The canonical source repository root.
         */
        source_repository_root: string;
        /**
         * The logical project scope relative to the source repository root.
         */
        repository_relative_workspace: string;
        /**
         * The runtime-owned physical worktree root.
         */
        physical_worktree_root: string;
        /**
         * The exact committed source snapshot selected before ownership.
         */
        base_commit: string;
        /**
         * The runtime-created branch/ref.
         */
        branch: string;
        /**
         * Whether the parent had uncommitted changes at selection time.
         */
        parent_had_uncommitted_changes: boolean;
        type: 'git_worktree';
      };
  /**
   * The post-terminal physical-resource lifecycle, independent of the
   * child's absorbing logical terminal state.
   */
  resource_state:
    | 'none'
    | 'retained'
    | 'preserved_unresolved'
    | 'disposal_in_progress'
    | 'worktree_removed'
    | 'disposed';
  /**
   * Retained child work-product facts, if the worktree was handed off.
   */
  handoff?: RuntimeClientWorkspaceHandoff | null;
}
/**
 * The Git facts needed to recover a preserved child worktree.
 */
export interface RuntimeClientWorkspaceHandoff {
  /**
   * The child's preserved logical project scope.
   */
  logical_workspace: string;
  /**
   * The preserved physical Git worktree root.
   */
  physical_worktree_root: string;
  /**
   * The runtime-created branch/ref.
   */
  branch: string;
  /**
   * The selected source commit.
   */
  base_commit: string;
  /**
   * The final child `HEAD`.
   */
  head_commit: string;
  /**
   * Whether ordinary tracked/index/untracked-non-ignored child state is
   * dirty. A changed child `HEAD` is reported independently by
   * `head_commit` and `base_commit`.
   */
  dirty: boolean;
}
export interface ServerCapabilities {
  multi_session: boolean;
  single_writable_controller: boolean;
  headless_interactions: boolean;
  experimental_methods: string[];
}
/**
 * Bounded authoritative metadata for one Session.
 *
 * The graph is deliberately not embedded here. Callers that need the graph
 * use the bounded tree page seam below, so `/session`, switch results, and
 * restart metadata never materialize every historical node.
 */
export interface SessionSnapshot {
  /**
   * Identifies one user-facing local Session.
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
   * Creation instant.
   */
  created_at: string;
  /**
   * Last metadata/active-node publication instant.
   */
  updated_at: string;
  /**
   * Identifies one linear lineage node inside a Session graph.
   */
  active_node: string;
  /**
   * Identifies a durable conversation.
   */
  active_conversation_id: string;
  /**
   * Number of persisted nodes, useful metadata for a bounded tree view.
   */
  node_count: number;
}
/**
 * One bounded row in the `/resume` selector.
 */
export interface SessionSummary {
  /**
   * Identifies one user-facing local Session.
   */
  id: string;
  /**
   * The user-defined display name, when this Session has one.
   */
  name?: string | null;
  /**
   * The first user message of this Session's root lineage, bounded to one
   * line. It is what an unnamed row is recognized by, and it is derived
   * for the page rather than stored: the catalog keeps no copy of
   * conversation content.
   */
  preview?: string | null;
  /**
   * Last metadata/active-node publication instant.
   */
  updated_at: string;
  /**
   * Identifies one linear lineage node inside a Session graph.
   */
  active_node: string;
}
/**
 * One node in the native Session graph.
 */
export interface SessionNode {
  /**
   * Identifies one linear lineage node inside a Session graph.
   */
  id: string;
  /**
   * Parent node within the same Session, when this is a tree branch.
   */
  parent?: SessionNodeId | null;
  /**
   * Identifies a durable conversation.
   */
  conversation_id: string;
  /**
   * Immutable product-level origin metadata.
   */
  origin:
    | {
        type: 'new';
      }
    | {
        /**
         * Identifies one user-facing local Session.
         */
        source_session: string;
        /**
         * Identifies one linear lineage node inside a Session graph.
         */
        source_node: string;
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
        source_surface_revision: string;
        type: 'clone';
      }
    | {
        /**
         * Identifies one user-facing local Session.
         */
        source_session: string;
        /**
         * Identifies one linear lineage node inside a Session graph.
         */
        source_node: string;
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
        source_surface_revision: string;
        /**
         * Identifies a committed canonical message block.
         */
        source_user_message: string;
        type: 'fork';
      };
}
/**
 * One user-message boundary the native product exposes for `/fork` and
 * `/tree`. The revision is part of the selection, so later source mutations
 * cannot change what the selection means.
 */
export interface SessionUserMessageBoundary {
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
  message: UserMessageBlock1;
}
/**
 * Inbound information supplied to the current agent.
 *
 * A `UserMessageBlock` does not necessarily mean a human spoke: it is the
 * canonical home for anything inbound, including messages from other agents
 * (with [`UserSource::Agent`] provenance) and runtime compaction summaries
 * (with [`InboundKind::CompactionSummary`] kind). It
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
           * Identity of the sending agent.
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
           * The rustX-derived logical extension identity.
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
        compaction_summary: CompactionSummaryMetadata;
      }
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
   * no wall-clock time is fabricated. Derived M4 compaction summaries
   * ([`InboundKind::CompactionSummary`]) never carry one. Older or
   * derived messages without a timestamp remain representable: the field
   * defaults to `None` on deserialization and is omitted from the
   * canonical encoding while absent.
   */
  timestamp?: string | null;
}
/**
 * Confirmation metadata; counts include the complete native ownership graph.
 */
export interface RuntimeClientSessionDeletePreview {
  session_id: string;
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
 * The authoritative Runtime Client snapshot of one conversation runtime.
 *
 * Every section is a deterministic projection of one authoritative
 * runtime owner. The shape belongs to the Runtime Client protocol: internal
 * snapshot types are projected into these external DTOs, never exposed
 * directly.
 */
export interface RuntimeClientSnapshot {
  settings_evidence: SettingsEvidence;
  /**
   * Immutable resolver facts; unavailable for durable-only or frozen-child attachment.
   */
  launch_settings?: LaunchSettings | null;
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
  effective_extensions?: EffectiveNativeAgentExtensions | null;
  settings_lifetimes: SettingsLifetimes;
  workflows: WorkflowSnapshot;
  /**
   * Identifies a durable conversation.
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
   * The latest requested mode when it is waiting for an attempt to settle.
   */
  pending_approval_mode?: ApprovalMode | null;
  /**
   * The runtime control-plane revision of the mode state.
   */
  approval_mode_revision?: string;
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
  transcript: RuntimeClientTranscriptPage1;
  /**
   * The current/latest attempt view, when any attempt exists.
   */
  attempt?: RuntimeClientAttempt | null;
  inbound: InboundDiagnostics;
  /**
   * Live process-owned native interaction requests. This is projection
   * state, never durable recovery input or client-owned truth.
   */
  pending_interactions?: RoutedInteraction[];
  /**
   * All background executions in execution allocation order, including
   * terminal records retained by the authoritative registry.
   */
  background?: RuntimeClientBackgroundExecution[];
  /**
   * All subagent children in subagent ordinal order, including terminal
   * records retained by the authoritative registry (Issue #60).
   */
  subagents?: RuntimeClientSubagent[];
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
  context?: RuntimeClientContextView1;
  capabilities: CapabilityView1;
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
   * Goal read model at this projection cursor; absent when disabled. Journal facts never reconstruct it.
   */
  goal?: GoalView | null;
}
/**
 * Captured resolver facts, explicitly NOT a read of today's disk defaults.
 */
export interface LaunchSettings {
  model: ModelDefault;
  model_origin: SettingOrigin;
  reasoning_origin: SettingOrigin;
  approval_mode: ApprovalMode;
  approval_origin: SettingOrigin;
  runtime_root_origin: SettingOrigin;
  tool_selection_origin: SettingOrigin;
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
 * - the snapshot's `effective_extensions` is `None` when there is no
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
 */
export interface EffectiveNativeAgentExtensions {
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
 */
export interface EffectiveAgentStatusExtension {
  time: EffectiveTimeStatus;
  background: EffectiveBackgroundStatus;
}
/**
 * The frozen Time contributor of a composed Agent Status extension.
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
 */
export interface EffectiveTodoExtension {}
/**
 * Frozen Goal composition; domain bounds are native, not launch settings.
 */
export interface GoalExtensionConfig {}
/**
 * Application boundaries of the existing canonical sections.
 */
export interface SettingsLifetimes {
  launch: SettingsBoundary;
  model: SettingsBoundary;
  approval: SettingsBoundary;
  resources: SettingsBoundary;
  attempt: SettingsBoundary;
  presentation: SettingsBoundary;
  saved_defaults: SettingsBoundary;
  /**
   * The application boundary of the effective native Agent Extension
   * composition (Issue #256).
   *
   * A root Agent's composition is launch-frozen, so this is
   * [`SettingsBoundary::LaunchCapture`]: only a restart can produce a
   * different one, and a resource reload never does. A Subagent child's
   * composition is the frozen execution profile its invoking generation
   * resolved, so a `frozen_child` snapshot reports
   * [`SettingsBoundary::FrozenAdmission`] instead — the same vocabulary
   * the frozen child model already uses.
   */
  extensions:
    | 'launch_capture'
    | 'next_admission'
    | 'safe_boundary'
    | 'resource_publication'
    | 'frozen_admission'
    | 'client_local'
    | 'next_launch';
}
/**
 * Bounded native Workflow state, never reconstructed from the journal.
 */
export interface WorkflowSnapshot {
  revision: string;
  runs: WorkflowRunView[];
  omitted_runs: number;
}
export interface WorkflowRunView {
  id: WorkflowRunId;
  workflow_id: WorkflowId;
  program_digest: string;
  resource_revision: RuntimeResourceRevision;
  tool_call_id: ToolCallId;
  state: WorkflowState;
  instances: WorkflowInstanceView[];
  omitted_instances: number;
  steps_consumed: number;
  steps_max: number;
  agents_consumed: number;
  candidate?: CandidateReference | null;
  /**
   * Admitted candidate users, including serialized workspace waiters.
   * While nonzero no historical check certifies the mutable workspace.
   */
  candidate_users: number;
  handoff?: WorkflowHandoff | null;
}
export interface WorkflowInstanceView {
  block: WorkflowBlockInstance;
  node?: string | null;
  visit?: number | null;
  kind: WorkflowNodeKind;
  state: WorkflowState;
  child?: SubagentId | null;
  invocation?: ToolInvocationId | null;
  tool_id?: ToolId | null;
  interaction?: InteractionRef | null;
  iteration?: number | null;
  iterations_max?: number | null;
  loop_exit?: WorkflowLoopExit | null;
  candidate?: CandidateReference | null;
  checks_passed?: boolean | null;
  review_accepted?: boolean | null;
}
export interface WorkflowHandoff {
  state: string;
  path: string;
  truncated: boolean;
}
/**
 * The client-visible durable-authority failure state of a conversation
 * runtime (Issue #63).
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
 * One bounded newest-or-older page of derived transcript history.
 */
export interface RuntimeClientTranscriptPage1 {
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
 * The external attempt view of the Runtime Client projection.
 *
 * The view folds attempt lifecycle, turn progress, in-flight agent
 * output, and foreground tool execution into one structured read model.
 */
export interface RuntimeClientAttempt {
  /**
   * Identifies one attempt to execute an agent manifest.
   */
  attempt_id: string;
  /**
   * The externally meaningful attempt phase.
   */
  phase:
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
               * The normalized client-visible failure.
               */
              error:
                | {
                    /**
                     * The normalized model error kind.
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
                    retry_after_ms?: number | null;
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
 * Normalized token accounting for one generation.
 *
 * Providers do not expose identical token metrics; this is the stable
 * common core. Provider SDK usage objects never appear here.
 */
export interface ModelUsage {
  /**
   * Input tokens consumed by the request.
   */
  input_tokens: number;
  /**
   * Output tokens produced by the response.
   */
  output_tokens: number;
  /**
   * Total tokens, where the provider reports or can derive them.
   */
  total_tokens: number;
  /**
   * Optional normalized usage details.
   */
  details?: UsageDetails | null;
}
/**
 * Optional normalized token details where providers expose them.
 */
export interface UsageDetails {
  /**
   * Tokens consumed by reasoning.
   */
  reasoning_tokens?: number | null;
  /**
   * Input tokens served from cache.
   */
  cached_input_tokens?: number | null;
}
/**
 * The accumulated in-flight output of one streaming Assistant message.
 *
 * This is the repair state of streaming: a snapshot taken mid-stream
 * carries every accumulated delta through its cursor, so a client
 * repairing after `resync` reconstructs the exact message it would have
 * observed incrementally.
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
 * The foreground tool execution read model of one logical tool call.
 *
 * Keyed by the canonical logical tool-call identity, so parallel physical
 * completion timing can never corrupt logical identities or canonical
 * ordering. Native, MCP, and Python foreground executions converge through
 * this one shape.
 */
export interface ForegroundToolExecution {
  /**
   * Identifies one tool call issued by the current agent.
   */
  call_id: string;
  /**
   * Identifies a tool definition in the capability set.
   */
  tool_id: string;
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
        result: ToolExecutionResult2;
        type: 'settled';
      };
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
  duration_ms: number;
  /**
   * Process exit code where the tool executed a process.
   */
  exit_code?: number | null;
  /**
   * Durable artifact/file references produced by the execution.
   */
  artifacts?: FileReference[];
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
 * The redacted client-facing projection of one attempt's frozen model
 * snapshot.
 *
 * This is what makes "session desired model = B, running attempt model = A"
 * unambiguous without a client inferring anything from event ordering.
 */
export interface AttemptModelView {
  primary: ModelInvocationView1;
  /**
   * The attempt's frozen summary policy.
   */
  summary:
    | {
        mode: 'session';
      }
    | {
        /**
         * A fully qualified catalog model reference: `provider-id/model-id`.
         *
         * The first `/` separates the provider from the model. The model ID itself
         * may contain additional `/` characters, as is common for Hugging Face
         * identities such as `Qwen/Qwen3`, but no model-ID segment may be empty.
         *
         * This is the explicit model-identity domain of the runtime. Concatenated
         * strings never travel through the runtime in its place: a reference either
         * resolves to exactly one catalog model or it fails.
         */
        model: string;
        /**
         * The protocol of the binding.
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
         * The selected reasoning profile, when the model declares any.
         */
        reasoningProfile?: ReasoningProfileId | null;
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
export interface ModelInvocationView1 {
  /**
   * A fully qualified catalog model reference: `provider-id/model-id`.
   *
   * The first `/` separates the provider from the model. The model ID itself
   * may contain additional `/` characters, as is common for Hugging Face
   * identities such as `Qwen/Qwen3`, but no model-ID segment may be empty.
   *
   * This is the explicit model-identity domain of the runtime. Concatenated
   * strings never travel through the runtime in its place: a reference either
   * resolves to exactly one catalog model or it fails.
   */
  model: string;
  /**
   * The protocol of the binding.
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
   * The selected reasoning profile, when the model declares any.
   */
  reasoningProfile?: ReasoningProfileId | null;
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
 * Facts frozen together with the attempt model under native admission.
 */
export interface AdmittedSettings {
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
 */
export interface InboundItemView {
  /**
   * The mailbox-assigned inbound sequence.
   */
  sequence: string;
  message: UserMessageBlock2;
}
/**
 * Inbound information supplied to the current agent.
 *
 * A `UserMessageBlock` does not necessarily mean a human spoke: it is the
 * canonical home for anything inbound, including messages from other agents
 * (with [`UserSource::Agent`] provenance) and runtime compaction summaries
 * (with [`InboundKind::CompactionSummary`] kind). It
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
           * Identity of the sending agent.
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
           * The rustX-derived logical extension identity.
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
        compaction_summary: CompactionSummaryMetadata;
      }
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
   * no wall-clock time is fabricated. Derived M4 compaction summaries
   * ([`InboundKind::CompactionSummary`]) never carry one. Older or
   * derived messages without a timestamp remain representable: the field
   * defaults to `None` on deserialization and is omitted from the
   * canonical encoding while absent.
   */
  timestamp?: string | null;
}
/**
 * The latest observed finite drain boundary.
 */
export interface InboundDrainView {
  /**
   * The highest selected inbound sequence.
   */
  watermark: string;
  /**
   * The number of drained items.
   */
  count: number;
}
/**
 * One pending interaction projected to the root Runtime Client.
 *
 * The request remains the originating conversation's immutable request. The
 * routed address and source only make that request understandable and
 * answerable at the shared human-facing surface.
 */
export interface RoutedInteraction {
  interaction: InteractionRef1;
  /**
   * Root-facing source metadata.
   */
  source:
    | {
        type: 'primary';
      }
    | {
        /**
         * Identifies one conversation-owned asynchronous one-shot subagent
         * (Issue #60).
         *
         * `SubagentId` is the logical lifecycle/delegation identity of a child
         * rustX runtime. It is deliberately not an OS pid: a pid is ephemeral
         * process state and is never durable identity, and pid reuse after a
         * restart can never prove that a surviving process is the previously
         * owned child.
         */
        subagent_id: string;
        /**
         * Identifies a durable conversation.
         */
        child_conversation_id: string;
        /**
         * The frozen named definition used by the child.
         */
        agent_name: string;
        type: 'subagent';
      };
  request: InteractionRequest;
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
   * Identifies a durable conversation.
   */
  conversation_id: string;
  /**
   * The interaction identity allocated by that conversation's coordinator.
   */
  interaction_id: string;
}
/**
 * The originating conversation's request facts.
 */
export interface InteractionRequest {
  /**
   * The non-reused runtime-owned interaction identity.
   */
  id: string;
  /**
   * Identifies a durable conversation.
   */
  conversation_id: string;
  /**
   * Identifies one attempt to execute an agent manifest.
   */
  attempt_id: string;
  /**
   * The primary model turn that reached the policy boundary.
   */
  turn: number;
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
         * The registry-resolved execution mode.
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
   * The canonical registry-resolved tool identity.
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
   * Identifies a committed canonical message block.
   */
  status_message_id: string;
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
  fresh_inbound?: FreshInboundStatusOpportunityView | null;
  /**
   * The complete settled tool batch that made this existing primary step
   * eligible, when present.
   */
  post_tool_batch?: PostToolBatchStatusOpportunityView | null;
}
/**
 * The external view of one `FreshInbound` status opportunity.
 */
export interface FreshInboundStatusOpportunityView {
  /**
   * Identifies a committed canonical message block.
   */
  target_message_id: string;
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
 */
export interface PostToolBatchStatusOpportunityView {
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
 * One bounded Todo task in the Agent Status client view.
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
 * The context diagnostics carried by the Runtime Client snapshot.
 */
export interface RuntimeClientContextView1 {
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
 */
export interface CapabilityView1 {
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
 * The active runtime resource generation: the project context files
 * the runtime actually loaded, and whether an agent profile is frozen
 * into the generation.
 *
 * This is deliberately separate from [`CapabilityView`]: a
 * resource-only reload advances the resource revision while the
 * capability revision stays put. Nothing here is conversation content
 * — a project context file is request input the runtime assembles into
 * the Effective System Prompt, never a canonical Ledger message.
 */
export interface RuntimeClientResourcesView {
  inspection: CapabilityInspection;
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
 * Facts copied from the same immutable generation as this revision.
 */
export interface CapabilityInspection {
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
export interface AgentInspection {
  identity: AgentIdentity;
  source?: string | null;
  tools: ToolInspection[];
  tool_selection: AgentToolSelection[];
  skills: SkillProvenance[];
  disabled_skills: string[];
  agents: SubagentName[];
  workflows: WorkflowId[];
  extensions: ExtensionInspection[];
  diagnostics: AgentProfileDiagnostic[];
}
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
 */
export interface SkillProvenance {
  /**
   * The effective logical Skill identity.
   */
  name: string;
  /**
   * The source that owns the effective package.
   */
  source: 'global' | 'workspace' | 'explicit';
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
 */
export interface ShadowedSkill {
  /**
   * The source whose package was shadowed.
   */
  source: 'global' | 'workspace' | 'explicit';
  /**
   * The shadowed package's `SKILL.md` location.
   */
  location: string;
}
export interface ExtensionInspection {
  identity: NativeExtension;
  active: boolean;
}
export interface WorkflowAdmissionDiagnostic {
  path: string;
  reason: WorkflowDependencyFailure;
}
/**
 * One runtime-loaded project instruction file.
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
 * The complete state of one conversation's list.
 *
 * This is the persistence format: it is what a successful mutation
 * publishes, and it is what [`ConversationTodoList::rebuilt`] reads back.
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
export interface Failure {
  jsonrpc: JsonRpcVersion;
  id?: RequestId | null;
  error: RpcError;
}
export interface RpcError {
  code: number;
  message: string;
  data?: ErrorData | null;
}
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
 */
export interface ModelUsage1 {
  /**
   * Input tokens consumed by the request.
   */
  input_tokens: number;
  /**
   * Output tokens produced by the response.
   */
  output_tokens: number;
  /**
   * Total tokens, where the provider reports or can derive them.
   */
  total_tokens: number;
  /**
   * Optional normalized usage details.
   */
  details?: UsageDetails | null;
}
/**
 * One pending interaction projected to the root Runtime Client.
 *
 * The request remains the originating conversation's immutable request. The
 * routed address and source only make that request understandable and
 * answerable at the shared human-facing surface.
 */
export interface RoutedInteraction1 {
  interaction: InteractionRef1;
  /**
   * Root-facing source metadata.
   */
  source:
    | {
        type: 'primary';
      }
    | {
        /**
         * Identifies one conversation-owned asynchronous one-shot subagent
         * (Issue #60).
         *
         * `SubagentId` is the logical lifecycle/delegation identity of a child
         * rustX runtime. It is deliberately not an OS pid: a pid is ephemeral
         * process state and is never durable identity, and pid reuse after a
         * restart can never prove that a surviving process is the previously
         * owned child.
         */
        subagent_id: string;
        /**
         * Identifies a durable conversation.
         */
        child_conversation_id: string;
        /**
         * The frozen named definition used by the child.
         */
        agent_name: string;
        type: 'subagent';
      };
  request: InteractionRequest;
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
   * Identifies a durable conversation.
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
export interface InteractionRef3 {
  /**
   * Identifies a durable conversation.
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
   * Durable Event Journal event identity.
   */
  event_id: string;
  /**
   * Event timestamp.
   */
  timestamp: string;
  /**
   * Identifies one attempt to execute an agent manifest.
   */
  attempt_id: string;
  /**
   * Owning turn.
   */
  turn_id: string;
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
         * Caller-neutral invocation correlation.
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
         * The registry-resolved tool identity.
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
   * Durable Event Journal event identity.
   */
  event_id: string;
  /**
   * Event timestamp.
   */
  timestamp: string;
  /**
   * Identifies one attempt to execute an agent manifest.
   */
  attempt_id: string;
  /**
   * Owning turn.
   */
  turn_id: string;
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
        type: 'review_invalidated';
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
 * The context diagnostics carried by the Runtime Client snapshot.
 */
export interface RuntimeClientContextView2 {
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
export interface ToolCallStart {
  /**
   * Identifies one tool call issued by the current agent.
   */
  id: string;
  /**
   * Identifies a tool definition in the capability set.
   */
  tool_id: string;
  /**
   * Name of the tool at call time.
   */
  name: string;
}
/**
 * One tool call issued by the current agent.
 */
export interface ToolCall1 {
  /**
   * Identity of this specific call, referenced by the matching
   * `ToolMessageBlock` and tool-result events.
   */
  id: string;
  /**
   * Identity of the tool being called within the capability set.
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
 * The bounded immutable audit of the settled stream.
 */
export interface PublicationAudit1 {
  /**
   * The settled publication stream.
   */
  stream_id: string;
  /**
   * Identifies one attempt to execute an agent manifest.
   */
  attempt_id: string;
  /**
   * The turn that owned the stream.
   */
  turn_id: string;
  /**
   * The provider request the stream published.
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
  duration_ms: number;
  /**
   * Process exit code where the tool executed a process.
   */
  exit_code?: number | null;
  /**
   * Durable artifact/file references produced by the execution.
   */
  artifacts?: FileReference[];
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
   * Identifies a committed canonical message block.
   */
  status_message_id: string;
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
 * (with [`UserSource::Agent`] provenance) and runtime compaction summaries
 * (with [`InboundKind::CompactionSummary`] kind). It
 * must never become `AssistantMessageBlock` or `ToolMessageBlock`, which are
 * reserved for output and actions of the current agent.
 */
export interface UserMessageBlock3 {
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
           * Identity of the sending agent.
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
           * The rustX-derived logical extension identity.
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
        compaction_summary: CompactionSummaryMetadata;
      }
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
   * no wall-clock time is fabricated. Derived M4 compaction summaries
   * ([`InboundKind::CompactionSummary`]) never carry one. Older or
   * derived messages without a timestamp remain representable: the field
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
export interface RuntimeClientBackgroundExecution1 {
  /**
   * Identifies one detached runtime execution instance of a background
   * tool.
   *
   * `ToolExecutionId` is distinct from `ToolCallId`: a `ToolCallId`
   * identifies the logical model-issued call, while a `ToolExecutionId`
   * identifies the runtime execution instance and may outlive the
   * attempt that created it. Allocation is conversation-owned and
   * monotonic (`exec_1`, `exec_2`, ...); cross-conversation uniqueness is
   * not required because the background registry is conversation-scoped.
   */
  execution_id: string;
  /**
   * The canonical tool identity.
   */
  tool_id: string;
  /**
   * The model-facing tool name.
   */
  tool_name: string;
  /**
   * The authoritative lifecycle state.
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
  result?: ToolExecutionResult1 | null;
}
/**
 * The Runtime Client view of one subagent child (Issue #60).
 *
 * A read-model materialization of the authoritative registry snapshot:
 * every field is derived, and the durable ownership/terminal events —
 * never this view — are the recovery authority.
 *
 * Since Issue #178 the view also carries the child's live activity
 * projection (`observation`), its redacted execution profile
 * (`execution_profile`), and its start time (`started_at`). These are
 * observation-plane facts: the lifecycle `state` remains the only
 * authority on whether the child is alive, settling, or settled.
 */
export interface RuntimeClientSubagent1 {
  /**
   * Identifies one conversation-owned asynchronous one-shot subagent
   * (Issue #60).
   *
   * `SubagentId` is the logical lifecycle/delegation identity of a child
   * rustX runtime. It is deliberately not an OS pid: a pid is ephemeral
   * process state and is never durable identity, and pid reuse after a
   * restart can never prove that a surviving process is the previously
   * owned child.
   */
  subagent_id: string;
  /**
   * The child agent identity (the provenance its answer carries).
   */
  child_agent_id: string;
  /**
   * Identifies a durable conversation.
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
   * actually started with, so a later resource reload that redefines the
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
  state:
    | 'running'
    | 'cancelling'
    | 'publishing_terminal'
    | 'succeeded'
    | 'failed'
    | 'cancelled'
    | 'interrupted';
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
  workspace: RuntimeClientSubagentWorkspace;
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
 */
export interface CapabilityView2 {
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
 * The deterministic capability projection.
 *
 * Projected from the active [`CapabilitySnapshot`]
 * ([`crate::capabilities::CapabilitySnapshot`]) plus the
 * coordinator-owned availability state (Issue #81): the revision, the
 * active Tool catalog, the complete available Tool catalog, the deterministic
 * model-visible Skill catalog, and the typed per-source availability. No
 * executors, environment paths, package-manager state, or private dependency
 * internals appear.
 */
export interface CapabilityView3 {
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
 * The active runtime resource projection after the reload.
 */
export interface RuntimeClientResourcesView1 {
  inspection: CapabilityInspection;
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
 * The redacted client-facing projection of the session model state.
 */
export interface SessionModelView1 {
  configured: SessionModelConfig1;
  effective: ModelInvocationView;
  /**
   * The resolved summary policy.
   */
  summary:
    | {
        mode: 'session';
      }
    | {
        /**
         * A fully qualified catalog model reference: `provider-id/model-id`.
         *
         * The first `/` separates the provider from the model. The model ID itself
         * may contain additional `/` characters, as is common for Hugging Face
         * identities such as `Qwen/Qwen3`, but no model-ID segment may be empty.
         *
         * This is the explicit model-identity domain of the runtime. Concatenated
         * strings never travel through the runtime in its place: a reference either
         * resolves to exactly one catalog model or it fails.
         */
        model: string;
        /**
         * The protocol of the binding.
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
         * The selected reasoning profile, when the model declares any.
         */
        reasoningProfile?: ReasoningProfileId | null;
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
