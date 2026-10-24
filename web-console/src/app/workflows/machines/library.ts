import { assign, enqueueActions, fromPromise, sendParent, setup, type ActorRefFrom, type SnapshotFrom } from 'xstate';
import type { WorkflowEdit, WorkflowFileSummary, WorkflowLibrary } from '../../../../../protocol/app-server/v57';
import type { WorkflowCommit } from '../../../workspaces/host';
import { classifyFailure, sameScope, type WorkflowFailure, type WorkflowScope, type WorkflowScopePort } from '../port';

/** One accepted listing and the exact port that produced it. Every action
 * started from what the library shows uses this pair, never the listing of
 * one read with the port of another. */
export type LibraryObservation = Readonly<{ port: WorkflowScopePort; listing: WorkflowLibrary }>;

/** One file operation of the library, bound to the exact scope it started in. */
export type LibraryOperation =
  | { kind: 'create'; id: string; description: string }
  | { kind: 'copy'; source: string; sourceRevision: string; id: string }
  | { kind: 'delete'; id: string; revision: string };

/** An open confirmation. Its target is captured from one observation when
 * it opens, so what it shows is exactly what it changes. */
export interface LibraryConfirmation {
  readonly seq: number;
  readonly kind: LibraryOperation['kind'];
  readonly observation: LibraryObservation;
  /** The source (copy) or target (delete) file, from that observation. */
  readonly file?: WorkflowFileSummary;
  /** Why its last submission failed; it stays open to show it. */
  readonly failure?: WorkflowFailure;
  /** An unknown result belongs to this exact dispatched transaction. Reads
   * may observe the file, but this confirmation can never submit it again. */
  readonly uncertain?: PendingOperation;
}

/** A dispatched operation. Its target is fixed from dispatch to outcome. */
export interface PendingOperation {
  readonly operation: LibraryOperation;
  readonly port: WorkflowScopePort;
  readonly expected: string;
  /** The confirmation that submitted it. */
  readonly confirmation: number;
}

export interface LibraryContext {
  readonly ports: (scope: WorkflowScope) => WorkflowScopePort;
  scope: WorkflowScope;
  /** The port of the read in flight; a newer read replaces it. */
  reading?: WorkflowScopePort;
  /** The accepted listing of exactly `scope`. It stays visible while a
   * refresh is in flight, and is never another scope's. */
  observation?: LibraryObservation;
  listFailure?: WorkflowFailure;
  confirmation?: LibraryConfirmation;
  confirmations: number;
  pending?: PendingOperation;
  /** A scope chosen while an operation was in flight, selected once its
   * outcome arrived. */
  deferredScope?: WorkflowScope;
  operationFailure?: { operation: LibraryOperation; failure: WorkflowFailure };
}

export type LibraryEvent =
  | { type: 'SCOPE'; scope: WorkflowScope }
  | { type: 'REFRESH' }
  /** The library became visible: list now. Nothing is read before. */
  | { type: 'ACTIVATE' }
  /** Open a confirmation from the accepted observation, only while no
   * read replaces it. `file` names a listed file by its file name. */
  | { type: 'CONFIRM'; kind: LibraryOperation['kind']; file?: string }
  /** Submit confirmation `seq` with the author's form values. */
  | { type: 'SUBMIT'; seq: number; id?: string; description?: string }
  | { type: 'CANCEL'; seq: number }
  /** Open a listed file through the port that listed it. */
  | { type: 'OPEN'; file: string }
  | { type: 'DISMISS' };

/** Sent to the owner after a create committed, naming the exact new file
 * and the port that created it. */
export interface LibraryCreated { type: 'LIBRARY.CREATED'; scope: WorkflowScope; id: string; port: WorkflowScopePort }
/** Sent to the owner to open one listed file through its listing's port. */
export interface LibraryOpen { type: 'OPEN'; id: string; port: WorkflowScopePort }

