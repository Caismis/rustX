import { assign, fromPromise, raise, setup } from 'xstate';
import type { AvailableConfiguration, ConfigurationApplication } from '../../../../../protocol/app-server/v18';
import { isOutcomeUncertain, type AppServerClient, type ConnectionState } from '../../../client/app-server';

/** The native Session configuration authority of exactly one Session. */
export interface SessionConfigurationPort {
  read(): Promise<ConfigurationApplication | null>;
  /** The real native adoption gate. Browser state is never authority for it. */
  adopt(candidate: AvailableConfiguration): Promise<void>;
}

export function createSessionConfigurationPort(client: AppServerClient, sessionId: string): SessionConfigurationPort {
  return {
    read: async () => (await client.request({ method: 'session/configuration', params: { session_id: sessionId } }, 'session_configuration')).application ?? null,
    adopt: async candidate => {
      await client.request({
        method: 'session/adoptConfiguration',
        params: { session_id: sessionId, candidate: candidate.identity, expected_binding: candidate.expected_binding },
      }, 'configuration_application');
    },
  };
}

export interface SessionConfigurationContext {
  port: SessionConfigurationPort;
  connection: ConnectionState;
  generation: number;
  /** The latest native application observed for this Session. */
  application?: ConfigurationApplication;
  /** Owned by the `observation` region alone. */
  readError: string;
  /** Owned by the `adoption` region alone. A successful authoritative read
   * answers the read, never an adoption rejection or an unknown adoption
   * outcome, so the two never share a field. */
  adoptionError: string;
}

export type SessionConfigurationEvent =
  | { type: 'TRANSPORT'; connection: ConnectionState; generation: number }
  | { type: 'REFRESH' }
  | { type: 'ADOPT'; candidate: AvailableConfiguration };

/** Session configuration observation and Session adoption.
 *
 * They are two independent facts and therefore two regions: whether this
 * browser currently knows the native application, and where an explicit
 * adoption attempt ended up. A successful read can never clear an adoption
 * rejection, and an adoption response can never clear a read failure, because
 * neither region can assign the other's field.
 *
 * Adoption stays explicit and native-gated: there is no auto-adopt, no
 * adopt-when-idle queue, no automatic retry and no replay after an unknown
 * outcome — an unknown outcome causes an authoritative reread only. */
export const sessionConfigurationMachine = setup({
  types: {
    context: {} as SessionConfigurationContext,
    events: {} as SessionConfigurationEvent,
    input: {} as { port: SessionConfigurationPort; connection: ConnectionState; generation: number },
  },
  actors: {
    readConfiguration: fromPromise(({ input }: { input: { port: SessionConfigurationPort } }) => input.port.read()),
    adoptConfiguration: fromPromise(({ input }: { input: { port: SessionConfigurationPort; candidate: AvailableConfiguration } }) =>
      input.port.adopt(input.candidate)),
  },
  guards: {
    generationChanged: ({ context, event }) => event.type === 'TRANSPORT' && event.generation !== context.generation,
    adoptionUncertain: ({ event }) => isOutcomeUncertain((event as unknown as { error: unknown }).error),
  },
  actions: {
    applyTransport: assign({
      connection: ({ context, event }) => event.type === 'TRANSPORT' ? event.connection : context.connection,
      generation: ({ context, event }) => event.type === 'TRANSPORT' ? event.generation : context.generation,
    }),
    /** Adopt one native observation. A native application version is monotonic
     * inside one scope, so an older projection never regresses a newer one. */
    adoptObservation: assign({
      application: ({ context, event }) => {
        const next = (event as unknown as { output: ConfigurationApplication | null }).output ?? undefined;
        if (context.application && next && BigInt(context.application.version) > BigInt(next.version)) return context.application;
        return next;
      },
      readError: () => '',
    }),
    recordReadFailure: assign({ readError: ({ event }) => String((event as unknown as { error: unknown }).error) }),
    recordAdoptionFailure: assign({
      adoptionError: ({ event }) => {
        const cause = (event as unknown as { error: unknown }).error;
        return isOutcomeUncertain(cause) ? 'Adoption outcome uncertain. Rereading authority; adoption will not be replayed.' : String(cause);
      },
    }),
    clearAdoptionFailure: assign({ adoptionError: () => '' }),
  },
}).createMachine({
  id: 'sessionConfiguration',
  context: ({ input }) => ({ port: input.port, connection: input.connection, generation: input.generation, readError: '', adoptionError: '' }),
  type: 'parallel',
  states: {
    /** Does this browser currently know the native Session application? */
    observation: {
      initial: 'idle',
      states: {
        /** Nothing has asked for an observation yet. The actor reads when a
         * presentation attaches or a native trigger arrives, never on its own. */
        idle: { on: { REFRESH: 'loading' } },
        loading: {
          invoke: {
            src: 'readConfiguration',
            input: ({ context }) => ({ port: context.port }),
            onDone: { target: 'ready', actions: 'adoptObservation' },
            onError: { target: 'unavailable', actions: 'recordReadFailure' },
          },
          // Re-entering stops the read actor already in flight, so read
          // ordering is structural: a superseded read can never publish a
          // projection or a failure, in any delivery order.
          on: { REFRESH: { target: 'loading', reenter: true } },
        },
        /** The last observation is authoritative and current. */
        ready: { on: { REFRESH: 'loading' } },
        /** The last observation is retained and explicitly stale. */
        unavailable: { on: { REFRESH: 'loading' } },
      },
    },

    /** Where an explicit adoption attempt ended up. */
    adoption: {
      initial: 'idle',
      states: {
        idle: { on: { ADOPT: { target: 'submitting', actions: 'clearAdoptionFailure' } } },
        submitting: {
          invoke: {
            src: 'adoptConfiguration',
            input: ({ context, event }) => ({ port: context.port, candidate: (event as Extract<SessionConfigurationEvent, { type: 'ADOPT' }>).candidate }),
            // The authoritative reread after an adoption response is
            // independent cleanup: it is raised here, and whether it succeeds
            // or fails it can neither strand this region nor replay adoption.
            onDone: { target: 'idle', actions: raise({ type: 'REFRESH' }) },
            onError: [
              { guard: 'adoptionUncertain', target: 'uncertain', actions: ['recordAdoptionFailure', raise({ type: 'REFRESH' })] },
              { target: 'rejected', actions: ['recordAdoptionFailure', raise({ type: 'REFRESH' })] },
            ],
          },
        },
        rejected: { on: { ADOPT: { target: 'submitting', actions: 'clearAdoptionFailure' } } },
        uncertain: { on: { ADOPT: { target: 'submitting', actions: 'clearAdoptionFailure' } } },
      },
    },
  },
  on: {
    TRANSPORT: [
      { guard: 'generationChanged', actions: ['applyTransport', raise({ type: 'REFRESH' })] },
      { actions: 'applyTransport' },
    ],
  },
});
