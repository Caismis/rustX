import { assign, fromPromise, setup } from 'xstate';
import { isOutcomeUncertain } from '../../client/app-server';
import { WorkspaceHostError } from '../../workspaces/host';
import type { SessionModelConfig, UploadReceipt } from '../../../../protocol/app-server/v25';

/** Only draft values and acknowledged native facts. No provisional identity. */
export interface FirstDraft { workspaceId: string; text: string; files: readonly File[]; model?: SessionModelConfig }
export interface CreatedSession { id: string; node: string; conversation: string; diagnostic?: string }
export interface FirstSubmitPort {
  current(): boolean;
  create(draft: FirstDraft): Promise<CreatedSession>;
  attach(session: CreatedSession): Promise<void>;
  model(session: CreatedSession, model: SessionModelConfig): Promise<void>;
  upload(session: CreatedSession, file: File): Promise<UploadReceipt>;
  send(session: CreatedSession, draft: FirstDraft, receipts: readonly UploadReceipt[]): Promise<void>;
}
/** A submission is bound to the authority current at its own gesture. */
interface Context { port?: FirstSubmitPort; draft?: FirstDraft; session?: CreatedSession; receipts: UploadReceipt[]; error?: unknown }
function live(port: FirstSubmitPort) { if (!port.current()) throw new Error('Authority changed. Reread native state; no operation was replayed.'); }
const step = (action: (context: Context & { port: FirstSubmitPort }) => Promise<unknown>) => fromPromise(async ({ input }: { input: Context }) => {
  const port = input.port!; live(port); const value = await action({ ...input, port }); live(port); return value;
});
/** Known rejection permits only a new explicit submission. Uncertain creation
 * has no retry edge. Capture the create acknowledgement before fencing the next
 * effect: a changed authority must never erase a committed native identity. */
export const firstSubmitMachine = setup({
  types: { context: {} as Context, events: {} as { type: 'SUBMIT'; draft: FirstDraft; port: FirstSubmitPort } | { type: 'RETIRE' } | { type: 'RESET' } },
  actors: {
    create: fromPromise(async ({ input }: { input: Context }) => { live(input.port!); return input.port!.create(input.draft!); }),
    attach: step(c => { if (c.session!.diagnostic) throw new Error(c.session!.diagnostic); return c.port.attach(c.session!); }),
    model: step(c => c.port.model(c.session!, c.draft!.model!)),
    upload: step(c => c.port.upload(c.session!, c.draft!.files[c.receipts.length])),
    send: step(c => c.port.send(c.session!, c.draft!, c.receipts)),
  },
  actions: { fail: assign({ error: ({ event }) => 'error' in event ? event.error : undefined }) },
}).createMachine({
  id: 'firstSubmit', initial: 'drafting', context: { receipts: [] },
  on: { RETIRE: '.retired', RESET: { target: '.drafting', actions: assign(() => ({ port: undefined, draft: undefined, session: undefined, receipts: [], error: undefined })) } },
  states: {
    drafting: { on: { SUBMIT: { guard: ({ event }) => event.port.current() && !!event.draft.workspaceId && (!!event.draft.text.trim() || event.draft.files.length > 0), target: 'creating_session', actions: assign({ port: ({ event }) => event.port, draft: ({ event }) => event.draft, error: () => undefined }) } } },
    creating_session: { invoke: { src: 'create', input: ({ context }) => context, onDone: { target: 'attaching_session', actions: assign({ session: ({ event }) => event.output as CreatedSession }) }, onError: [{ guard: ({ event }) => isOutcomeUncertain(event.error) || event.error instanceof WorkspaceHostError && event.error.uncertain, target: 'uncertain_creation', actions: 'fail' }, { target: 'drafting', actions: 'fail' }] } },
    attaching_session: { invoke: { src: 'attach', input: ({ context }) => context, onDone: 'selecting_model', onError: { target: 'failed', actions: 'fail' } } },
    selecting_model: { always: [{ guard: ({ context }) => !!context.draft?.model, target: 'applying_session_model' }, { target: 'next_attachment' }] },
    applying_session_model: { invoke: { src: 'model', input: ({ context }) => context, onDone: 'next_attachment', onError: { target: 'failed', actions: 'fail' } } },
    next_attachment: { always: [{ guard: ({ context }) => context.receipts.length < context.draft!.files.length, target: 'uploading_attachments' }, { target: 'submitting_turn' }] },
    uploading_attachments: { invoke: { src: 'upload', input: ({ context }) => context, onDone: { target: 'next_attachment', actions: assign({ receipts: ({ context, event }) => [...context.receipts, event.output as UploadReceipt] }) }, onError: { target: 'failed', actions: 'fail' } } },
    submitting_turn: { invoke: { src: 'send', input: ({ context }) => context, onDone: 'session', onError: { target: 'failed', actions: 'fail' } } },
    session: {}, uncertain_creation: {}, failed: {}, retired: {},
  },
});
