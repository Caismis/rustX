import { expect, it, vi } from 'vitest';
import { FirstSubmissions, type FirstDraft } from '../src/app/new-conversation/first-submit';
import { firstSubmitPort } from '../src/app/new-conversation/port';

// Regression: a send that lands inside the just-forked Session's attach/resync
// churn must not be silently dropped. The wire evidence (no session/submissionIntent
// and no inbound/queue to the fork) showed the gesture died on the gesture-time
// authority fence instead of settling the in-flight attach first.

const sessionId = 'sess';
const target = { session_id: sessionId, conversation_id: 'conv' } as const;

function churning(overrides?: { connection?: string }) {
  const state: any = {
    connection: overrides?.connection ?? 'connected',
    generation: 1,
    authorityRevision: 1,
    endpoint: 'ep',
    views: { [sessionId]: { id: sessionId, attachment: 'attaching', nodeId: 'n1', attachmentObservation: { epoch: 1 }, target } },
  };
  const requested: string[] = [];
  const sent: string[] = [];
  const client: any = {
    getSnapshot: () => state,
    request: vi.fn(async (req: any, _expected: string, _ack: unknown, _fence: unknown) => {
      requested.push(req.method);
      if (req.method === 'session/read') return { session: { active_node: 'n1' } };
      return {};
    }),
    admitSessionOperation: vi.fn(async () => ({ current: () => true, validate: async () => true })),
    waitForAttachment: vi.fn(async (_id: string, fence: () => boolean) => {
      if (!fence()) throw new Error('Conversation connection changed. Your input was not sent.');
      // The UI-initiated attach completes and the host re-observation bumps the
      // authority revision — the churn that used to kill this send mid-flight.
      state.views[sessionId].attachment = 'attached';
      state.authorityRevision += 1;
      state.connection = 'connected';
    }),
    target: vi.fn(() => target),
    attach: vi.fn(async () => {}),
    send: vi.fn(async (_id: string, text: string, _receipts: unknown, _delivery: string, acceptance: (a: any) => void, _current: () => boolean) => {
      sent.push(text);
      acceptance({ message_id: 'm1' });
    }),
    upload: vi.fn(),
    uploadStatus: vi.fn(),
  };
  return { state, client, requested, sent };
}

const draft: FirstDraft = { workspaceId: '', text: 'Fork follow-up', files: [] };

function run(client: any) {
  const owner = new FirstSubmissions();
  const port = firstSubmitPort(client, undefined as any, () => true, () => () => undefined, true);
  return owner.submit('draft', draft, port, undefined, undefined, sessionId);
}

it('existing-Session send survives the attach-window authority churn and is dispatched once', async () => {
  const { client, requested, sent } = churning();
  expect(await run(client)).toBe(true);
  expect(client.waitForAttachment).toHaveBeenCalledTimes(3);
  expect(requested).toContain('session/submissionIntent');
  expect(sent).toEqual(['Fork follow-up']);
});

it('existing-Session send stays viable through a transient socket reconnect at the gesture', async () => {
  const { client, requested, sent } = churning({ connection: 'reconnecting' });
  expect(await run(client)).toBe(true);
  expect(requested).toContain('session/submissionIntent');
  expect(sent).toEqual(['Fork follow-up']);
});

it('existing-Session send still refuses once navigation ownership is gone', async () => {
  const { client } = churning();
  const owner = new FirstSubmissions();
  const port = firstSubmitPort(client, undefined as any, () => false, () => () => undefined, true);
  expect(await owner.submit('draft', draft, port, undefined, undefined, sessionId)).toBe(false);
  expect(client.waitForAttachment).not.toHaveBeenCalled();
  expect(client.send).not.toHaveBeenCalled();
});
