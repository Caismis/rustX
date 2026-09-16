import type { RuntimeClientSnapshot } from '../../../protocol/app-server/v4';
import type { SessionView } from '../client/app-server';
/** Select only canonical snapshot messages. Streaming is a separately labelled
 * server projection and disappears when its canonical identity is committed. */
export function conversation(snapshot: RuntimeClientSnapshot) {
  const messages = snapshot.messages;
  const inFlight = snapshot.attempt?.in_flight;
  return { messages, streaming: inFlight && snapshot.attempt?.phase.type !== 'settled' && !messages.some(message => message.id === inFlight.message_id) ? inFlight : undefined };
}
export const activeAttempt = (snapshot?: RuntimeClientSnapshot) => !!snapshot?.attempt && snapshot.attempt.phase.type !== 'settled';
/** Product guard, not execution authority. Acknowledged MessageIds close the
 * projection gap until native pending/canonical observations reconcile them. */
export function executionIdle(view?: Pick<SessionView, 'snapshot' | 'submissions'>): boolean {
  return !!view?.snapshot && !activeAttempt(view.snapshot)
    && !view.snapshot.inbound.pending?.length && !view.submissions?.length;
}
export const json = (value: unknown) => JSON.stringify(value, null, 2) ?? 'Unavailable';
