import { materializeSource } from '../../../protocol/app-server/source-observation';
import type { AttachmentTarget, RuntimeClientEvent, SourceTarget } from '../../../protocol/app-server/v57';
import { ObservationChain } from '../../../protocol/app-server/observation';
import { checkpointSnapshot, foldRuntimeEvent } from '../../../protocol/app-server/projection';
import { materializeEffective } from '../../../protocol/app-server/effective-observation';
import { materializeApplication } from '../../../protocol/app-server/application-observation';
import type { AppServerHost } from '../../../tui/src/app-server/host';
import { expect, type Page } from '@playwright/test';

/** Exercise the visible bounded read before answering a deferred questionnaire.
 * Reattachment restores its native header, rather than a cached request body. */
export async function openQuestionnaire(page: Page) {
  const questionnaire = page.getByRole('region', { name: /^(Questionnaire|问卷)$/ });
  const deferred = page.getByRole('button', { name: /^(Read content \(0 bytes\)|读取内容（0 字节）)$/ });
  await expect(questionnaire.or(deferred).first()).toBeVisible();
  if (await deferred.isVisible()) await deferred.click();
  await expect(questionnaire).toBeVisible();
}

/** Read the real bounded Source receipt and its authoritative field ranges. */
export async function readSource(remote: AppServerHost, target: SourceTarget) {
  const { projection } = await remote.client.call('configuration/sourcesRead', { target }, 'source_settings');
  return materializeSource(projection, async (reference, offset) => (await remote.client.call(
    'configuration/sourceValueRead', { target, reference, offset, max_bytes: 65_536 }, 'source_value_chunk',
  )).chunk, () => true);
}

export async function readEffective(remote: AppServerHost, target: AttachmentTarget) {
  const { projection } = await remote.client.call('session/effectiveConfiguration', { target }, 'effective_configuration');
  return materializeEffective(projection, async (reference, offset) => (await remote.client.call(
    'session/effectiveConfigurationValueRead', { target, reference, offset, max_bytes: 65_536 }, 'configuration_value_chunk',
  )).chunk, () => !remote.client.closed);
}

export async function readApplication(remote: AppServerHost, session_id: string) {
  const { projection } = await remote.client.call('session/configuration', { session_id }, 'session_configuration');
  const application = await materializeApplication(projection, async (reference, offset) => (await remote.client.call(
    'session/configurationValueRead', { session_id, reference, offset, max_bytes: 65_536 }, 'session_application_value_chunk',
  )).chunk, () => !remote.client.closed);
  if (!application) throw new Error('Native Session has no configuration application.');
  return application;
}

/** Follow the real receipt and complete native transitions. A missing cursor
 * fails preparation instead of waiting for a later full snapshot to repair it. */
