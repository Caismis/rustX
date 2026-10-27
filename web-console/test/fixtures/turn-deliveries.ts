import { type Server } from '../fixture';
import type { RuntimeClientSnapshot } from '../../../protocol/app-server/v61';
import { deliveryConversation } from '../turn-deliveries-fixture';

export function turnDeliveriesFixture(server: Server, current: RuntimeClientSnapshot) {
  const completed = deliveryConversation(new URLSearchParams(location.search).has('deferred'));
  completed.attempt = null;
  Object.assign(current, structuredClone(completed));
  current.transcript.entries = current.transcript.entries!.slice(0, 1);
  current.transcript.deferred_entries = [];
  current.messages = current.messages.filter(message => message.role === 'user');
  current.message_descriptors = [];
  current.attempt = { attempt_id: 'delivery-attempt', phase: { type: 'running' }, turn: 1,
    in_flight: { message_id: 'delivery-answer', blocks: [{ type: 'text', block_index: 0, text: 'Finalizing the report and instructions.' }] } };
  window.turnDeliveriesFixture = {
    settle: () => server.update('A', completed),
    refresh: () => server.client.refresh('A'),
    async nextQuestion() {
      const next = structuredClone(server.snapshots.get('A')!);
      const message = { role: 'user' as const, source: 'human' as const, id: 'next-question', content: [{ type: 'text' as const, text: 'Continue with the next task.' }] };
      next.messages.push(message); next.transcript.entries!.push({ cursor: '5', item: { type: 'message', message } });
      await server.update('A', next);
    },
  };
}

declare global { interface Window { turnDeliveriesFixture: { settle(): Promise<void>; refresh(): Promise<void>; nextQuestion(): Promise<void> } } }
