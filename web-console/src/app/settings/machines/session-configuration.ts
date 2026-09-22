import { assign, fromPromise, raise, setup, type SnapshotFrom } from 'xstate';
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
  /** The native application observed for this Session *in the current
   * connection generation*, and the only value the monotonic version
   * comparison ever runs against.
   *
   * A native application version is a runtime counter of one App Server
   * process. It is monotonic inside the generation that produced it and means
   * nothing across a restart — generation 2's version 3 is not older than
   * generation 1's version 100. The comparison is therefore scoped by
   * construction: a generation change empties this field, so the first
   * authoritative observation of the new generation has nothing to be compared
   * against and simply becomes that generation's baseline. */
  application?: ConfigurationApplication;
  /** The last observation of an earlier connection generation, retained as
   * explicitly stale presentation data alone. It is never a comparison
   * baseline and never becomes authoritative again. */
  staleApplication?: ConfigurationApplication;
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
  | { type: 'ADOPT'; candidate: AvailableConfiguration }
  /** The native adoption response was classified and now owes the adoption
   * transaction's own authoritative reread. Raised by the `adoption` region. */
  | { type: 'ADOPTION.REREAD' };

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
 * outcome — an unknown outcome causes an authoritative reread only.
 *
 * Read ordering inside one generation is structural: a `REFRESH` re-enters
 * `loading`, which stops the read already in flight, so a superseded read can
 * publish neither a projection nor a failure. Across generations the *values*
 * need the same care, because a native application version is a per-process
 * runtime counter: a generation change retires the held observation into
 * explicitly stale presentation data, so the first observation of the new
 * generation is never compared against a counter from a different process.
 *
 * One adoption transaction spans both regions, and its span is explicit: the
 * `adoptionInFlight` tag holds from the moment the adoption is submitted,
 * through the native response, until the authoritative reread that response
 * owes has settled or been superseded by a newer read. That terminal point is
 * what the configuration system observes to release a Session actor that no
 * presentation holds any longer. */
export const sessionConfigurationMachine = setup({
  types: {
    context: {} as SessionConfigurationContext,
    events: {} as SessionConfigurationEvent,
    input: {} as { port: SessionConfigurationPort; connection: ConnectionState; generation: number },
    tags: {} as 'adoptionInFlight',
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
    /** A replaced connection generation ends the application-version domain the
     * held observation belongs to. What that generation observed stays
     * available only as stale presentation data, and is out of the comparison
     * from this moment on. */
    retireObservation: assign({
      staleApplication: ({ context }) => context.application ?? context.staleApplication,
      application: () => undefined,
      // The read failure answered a read of the generation that ended with it.
      // Where an adoption attempt stands is a mutation fact and survives.
      readError: () => '',
    }),
    /** Adopt one native observation. Inside one connection generation a native
     * application version is monotonic, so an obsolete result never regresses a
     * newer one. Across generations there is nothing to compare at all, and
     * this observation establishes the new generation's baseline. */
    adoptObservation: assign({
      application: ({ context, event }) => {
        const next = (event as unknown as { output: ConfigurationApplication | null }).output ?? undefined;
        if (context.application && next && BigInt(context.application.version) > BigInt(next.version)) return context.application;
        return next;
      },
      staleApplication: () => undefined,
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
      // Every adoption response owes a fresh authoritative reread. The
      // transition is internal to this region, so it leaves the `adoption`
      // region untouched, yet it still re-enters `loading` and so stops any
      // read already in flight.
      on: { 'ADOPTION.REREAD': '.loading.adoptionReread' },
      states: {
        /** Nothing has asked for an observation yet. The actor reads when a
         * presentation attaches or a native trigger arrives, never on its own. */
        idle: { on: { REFRESH: 'loading' } },
        loading: {
          initial: 'requested',
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
          states: {
            /** A read a presentation or a native trigger asked for. */
            requested: {},
            /** The authoritative reread an adoption response owes. It is the
             * last step of that adoption transaction, which ends when this read
             * settles — or when a newer read supersedes it and takes the read
             * order over. */
            adoptionReread: { tags: 'adoptionInFlight' },
          },
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
          tags: 'adoptionInFlight',
          invoke: {
            src: 'adoptConfiguration',
            input: ({ context, event }) => ({ port: context.port, candidate: (event as Extract<SessionConfigurationEvent, { type: 'ADOPT' }>).candidate }),
            // The authoritative reread after an adoption response is owed by
            // the adoption transaction but settles in the `observation` region:
            // it is raised here, and whether it succeeds or fails it can
            // neither strand this region nor replay adoption.
            onDone: { target: 'idle', actions: raise({ type: 'ADOPTION.REREAD' }) },
            onError: [
              { guard: 'adoptionUncertain', target: 'uncertain', actions: ['recordAdoptionFailure', raise({ type: 'ADOPTION.REREAD' })] },
              { target: 'rejected', actions: ['recordAdoptionFailure', raise({ type: 'ADOPTION.REREAD' })] },
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
      { guard: 'generationChanged', actions: ['applyTransport', 'retireObservation', raise({ type: 'REFRESH' })] },
      { actions: 'applyTransport' },
    ],
  },
});

/** Whether one Session adoption transaction is still in flight: submitted and
 * not yet answered natively, or answered and still awaiting the authoritative
 * reread it owes. Its end is the adoption transaction's terminal point. */
export function adoptionInFlight(snapshot: SnapshotFrom<typeof sessionConfigurationMachine>): boolean {
  return snapshot.hasTag('adoptionInFlight');
}