function edit(operation: LibraryOperation): WorkflowEdit {
  switch (operation.kind) {
    case 'create': return { operation: 'create', description: operation.description };
    case 'copy': return { operation: 'copy', source: operation.source, source_revision: operation.sourceRevision };
    case 'delete': return { operation: 'delete' };
  }
}

function listed(observation: LibraryObservation, name: string | undefined): WorkflowFileSummary | undefined {
  return name === undefined ? undefined : observation.listing.files.find(file => file.file_name === name);
}

/** The operation a confirmation submits: its port, source file and
 * expected revision all come from its own observation. */
function operationOf(confirmation: LibraryConfirmation, event: Extract<LibraryEvent, { type: 'SUBMIT' }>): PendingOperation | undefined {
  const { observation, file } = confirmation;
  const absent = observation.listing.absent_revision;
  const bound = (operation: LibraryOperation, expected: string): PendingOperation => ({ operation, port: observation.port, expected, confirmation: confirmation.seq });
  const description = event.description?.trim();
  switch (confirmation.kind) {
    case 'create': return event.id && description ? bound({ kind: 'create', id: event.id, description }, absent) : undefined;
    case 'copy': return file?.id && event.id ? bound({ kind: 'copy', source: file.id, sourceRevision: file.revision, id: event.id }, absent) : undefined;
    case 'delete': return file?.id && file.revision ? bound({ kind: 'delete', id: file.id, revision: file.revision }, file.revision) : undefined;
  }
}

/** The library after an operation settled: a scope chosen meanwhile is
 * selected now. */
function settled(context: LibraryContext): Partial<LibraryContext> {
  return { pending: undefined, deferredScope: undefined, ...(context.deferredScope ? { scope: context.deferredScope, observation: undefined, confirmation: undefined } : {}) };
}

/** Level 1: the Workflow files physically owned by one selected scope.
 * Each read runs on a port created for it and returns that port with its
 * listing; selecting a scope or refreshing re-enters `loading`, which stops
 * the older read, so its result has no completion path. */
