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
  port: WorkflowScopePort;
  /** The listing of exactly `scope`; never another scope's. */
  library?: WorkflowLibrary;
  listFailure?: WorkflowFailure;
  pending?: LibraryOperation;
  operationFailure?: { operation: LibraryOperation; failure: WorkflowFailure };
}

export type LibraryEvent =
  | { type: 'SCOPE'; scope: WorkflowScope }
  | { type: 'REFRESH' }
  /** The library became visible: list now. Nothing is read before. */
  | { type: 'ACTIVATE' }
  | { type: 'OPERATE'; operation: LibraryOperation }
  | { type: 'DISMISS' };

/** Sent to the owner after a create committed, naming the exact new file. */
export interface LibraryCreated { type: 'LIBRARY.CREATED'; scope: WorkflowScope; id: string }

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
  context: ({ input }) => ({ ports: input.ports, scope: input.scope, port: input.ports(input.scope) }),
  initial: 'dormant',
  on: {
    SCOPE: { guard: ({ context, event }) => !sameScope(context.scope, event.scope), target: '.loading',
      actions: assign(({ context, event }) => ({ scope: event.scope, port: context.ports(event.scope), library: undefined, operationFailure: undefined })) },
    REFRESH: { target: '.loading', actions: assign(({ context }) => ({ port: context.ports(context.scope) })) },
    DISMISS: { actions: assign({ operationFailure: undefined }) },
  },
  states: {
    dormant: { on: { ACTIVATE: 'loading' } },
    loading: {
      entry: assign({ listFailure: undefined }),
      invoke: {
        src: 'list', input: ({ context }) => ({ port: context.port }),
        onDone: { target: 'ready', actions: assign(({ event }) => ({ library: event.output })) },
        onError: { target: 'failed', actions: assign(({ event }) => ({ listFailure: classifyFailure(event.error, false) })) },
      },
    },
    failed: { on: { ACTIVATE: 'loading' } },
    ready: {
      on: {
        ACTIVATE: 'loading',
        OPERATE: { guard: ({ context }) => !!context.library, target: 'operating', actions: assign(({ event }) => ({ pending: event.operation, operationFailure: undefined })) },
      },
    },
    operating: {
      // A scope change stops this actor, but a write already sent may commit:
      // its outcome is then observed by the next listing, never replayed.
      invoke: {
        src: 'operate',
        input: ({ context }) => ({ port: context.port, operation: context.pending!, absent: context.library!.absent_revision }),
        onDone: {
          target: 'loading',
          actions: [
            sendParent(({ context }): LibraryCreated | { type: 'LIBRARY.CHANGED' } => context.pending!.kind === 'create'
              ? { type: 'LIBRARY.CREATED', scope: context.scope, id: context.pending!.id } : { type: 'LIBRARY.CHANGED' }),
            assign({ pending: undefined }),
          ],
        },
        onError: { target: 'loading', actions: assign(({ context, event }) => ({ operationFailure: { operation: context.pending!, failure: classifyFailure(event.error, true) }, pending: undefined })) },
      },
    },
  },
});

export type WorkflowLibraryActor = ActorRefFrom<typeof workflowLibraryMachine>;
export type WorkflowLibrarySnapshot = SnapshotFrom<typeof workflowLibraryMachine>;
