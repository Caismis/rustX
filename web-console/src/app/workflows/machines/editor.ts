import { assign, fromPromise, setup, type ActorRefFrom, type SnapshotFrom } from 'xstate';
import type { WorkflowCheckReport, WorkflowSource } from '../../../../../protocol/app-server/v51';
import type { WorkflowCommit } from '../../../workspaces/host';
import { CommandRejected, type RejectionCode } from '../document/commands';
import { existingPath, sameDocument, ROOT, type BlockPath, type WorkflowDocument } from '../document/model';
import { classifyFailure, type WorkflowFailure, type WorkflowScopePort, type WorkflowTarget } from '../port';

const HISTORY_LIMIT = 100;

/** One correlated native check of exactly one candidate generation. */
export interface CheckResult {
  readonly generation: number;
  readonly report: WorkflowCheckReport;
  readonly explain: boolean;
  /** The Workspace registration a Global file's dependencies were checked in. */
  readonly context?: string;
}

/** The last committed save: its file revision and the generation it wrote. */
export interface SaveReceipt {
  readonly revision: string;
  readonly configurationRevision: string | null;
  readonly generation: number;
  /** Whether an authoritative reread observed exactly this commit. */
  readonly confirmed: boolean;
}

/** Facts reported to the author. Each is distinct; none is a green light. */
export type EditorNotice =
  | { kind: 'failure'; operation: 'check' | 'save' | 'reconcile'; failure: WorkflowFailure }
  /** Saved, but the reread failed: committed and unconfirmed. Never "unsaved". */
  | { kind: 'committed_unconfirmed' }
  /** Saved, and the reread found a different revision: the file changed again. */
  | { kind: 'changed_after_save'; revision: string }
  /** Reconciliation found the uncertain save did not commit. */
  | { kind: 'not_committed' }
  /** Reconciliation found the uncertain save committed. */
  | { kind: 'reconciled' }
  /** Reconciliation found a revision this editor did not write. */
  | { kind: 'changed_externally'; revision: string };

export interface EditorContext {
  readonly port: WorkflowScopePort;
  readonly target: WorkflowTarget;
  /** The last authoritative observation of the file and its document. */
  base?: { revision: string; source: WorkflowSource; document?: WorkflowDocument };
  /** The one working document; absent while loading or for an unreadable file. */
  document?: WorkflowDocument;
  /** Increments on every semantic change of `document`. */
  generation: number;
  past: WorkflowDocument[];
  future: WorkflowDocument[];
  /** The coalescing key of the latest edit, so one field session is one undo step. */
  coalesce?: string;
  block: BlockPath;
  check?: CheckResult;
  /** The input of the latest check; in flight only while `ready.checking`. */
  checking?: { generation: number; document: WorkflowDocument; explain: boolean; context?: string };
  saving?: { generation: number; document: WorkflowDocument };
  /** Set by an uncertain save; a new save requires reconciliation first. */
  uncertain?: { generation: number; document: WorkflowDocument };
  receipt?: SaveReceipt;
  notice?: EditorNotice;
  rejection?: RejectionCode;
  loadFailure?: WorkflowFailure;
}

export type EditorEvent =
  | { type: 'EDIT'; apply: (document: WorkflowDocument) => WorkflowDocument; coalesce?: string }
  | { type: 'UNDO' } | { type: 'REDO' }
  | { type: 'NAVIGATE'; block: BlockPath }
  | { type: 'CHECK'; explain: boolean; context?: string }
  | { type: 'SAVE' }
  | { type: 'RECONCILE' }
  | { type: 'RELOAD' }
  | { type: 'DISMISS' };

export interface EditorInput { readonly port: WorkflowScopePort; readonly target: WorkflowTarget }

export function isDirty(context: Pick<EditorContext, 'document' | 'base'>): boolean {
  return !!context.document && !!context.base?.document && !sameDocument(context.document, context.base.document);
}

function failure(event: unknown, write: boolean) {
  return classifyFailure((event as { error: unknown }).error, write);
}