export const workflowLibraryMachine = setup({
  types: { context: {} as LibraryContext, events: {} as LibraryEvent, input: {} as { ports: (scope: WorkflowScope) => WorkflowScopePort; scope: WorkflowScope } },
  actors: {
    list: fromPromise<LibraryObservation, { port: WorkflowScopePort }>(async ({ input: { port } }) => ({ port, listing: await port.list() })),
    operate: fromPromise<WorkflowCommit, PendingOperation>(({ input }) => input.port.write(input.operation.id, input.expected, edit(input.operation))),
  },
  guards: {
    otherScope: ({ context, event }) => event.type === 'SCOPE' && !sameScope(context.scope, event.scope),
    submittable: ({ context, event }) => event.type === 'SUBMIT' && context.confirmation?.seq === event.seq && !context.confirmation.uncertain && !!operationOf(context.confirmation, event),
  },
  actions: {
    selectScope: assign(({ event }) => event.type === 'SCOPE' ? { scope: event.scope, observation: undefined, operationFailure: undefined } : {}),
    confirm: assign(({ context, event }) => {
      if (event.type !== 'CONFIRM' || !context.observation || context.confirmation) return {};
      const file = listed(context.observation, event.file);
      if (event.kind !== 'create' && !file?.id) return {};
      return { confirmation: { seq: context.confirmations + 1, kind: event.kind, observation: context.observation, file }, confirmations: context.confirmations + 1 };
    }),
    submit: assign(({ context, event }) => event.type === 'SUBMIT' && context.confirmation
      ? { pending: operationOf(context.confirmation, event), confirmation: { ...context.confirmation, failure: undefined }, operationFailure: undefined } : {}),
    cancel: assign(({ context, event }) => event.type === 'CANCEL' && context.confirmation?.seq === event.seq && context.pending?.confirmation !== event.seq ? { confirmation: undefined } : {}),
    open: enqueueActions(({ context, event, enqueue }) => {
      const file = event.type === 'OPEN' && context.observation ? listed(context.observation, event.file) : undefined;
      if (file?.id && file.revision) enqueue.sendParent({ type: 'OPEN', id: file.id, port: context.observation!.port } satisfies LibraryOpen);
    }),
  },
}).createMachine({
  id: 'workflowLibrary',
  context: ({ input }) => ({ ports: input.ports, scope: input.scope, confirmations: 0 }),
  initial: 'dormant',
  on: {
    SCOPE: { guard: 'otherScope', target: '.loading', actions: 'selectScope' },
    CANCEL: { actions: 'cancel' },
    DISMISS: { actions: assign({ operationFailure: undefined }) },
  },
  states: {
    // A listing always starts from a port created for it, so it reads the
    // authorities current when it starts, never ones from App startup.
    dormant: { on: { ACTIVATE: 'loading', REFRESH: 'loading' } },
    loading: {
      entry: assign(({ context }) => ({ reading: context.ports(context.scope), listFailure: undefined })),
      invoke: {
        src: 'list', input: ({ context }) => ({ port: context.reading! }),
        onDone: {
          target: 'ready',
          // The observation is the read's own port and listing, accepted whole.
          guard: ({ context, event }) => event.output.port === context.reading,
          actions: assign(({ event }) => ({ observation: event.output, reading: undefined })),
        },
        onError: { target: 'failed', actions: assign(({ event }) => ({ listFailure: classifyFailure(event.error, false), reading: undefined })) },
      },
      on: {
        REFRESH: { target: 'loading', reenter: true },
        ACTIVATE: { target: 'loading', reenter: true },
        // A confirmation opened before this read keeps its own target and
        // may be submitted; no new one starts from a listing being replaced.
        SUBMIT: { guard: 'submittable', target: 'operating', actions: 'submit' },
      },
    },
    failed: { on: { ACTIVATE: 'loading', REFRESH: 'loading', SUBMIT: { guard: 'submittable', target: 'operating', actions: 'submit' } } },
    ready: {
      on: {
        ACTIVATE: 'loading',
        REFRESH: 'loading',
        CONFIRM: { actions: 'confirm' },
        OPEN: { actions: 'open' },
        SUBMIT: { guard: 'submittable', target: 'operating', actions: 'submit' },
      },
    },
    operating: {
      // The dispatched write keeps its receiver: a refresh waits for its
      // outcome (the listing that follows observes it), and a scope chosen
      // meanwhile is selected after it. An uncertain write is observed by
      // that listing, never replayed.
      on: {
        SCOPE: { actions: assign(({ context, event }) => ({ deferredScope: sameScope(context.scope, event.scope) ? undefined : event.scope })) },
      },
      invoke: {
        src: 'operate',
        input: ({ context }) => context.pending!,
        onDone: {
          target: 'loading',
          actions: [
            sendParent(({ context }): LibraryCreated | { type: 'LIBRARY.CHANGED' } => context.pending!.operation.kind === 'create'
              ? { type: 'LIBRARY.CREATED', scope: context.pending!.port.scope, id: context.pending!.operation.id, port: context.pending!.port } : { type: 'LIBRARY.CHANGED' }),
            assign(({ context }) => ({
              confirmation: context.confirmation?.seq === context.pending!.confirmation ? undefined : context.confirmation,
              ...settled(context),
            })),
          ],
        },
        onError: {
          target: 'loading',
          actions: assign(({ context, event }) => {
            const failure = classifyFailure(event.error, true), pending = context.pending!;
            return {
              operationFailure: { operation: pending.operation, failure },
              confirmation: context.confirmation?.seq === pending.confirmation ? { ...context.confirmation, failure, ...(failure.kind === 'uncertain' ? { uncertain: pending } : {}) } : context.confirmation,
              ...settled(context),
            };
          }),
        },
      },
    },
  },
});

export type WorkflowLibraryActor = ActorRefFrom<typeof workflowLibraryMachine>;
export type WorkflowLibrarySnapshot = SnapshotFrom<typeof workflowLibraryMachine>;
