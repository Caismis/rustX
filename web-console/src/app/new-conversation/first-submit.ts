import { assign, fromPromise, setup } from 'xstate';
import type { SessionModelConfig, UploadReceipt } from '../../../../protocol/app-server/v21';

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
interface Context { port: FirstSubmitPort; draft?: FirstDraft; session?: CreatedSession; receipts: UploadReceipt[]; error?: unknown }
function live(port: FirstSubmitPort) { if (!port.current()) throw new Error('Authority changed. Reread native state; no operation was replayed.'); }
const step = (action: (context: Context) => Promise<unknown>) => fromPromise(async ({ input }: { input: Context }) => {
  live(input.port); const value = await action(input); live(input.port); return value;
});
/** Acknowledged creation is the irreversible commit point. Every later error
 * retains it and every individual upload receipt. Errors have no retry edge:
 * reconnect/reread is recovery; an unknown mutation is never replayed. */
export const firstSubmitMachine = setup({
  types: { context: {} as Context, input: {} as { port: FirstSubmitPort }, events: {} as { type: 'SUBMIT'; draft: FirstDraft } | { type: 'RETIRE' } },
  actors: {
    create: step(c => c.port.create(c.draft!)),
    attach: step(c => { if (c.session!.diagnostic) throw new Error(c.session!.diagnostic); return c.port.attach(c.session!); }),
    model: step(c => c.port.model(c.session!, c.draft!.model!)),
    upload: step(c => c.port.upload(c.session!, c.draft!.files[c.receipts.length])),
    send: step(c => c.port.send(c.session!, c.draft!, c.receipts)),
  },
  actions: { fail: assign({ error: ({ event }) => 'error' in event ? event.error : undefined }) },
}).createMachine({
  id: 'firstSubmit', initial: 'drafting', context: ({ input }) => ({ port: input.port, receipts: [] }),
  on: { RETIRE: '.retired' },
  states: {
    drafting: { on: { SUBMIT: { guard: ({ context, event }) => context.port.current() && !!event.draft.workspaceId && (!!event.draft.text.trim() || event.draft.files.length > 0), target: 'creating_session', actions: assign({ draft: ({ event }) => event.draft }) } } },
    creating_session: { invoke: { src: 'create', input: ({ context }) => context, onDone: { target: 'attaching_session', actions: assign({ session: ({ event }) => event.output as CreatedSession }) }, onError: { target: 'failed', actions: 'fail' } } },
    attaching_session: { invoke: { src: 'attach', input: ({ context }) => context, onDone: 'selecting_model', onError: { target: 'failed', actions: 'fail' } } },
    selecting_model: { always: [{ guard: ({ context }) => !!context.draft?.model, target: 'applying_session_model' }, { target: 'next_attachment' }] },
    applying_session_model: { invoke: { src: 'model', input: ({ context }) => context, onDone: 'next_attachment', onError: { target: 'failed', actions: 'fail' } } },
    next_attachment: { always: [{ guard: ({ context }) => context.receipts.length < context.draft!.files.length, target: 'uploading_attachments' }, { target: 'submitting_turn' }] },
    uploading_attachments: { invoke: { src: 'upload', input: ({ context }) => context, onDone: { target: 'next_attachment', actions: assign({ receipts: ({ context, event }) => [...context.receipts, event.output as UploadReceipt] }) }, onError: { target: 'failed', actions: 'fail' } } },
    submitting_turn: { invoke: { src: 'send', input: ({ context }) => context, onDone: 'session', onError: { target: 'failed', actions: 'fail' } } },
    session: { type: 'final' }, failed: {}, retired: { type: 'final' },
  },
});