export async function followRoot(remote: AppServerHost, target: AttachmentTarget, options: { measure?: boolean } = {}) {
  const { checkpoint } = await remote.client.call('session/checkpoint', { target }, 'checkpoint');
  let snapshot = checkpointSnapshot(checkpoint);
  const observation = new ObservationChain<RuntimeClientEvent>();
  observation.installCheckpoint(checkpoint.cursor);
  observation.beginSubscription(checkpoint.cursor);
  let waiting: { attempt?: string; resolve: () => void; reject: (error: Error) => void } | undefined;
  let received: { registrationId: string; method: string; cursor?: string; result?: string } | undefined;
  const profile = options.measure ? {
    notifications: 0, notificationMs: 0, foldCalls: 0, foldMs: 0,
    settlements: 0, lastSettlementAt: 0, maxTranscriptEntries: 0, maxMessageDescriptors: 0,
  } : undefined;
  const fold = (event: RuntimeClientEvent) => {
    const started = profile ? performance.now() : undefined;
    try {
      snapshot = foldRuntimeEvent(snapshot, event);
      if (profile) {
        profile.maxTranscriptEntries = Math.max(profile.maxTranscriptEntries, snapshot.transcript.entries?.length ?? 0);
        profile.maxMessageDescriptors = Math.max(profile.maxMessageDescriptors, snapshot.message_descriptors.length);
      }
      const attempt = snapshot.attempt;
      if (!waiting || !attempt || attempt.phase.type !== 'settled') return;
      if (typeof attempt.attempt_id !== 'string') {
        const current = waiting; waiting = undefined;
        current.reject(new Error('Native preparation terminal lacks its exact inline Attempt identity.')); return;
      }
      if (attempt.attempt_id === waiting.attempt) return;
      const current = waiting; waiting = undefined;
      if (attempt.phase.outcome.type === 'completed') {
        if (profile) { profile.settlements++; profile.lastSettlementAt = performance.now(); }
        current.resolve();
      }
      else current.reject(new Error(`Native Attempt failed: ${JSON.stringify(attempt.phase.outcome)}`));
    } finally {
      if (profile && started !== undefined) { profile.foldCalls++; profile.foldMs += performance.now() - started; }
    }
  };
  const stop = remote.client.onNotification(note => {
    if ((note.method !== 'session/event' && note.method !== 'session/resyncRequired' && note.method !== 'session/observationUnavailable') || JSON.stringify(note.params.target) !== JSON.stringify(target)) return;
    const started = profile ? performance.now() : undefined;
    try {
      received = { registrationId: note.params.registration_id, method: note.method, cursor: note.method === 'session/event' ? note.params.cursor : undefined };
      const result = observation.deliver(note.method === 'session/event'
        ? { registrationId: note.params.registration_id, cursor: note.params.cursor, event: note.params.event }
        : note.method === 'session/observationUnavailable' ? {registrationId:note.params.registration_id,unavailable:true}
        : { registrationId: note.params.registration_id, resync: true }, fold);
      received.result = result;
      if (result === 'unavailable') { waiting?.reject(new Error('Native preparation source is unavailable.')); waiting = undefined; }
      if (result === 'recovery') { waiting?.reject(new Error('Native preparation observation requires recovery.')); waiting = undefined; }
    } finally {
      if (profile && started !== undefined) { profile.notifications++; profile.notificationMs += performance.now() - started; }
    }
  });
  try {
    const receipt = await remote.client.call('session/subscribe', { target, after_cursor: checkpoint.cursor }, 'subscribed');
    if (!observation.acceptReceipt(receipt.registration_id, receipt.after_cursor)) throw new Error('Native subscription receipt does not match its checkpoint.');
  } catch (error) { stop(); throw error; }
  return {
    /** Local-only scalar timings. Reading these never performs native work. */
    profile() { return profile ? { ...profile } : undefined; },
    /** Existing local observation state only; never query native execution to diagnose a wait. */
    diagnostics() {
      const attempt = snapshot.attempt;
      return {
        phase: observation.phase, registrationId: observation.registrationId, applied: observation.applied, received,
        waitingAfterAttempt: waiting?.attempt?.slice(0, 256),
        projectedAttempt: attempt ? {
          id: typeof attempt.attempt_id === 'string' ? attempt.attempt_id.slice(0, 256) : 'deferred',
          phase: attempt.phase.type, outcome: attempt.phase.type === 'settled' ? attempt.phase.outcome.type : undefined,
        } : undefined,
      };
    },
    nextSettlement() {
      if (waiting || observation.phase !== 'following') throw new Error('Native preparation observation is not ready for an Attempt.');
      const previous = snapshot.attempt;
      let previousAttempt: string | undefined;
      if (previous) {
        if (typeof previous.attempt_id !== 'string') throw new Error('Native preparation owner lacks its exact inline Attempt identity.');
        previousAttempt = previous.attempt_id;
      }
      return new Promise<void>((resolve, reject) => { waiting = { attempt: previousAttempt, resolve, reject }; });
    },
    stop() { stop(); observation.retire(); waiting?.reject(new Error('Native preparation observation retired.')); waiting = undefined; },
  };
}
