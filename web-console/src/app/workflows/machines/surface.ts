import { assign, enqueueActions, setup, stopChild, type ActorRefFrom, type SnapshotFrom } from 'xstate';
import { USER_SCOPE, type WorkflowScope, type WorkflowScopePort } from '../port';
import { isDirty, workflowEditorMachine, type WorkflowEditorActor } from './editor';
import { workflowLibraryMachine, type LibraryCreated, type WorkflowLibraryActor } from './library';

export interface SurfaceContext {
  readonly ports: (scope: WorkflowScope) => WorkflowScopePort;
  library: WorkflowLibraryActor;
  editor?: WorkflowEditorActor;
  /** The navigation to continue once unsaved edits are discarded. */
  leaving?: { kind: 'back' } | { kind: 'external'; proceed: () => void };
}

export type SurfaceEvent =
  | { type: 'OPEN'; scope: WorkflowScope; id: string }
  | LibraryCreated | { type: 'LIBRARY.CHANGED' }
  | { type: 'BACK' }
  | { type: 'LEAVE'; proceed: () => void }
  | { type: 'CONFIRM' } | { type: 'CANCEL' };

/** Unsaved semantic edits, or a write whose outcome is not yet settled. */
export function editorHolds(editor: WorkflowEditorActor | undefined): boolean {
  if (!editor) return false;
  const snapshot = editor.getSnapshot();
  return isDirty(snapshot.context) || snapshot.matches({ ready: 'saving' }) || !!snapshot.context.uncertain;
}

/** The browser-lifetime Workflows surface: Level 1 (library) and Level 2
 * (one exact file's editor), and the guard that keeps unsaved edits from
 * being dropped by Back, by the sidebar or by any other navigation. */
export const workflowSurfaceMachine = setup({
  types: { context: {} as SurfaceContext, events: {} as SurfaceEvent, input: {} as { ports: (scope: WorkflowScope) => WorkflowScopePort; scope?: WorkflowScope } },
  actors: { library: workflowLibraryMachine, editor: workflowEditorMachine },
  guards: {
    holds: ({ context }) => editorHolds(context.editor),
  },
  actions: {
    open: assign(({ context, event, spawn }) => {
      if (event.type !== 'OPEN' && event.type !== 'LIBRARY.CREATED') return {};
      const port = context.ports(event.scope);
      return { editor: spawn('editor', { id: 'editor', input: { port, target: { endpoint: port.endpoint, scope: event.scope, id: event.id } } }) as WorkflowEditorActor };
    }),
    close: enqueueActions(({ context, enqueue }) => {
      if (context.editor) enqueue(stopChild(context.editor));
      enqueue.assign({ editor: undefined, leaving: undefined });
      enqueue.sendTo(context.library, { type: 'REFRESH' });
    }),
    proceed: ({ context }) => { if (context.leaving?.kind === 'external') context.leaving.proceed(); },
  },
}).createMachine({
  id: 'workflowSurface',
  context: ({ input, spawn }) => ({ ports: input.ports, library: spawn('library', { id: 'library', input: { ports: input.ports, scope: input.scope ?? USER_SCOPE } }) as WorkflowLibraryActor }),
  initial: 'library',
  on: {
    // Leaving the route keeps a clean editor for return; unsaved edits ask first.
    LEAVE: [
      { guard: 'holds', target: '.confirming', actions: assign(({ event }) => ({ leaving: { kind: 'external' as const, proceed: event.proceed } })) },
      { actions: ({ event }) => event.proceed() },
    ],
  },
  states: {
    library: {
      on: {
        OPEN: { target: 'editor', actions: 'open' },
        'LIBRARY.CREATED': { target: 'editor', actions: 'open' },
      },
    },
    editor: {
      on: {
        BACK: [
          { guard: 'holds', target: 'confirming', actions: assign({ leaving: { kind: 'back' as const } }) },
          { target: 'library', actions: 'close' },
        ],
      },
    },
    confirming: {
      on: {
        CONFIRM: { target: 'library', actions: ['proceed', 'close'] },
        CANCEL: { target: 'editor', actions: assign({ leaving: undefined }) },
      },
    },
  },
});

export type WorkflowSurfaceActor = ActorRefFrom<typeof workflowSurfaceMachine>;
export type WorkflowSurfaceSnapshot = SnapshotFrom<typeof workflowSurfaceMachine>;
