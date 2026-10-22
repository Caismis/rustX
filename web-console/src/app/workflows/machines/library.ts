import { assign, fromPromise, sendParent, setup, type ActorRefFrom, type SnapshotFrom } from 'xstate';
import type { WorkflowEdit, WorkflowLibrary } from '../../../../../protocol/app-server/v51';
import type { WorkflowCommit } from '../../../workspaces/host';
import { classifyFailure, sameScope, type WorkflowFailure, type WorkflowScope, type WorkflowScopePort } from '../port';

/** One file operation of the library, bound to the exact scope it started in. */
export type LibraryOperation =
  | { kind: 'create'; id: string; description: string }
  | { kind: 'copy'; source: string; sourceRevision: string; id: string }
  | { kind: 'delete'; id: string; revision: string };

export interface LibraryContext {
  readonly ports: (scope: WorkflowScope) => WorkflowScopePort;
  scope: WorkflowScope;
  /** The read port of the current listing. Each new activation, scope
   * selection or refresh creates a current one; until the first, none. */
  port?: WorkflowScopePort;
  /** The listing of exactly `scope`; never another scope's. */
  library?: WorkflowLibrary;
  listFailure?: WorkflowFailure;
  pending?: LibraryOperation;
  /** The exact port and absent revision the pending operation was bound to
   * when its confirmation opened; never the current listing's. */
  pendingTarget?: { port: WorkflowScopePort; absent: string };
  operationFailure?: { operation: LibraryOperation; failure: WorkflowFailure };
}

export type LibraryEvent =
  | { type: 'SCOPE'; scope: WorkflowScope }
  | { type: 'REFRESH' }
  /** The library became visible: list now. Nothing is read before. */
  | { type: 'ACTIVATE' }
  /** Run `operation` against the port and listing its confirmation was
   * opened with, so a later refresh never retargets it. */
  | { type: 'OPERATE'; operation: LibraryOperation; port: WorkflowScopePort; absent: string }
  | { type: 'DISMISS' };

/** Sent to the owner after a create committed, naming the exact new file
 * and the port that created it. */
export interface LibraryCreated { type: 'LIBRARY.CREATED'; scope: WorkflowScope; id: string; port: WorkflowScopePort }

function edit(operation: LibraryOperation): WorkflowEdit {
  switch (operation.kind) {
    case 'create': return { operation: 'create', description: operation.description };
    case 'copy': return { operation: 'copy', source: operation.source, source_revision: operation.sourceRevision };
    case 'delete': return { operation: 'delete' };
  }
}

/** Level 1: the Workflow files physically owned by one selected scope.
 * Selecting a scope re-enters `loading`, which stops the older read, so an
 * older scope's listing has no completion path into a newer selection. */
export const workflowLibraryMachine = setup({
  types: { context: {} as LibraryContext, events: {} as LibraryEvent, input: {} as { ports: (scope: WorkflowScope) => WorkflowScopePort; scope: WorkflowScope } },
  actions: {
    current: assign(({ context }) => ({ port: context.ports(context.scope) })),
  },
  actors: {
    list: fromPromise<WorkflowLibrary, { port: WorkflowScopePort }>(({ input }) => input.port.list()),
    operate: fromPromise<WorkflowCommit, { port: WorkflowScopePort; operation: LibraryOperation; absent: string }>(({ input }) => {
      const { operation } = input;
      const expected = operation.kind === 'delete' ? operation.revision : input.absent;
      return input.port.write(operation.id, expected, edit(operation));
    }),
  },
}).createMachine({
  id: 'workflowLibrary',
  context: ({ input }) => ({ ports: input.ports, scope: input.scope }),
  initial: 'dormant',
  on: {
    SCOPE: { guard: ({ context, event }) => !sameScope(context.scope, event.scope), target: '.loading',
      actions: assign(({ context, event }) => ({ scope: event.scope, port: context.ports(event.scope), library: undefined, operationFailure: undefined })) },
    DISMISS: { actions: assign({ operationFailure: undefined }) },
  },
  states: {
    // A listing always starts from a port created for it, so it reads the
    // authorities current when it starts, never ones from App startup.
    dormant: { on: { ACTIVATE: { target: 'loading', actions: 'current' }, REFRESH: { target: 'loading', actions: 'current' } } },
    loading: {
      entry: assign({ listFailure: undefined }),
      invoke: {
        src: 'list', input: ({ context }) => ({ port: context.port! }),
        onDone: { target: 'ready', actions: assign(({ event }) => ({ library: event.output })) },
        onError: { target: 'failed', actions: assign(({ event }) => ({ listFailure: classifyFailure(event.error, false) })) },
      },
      on: { REFRESH: { target: 'loading', reenter: true, actions: 'current' } },
    },
    failed: { on: { ACTIVATE: { target: 'loading', actions: 'current' }, REFRESH: { target: 'loading', actions: 'current' } } },
    ready: {
      on: {
        ACTIVATE: { target: 'loading', actions: 'current' },
        REFRESH: { target: 'loading', actions: 'current' },
        OPERATE: { target: 'operating', actions: assign(({ event }) => ({ pending: event.operation, pendingTarget: { port: event.port, absent: event.absent }, operationFailure: undefined })) },
      },
    },
    operating: {
      // A refresh waits for the operation; a scope change stops this actor,
      // but a write already sent may commit: its outcome is then observed
      // by the next listing, never replayed.
      invoke: {
        src: 'operate',
        input: ({ context }) => ({ port: context.pendingTarget!.port, operation: context.pending!, absent: context.pendingTarget!.absent }),
        onDone: {
          target: 'loading',
          actions: [
            sendParent(({ context }): LibraryCreated | { type: 'LIBRARY.CHANGED' } => context.pending!.kind === 'create'
              ? { type: 'LIBRARY.CREATED', scope: context.pendingTarget!.port.scope, id: context.pending!.id, port: context.pendingTarget!.port } : { type: 'LIBRARY.CHANGED' }),
            assign({ pending: undefined, pendingTarget: undefined }),
          ],
        },
        onError: { target: 'loading', actions: assign(({ context, event }) => ({ operationFailure: { operation: context.pending!, failure: classifyFailure(event.error, true) }, pending: undefined, pendingTarget: undefined })) },
      },
    },
  },
});

export type WorkflowLibraryActor = ActorRefFrom<typeof workflowLibraryMachine>;
export type WorkflowLibrarySnapshot = SnapshotFrom<typeof workflowLibraryMachine>;