function committed(context: EditorContext, saving: NonNullable<EditorContext['saving']>, commit: WorkflowCommit): Partial<EditorContext> {
  const { revision, configuration_revision: configurationRevision } = commit.acknowledgement;
  const reread = commit.reread;
  const confirmed = reread.status === 'observed' && reread.source.revision === revision;
  const notice: EditorNotice | undefined = reread.status === 'failed' ? { kind: 'committed_unconfirmed' }
    : reread.source.revision !== revision ? { kind: 'changed_after_save', revision: reread.source.revision } : undefined;
  return {
    // The commit's own revision is the next CAS base; the working document
    // is never replaced by a reread, so later edits survive.
    base: { revision, source: confirmed && reread.status === 'observed' ? reread.source : { ...context.base!.source, revision, present: true }, document: saving.document },
    receipt: { revision, configurationRevision, generation: saving.generation, confirmed },
    saving: undefined, uncertain: undefined, notice,
  };
}

/** One editing transaction of one exact Workflow file. React subscribes and
 * submits intent; this machine alone arbitrates the file target, the base
 * revision, edit generations and correlated native operations. */
export const workflowEditorMachine = setup({
  types: { context: {} as EditorContext, events: {} as EditorEvent, input: {} as EditorInput },
  actors: {
    read: fromPromise<WorkflowSource, { port: WorkflowScopePort; id: string }>(({ input }) => input.port.read(input.id)),
    check: fromPromise<WorkflowCheckReport, { port: WorkflowScopePort; id: string; document: WorkflowDocument; explain: boolean; context?: string }>(
      ({ input }) => input.port.check(input.id, input.document, input.explain, input.context)),
    save: fromPromise<WorkflowCommit, { port: WorkflowScopePort; id: string; revision: string; document: WorkflowDocument }>(
      ({ input }) => input.port.write(input.id, input.revision, { operation: 'replace', document: input.document })),
  },
  guards: {
    editable: ({ context }) => !!context.document && !!context.base?.document,
    savable: ({ context }) => !!context.document && !!context.base?.document && !context.uncertain,
    canUndo: ({ context }) => context.past.length > 0,
    canRedo: ({ context }) => context.future.length > 0,
  },
  actions: {
    edit: assign(({ context, event }) => {
      if (event.type !== 'EDIT' || !context.document) return {};
      let next: WorkflowDocument;
      try { next = event.apply(context.document); }
      catch (cause) {
        if (cause instanceof CommandRejected) return { rejection: cause.code };
        throw cause;
      }
      if (next === context.document) return { rejection: undefined };
      const coalesced = event.coalesce !== undefined && event.coalesce === context.coalesce;
      return {
        document: next, generation: context.generation + 1, rejection: undefined,
        past: coalesced ? context.past : [...context.past, context.document].slice(-HISTORY_LIMIT),
        future: [], coalesce: event.coalesce, block: existingPath(next, context.block),
      };
    }),
    undo: assign(({ context }) => {
      const document = context.past.at(-1)!;
      return { document, past: context.past.slice(0, -1), future: [context.document!, ...context.future], generation: context.generation + 1, coalesce: undefined, rejection: undefined, block: existingPath(document, context.block) };
    }),
    redo: assign(({ context }) => {
      const [document, ...future] = context.future;
      return { document, past: [...context.past, context.document!], future, generation: context.generation + 1, coalesce: undefined, rejection: undefined, block: existingPath(document, context.block) };
    }),
    navigate: assign(({ context, event }) => event.type === 'NAVIGATE' && context.document ? { block: existingPath(context.document, event.block) } : {}),
    dismiss: assign({ notice: undefined, rejection: undefined }),
  },
}).createMachine({
  id: 'workflowEditor',
  context: ({ input }) => ({ port: input.port, target: input.target, generation: 0, past: [], future: [], block: ROOT }),
  initial: 'loading',
  states: {
    loading: {
      entry: assign({ loadFailure: undefined, notice: undefined }),
      invoke: {
        src: 'read', input: ({ context }) => ({ port: context.port, id: context.target.id }),
        onDone: { target: 'ready', actions: assign(({ context, event }) => {
          const document = event.output.document ?? undefined;
          return { base: { revision: event.output.revision, source: event.output, document }, document, generation: context.generation + 1, past: [], future: [], coalesce: undefined, block: ROOT, check: undefined, receipt: undefined, uncertain: undefined };
        }) },
        onError: { target: 'failed', actions: assign(({ event }) => ({ loadFailure: failure(event, false) })) },
      },
    },
    failed: { on: { RELOAD: 'loading' } },
    ready: {
      initial: 'idle',
      on: {
        EDIT: { guard: 'editable', actions: 'edit' },
        UNDO: { guard: 'canUndo', actions: 'undo' },
        REDO: { guard: 'canRedo', actions: 'redo' },
        NAVIGATE: { actions: 'navigate' },
        DISMISS: { actions: 'dismiss' },
      },
      states: {
        idle: {
          on: {
            CHECK: { guard: 'editable', target: 'checking' },
            SAVE: { guard: 'savable', target: 'saving' },
            RECONCILE: { guard: ({ context }) => !!context.uncertain, target: 'reconciling' },
            RELOAD: '#workflowEditor.loading',
          },
        },
        checking: {
          // A newer check supersedes the older one by stopping it, so the
          // older result has no completion path at all.
          // `checking` is the exact input of the current check. It stays set
          // through the transition that consumes its result.
          entry: assign(({ context, event }) => ({ checking: { generation: context.generation, document: context.document!, explain: event.type === 'CHECK' && event.explain, context: event.type === 'CHECK' ? event.context : undefined }, notice: undefined })),
          invoke: {
            src: 'check',
            input: ({ context }) => ({ port: context.port, id: context.target.id, document: context.checking!.document, explain: context.checking!.explain, context: context.checking!.context }),
            // The report names the generation it checked. A later edit never
            // inherits it: presentation compares it with the current one.
            onDone: { target: 'idle', actions: assign(({ context, event }) => ({ check: { generation: context.checking!.generation, report: event.output, explain: context.checking!.explain, context: context.checking!.context } })) },
            onError: { target: 'idle', actions: assign(({ event }) => ({ notice: { kind: 'failure', operation: 'check', failure: failure(event, false) } as EditorNotice })) },
          },
          on: {
            CHECK: { guard: 'editable', target: 'checking', reenter: true },
            SAVE: { guard: 'savable', target: 'saving' },
          },
        },
        saving: {
          entry: assign(({ context }) => ({ saving: { generation: context.generation, document: context.document! }, notice: undefined })),
          invoke: {
            src: 'save',
            input: ({ context }) => ({ port: context.port, id: context.target.id, revision: context.base!.revision, document: context.saving!.document }),
            onDone: { target: 'idle', actions: assign(({ context, event }) => committed(context, context.saving!, event.output)) },
            onError: [
              {
                guard: ({ event }) => failure(event, true).kind === 'uncertain', target: 'idle',
                actions: assign(({ context, event }) => ({ uncertain: context.saving, saving: undefined, notice: { kind: 'failure', operation: 'save', failure: failure(event, true) } as EditorNotice })),
              },
              { target: 'idle', actions: assign(({ event }) => ({ saving: undefined, notice: { kind: 'failure', operation: 'save', failure: failure(event, true) } as EditorNotice })) },
            ],
          },
        },
        reconciling: {
          // Reconciliation is a read. It never resends the uncertain write.
          invoke: {
            src: 'read', input: ({ context }) => ({ port: context.port, id: context.target.id }),
            onDone: { target: 'idle', actions: assign(({ context, event }) => {
              const source = event.output, attempt = context.uncertain!;
              if (source.revision === context.base!.revision) return { uncertain: undefined, notice: { kind: 'not_committed' } as EditorNotice };
              if (source.document && sameDocument(source.document, attempt.document)) {
                return { uncertain: undefined, base: { revision: source.revision, source, document: attempt.document }, receipt: { revision: source.revision, configurationRevision: null, generation: attempt.generation, confirmed: true }, notice: { kind: 'reconciled' } as EditorNotice };
              }
              return { uncertain: undefined, notice: { kind: 'changed_externally', revision: source.revision } as EditorNotice };
            }) },
            onError: { target: 'idle', actions: assign(({ event }) => ({ notice: { kind: 'failure', operation: 'reconcile', failure: failure(event, false) } as EditorNotice })) },
          },
        },
      },
    },
  },
});

export type WorkflowEditorActor = ActorRefFrom<typeof workflowEditorMachine>;
export type WorkflowEditorSnapshot = SnapshotFrom<typeof workflowEditorMachine>;
