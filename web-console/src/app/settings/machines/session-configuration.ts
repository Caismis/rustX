import { assign, fromPromise, raise, setup, stateIn, type SnapshotFrom } from 'xstate';
import type { AvailableConfiguration, ConfigurationApplication } from '../../../../../protocol/app-server/v41';
import { isOutcomeUncertain, type AppServerClient, type ClientView, type ConnectionState } from '../../../client/app-server';

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

/** Everything the client's transport tells one Session's configuration actor.
 *
 * `publication` is the complete application native last published for this
 * Session on the current connection (`configuration/changed`). It is the same
 * native view `session/configuration` returns, so it is consumed as an
 * authoritative observation — never as a cue to read that application again.
 *
 * Nothing about the Session's Runtime Client snapshot is part of this
 * transport. Snapshot identity changes with every streamed delta and is never a
 * configuration invalidation signal; adoption eligibility is runtime-domain
 * state the presentation reads from that snapshot directly. */
export interface SessionTransport {
  connection: ConnectionState;
  generation: number;
  publication?: ConfigurationApplication;
}
export function sessionTransport(view: ClientView, sessionId: string): SessionTransport {
  return { connection: view.connection, generation: view.generation, publication: view.configuration?.[sessionId] };
}

export interface SessionConfigurationContext extends SessionTransport {
  port: SessionConfigurationPort;
  /** Whether the *current connected span* has successfully observed native
   * authority — by an authoritative read or a native publication. Native may
   * authoritatively answer that the Session has no application, so this is
   * never inferred from `application` being present. */
  observed: boolean;
  /** Publication order inside this actor; read ownership captures it on entry.
   * Versions alone cannot order absence or failure against a publication. */
  publicationOrder: number;
  readPublicationOrder: number;
  /** The native application observed for this Session in the current
   * connected span of the current connection generation, and the only value
   * the monotonic version comparison ever runs against. `undefined` after an
   * observation means native reported none.
   *
   * A native application version is a runtime counter of one App Server
   * process. It is monotonic inside the generation that produced it and means
   * nothing across a restart — generation 2's version 3 is not older than
   * generation 1's version 100. The comparison is therefore scoped by
   * construction: leaving the connected span empties this field, so the first
   * authoritative observation afterwards has nothing to be compared against and
   * simply becomes the new baseline. */
  application?: ConfigurationApplication;
  /** The last observation of an ended connected span, retained as explicitly
   * stale presentation data alone. It is never a comparison baseline and never
   * becomes authoritative again. */
  staleApplication?: ConfigurationApplication;
  /** Owned by the `observation` region alone: the failure of the read this
   * span owes, until an authoritative observation answers it. */
  readError: string;
  /** Owned by the `adoption` region alone. A successful authoritative read
   * answers the read, never an adoption rejection or an unknown adoption
   * outcome, so the two never share a field. */
  adoptionError: string;
}

export type SessionConfigurationEvent =
  | ({ type: 'TRANSPORT' } & SessionTransport)
  | { type: 'ADOPT'; candidate: AvailableConfiguration }
  /** The native adoption response was classified and now owes the adoption
   * transaction's own authoritative reread. Raised by the `adoption` region. */
  | { type: 'ADOPTION.REREAD' };

const UNCERTAIN_ADOPTION = 'Adoption outcome uncertain. Rereading authority; adoption will not be replayed.';

const newer = (next: ConfigurationApplication, current?: ConfigurationApplication) =>
  !current || BigInt(next.version) > BigInt(current.version);

/** Session configuration observation and Session adoption.
 *
 * They are two independent facts and therefore two regions: whether this
 * browser currently knows the native application, and where an explicit
 * adoption attempt ended up. A successful read can never clear an adoption
 * rejection, and an adoption response can never clear a read failure, because
 * neither region can assign the other's field.
 *
 * The observation region is the whole transport/read obligation, as states:
 *
 * - `offline` — the transport cannot read. Nothing reads, nothing polls and
 *   nothing is authoritative; the last observation of an ended span is at most
 *   stale presentation data. Entering `connected` owes the read that answers
 *   whatever was published meanwhile.
 * - `connected` — one connected span of one connection generation. It is only
 *   ever entered from `offline`, and `offline` holds no current observation, so
 *   entering it *is* the one authoritative read that span owes: its initial
 *   state is `loading`. Nothing else — no presentation, Session attachment or
 *   snapshot change — is needed to recover after a reconnect.
 *   - `loading` — exactly one read in flight: the span's owed read, or the
 *     reread an adoption response owes. A newer adoption reread re-enters it
 *     and so stops the older read actor: a superseded read has no completion
 *     path.
 *   - `ready` — the span's authoritative observation is current.
 *   - `failed` — the span's read failed. It is answered by the next native
 *     publication or adoption reread; never by a timer or by unrelated traffic.
 *
 * A native publication inside the span is folded as the authoritative
 * observation it is: monotonically, in whatever state the region is in, with
 * no read. It answers a failed read; it never ends a read in flight, because
 * that read may be an adoption transaction's own reread.
 *
 * Knownness, currency and actionability are three facts. The span *knows* the
 * application once it has observed authority and no later read failure is
 * outstanding — a reread in flight does not unlearn it. The observation is
 * *current* only in `ready`. Whether adoption is *actionable* additionally
 * needs the runtime's own published eligibility, which the presentation reads
 * from the Runtime Client snapshot; native adoption revalidates all of it.
 *
 * Leaving the connected span — a replaced connection generation, or a
 * transport that can no longer read — retires the span's observation and read
 * failure at that transition and stops the read in flight, so a reply of the
 * ended span can publish neither an application nor a read failure.
 *
 * Adoption stays explicit and native-gated: there is no auto-adopt, no
 * adopt-when-idle queue, no automatic retry and no replay after an unknown
 * outcome — an unknown outcome causes an authoritative reread only. An adoption
 * is submitted on one connection generation, and that generation's replacement
 * ends it as an unknown outcome at once: its reply can no longer arrive, and a
 * late one could only belong to the ended connection, so it settles nothing.
 *
 * One adoption transaction spans both regions, and its span is explicit: the
 * `adoptionInFlight` tag holds from the moment the adoption is submitted,
 * through the native response, until the authoritative reread that response
 * owes has settled, been superseded by a newer read, or been subsumed by the
 * end of its connected span. That terminal point is what the configuration
 * system observes to release a Session actor that no presentation holds. */
export const sessionConfigurationMachine = setup({
  types: {
    context: {} as SessionConfigurationContext,
    events: {} as SessionConfigurationEvent,
    input: {} as { port: SessionConfigurationPort } & SessionTransport,
    tags: {} as 'adoptionInFlight',
  },
  actors: {
    readConfiguration: fromPromise(({ input }: { input: { port: SessionConfigurationPort } }) => input.port.read()),
    adoptConfiguration: fromPromise(({ input }: { input: { port: SessionConfigurationPort; candidate: AvailableConfiguration } }) =>
      input.port.adopt(input.candidate)),
  },
  guards: {
    readSuperseded: ({ context }) => context.readPublicationOrder !== context.publicationOrder,
    canRead: ({ context }) => context.connection === 'connected',
    /** The connected span that owns the current observation ends: the
     * connection generation is replaced, or the transport can no longer read. */
    spanEnds: ({ context, event }) => event.type === 'TRANSPORT'
      && (event.generation !== context.generation || (context.connection === 'connected' && event.connection !== 'connected')),
    /** Inside one connected span, native delivered a distinct publication that
     * is not older than the span's application. Equal versions still establish
     * authority over a read started before that delivery. */
    publicationAdvances: ({ context, event }) => event.type === 'TRANSPORT'
      && event.generation === context.generation && context.connection === 'connected' && event.connection === 'connected'
      && !!event.publication && event.publication !== context.publication
      && (!context.application || !newer(context.application, event.publication)),
    /** A delivery that changes nothing this actor observes is not a transition:
     * client publications for unrelated state never touch this actor. */
    transportChanged: ({ context, event }) => event.type === 'TRANSPORT'
      && (event.connection !== context.connection || event.generation !== context.generation || event.publication !== context.publication),
    generationReplaced: ({ context, event }) => event.type === 'TRANSPORT' && event.generation !== context.generation,
    /** Adoption is offered only against the current span's current
     * observation; native `session/adoptConfiguration` still revalidates it. */
    canAdopt: stateIn({ observation: { connected: 'ready' } }),
    adoptionUncertain: ({ event }) => isOutcomeUncertain((event as unknown as { error: unknown }).error),
  },
  actions: {
    ownRead: assign({ readPublicationOrder: ({ context }) => context.publicationOrder }),
    applyTransport: assign(({ event }) => event.type !== 'TRANSPORT' ? {} : {
      connection: event.connection, generation: event.generation, publication: event.publication,
    }),
    /** The connected span ended, and with it the application-version domain,
     * its observation and its read failure. What it observed stays available
     * only as stale presentation data. Where an adoption attempt stands is a
     * mutation fact and is not touched here. */
    retireObservation: assign({
      staleApplication: ({ context }) => context.application ?? context.staleApplication,
      application: () => undefined,
      observed: () => false,
      readError: () => '',
    }),
    /** Adopt one authoritative read. Inside one connected span a native
     * application version is monotonic, so an obsolete result never regresses a
     * newer one. Across spans there is nothing to compare at all, and this
     * observation establishes the new span's baseline. */
    adoptObservation: assign({
      application: ({ context, event }) => {
        const next = (event as unknown as { output: ConfigurationApplication | null }).output ?? undefined;
        // Only a strictly newer observation of this span outranks a read.
        if (context.application && next && newer(context.application, next)) return context.application;
        return next;
      },
      observed: () => true,
      staleApplication: () => undefined,
      readError: () => '',
    }),
    /** Fold one native publication of the current span; the guard proved it
     * not older than anything the span observed. */
    foldPublication: assign({
      publicationOrder: ({ context }) => context.publicationOrder + 1,
      application: ({ event }) => (event as Extract<SessionConfigurationEvent, { type: 'TRANSPORT' }>).publication,
      observed: () => true,
      staleApplication: () => undefined,
      readError: () => '',
    }),
    recordReadFailure: assign({ readError: ({ event }) => String((event as unknown as { error: unknown }).error) }),
    recordAdoptionFailure: assign({
      adoptionError: ({ event }) => {
        const cause = (event as unknown as { error: unknown }).error;
        return isOutcomeUncertain(cause) ? UNCERTAIN_ADOPTION : String(cause);
      },
    }),
    recordAdoptionSevered: assign({ adoptionError: () => UNCERTAIN_ADOPTION }),
    clearAdoptionFailure: assign({ adoptionError: () => '' }),
  },
}).createMachine({
  id: 'sessionConfiguration',
  context: ({ input }) => ({
    port: input.port, connection: input.connection, generation: input.generation,
    publication: input.publication, observed: false, readError: '', adoptionError: '',
    publicationOrder: 0, readPublicationOrder: 0,
  }),
  type: 'parallel',
  states: {
    /** Does this browser currently know the native Session application? */
    observation: {
      initial: 'offline',
      on: {
        TRANSPORT: [
          // Entering `offline` exits whatever the ended span had reached and
          // stops its read, so a reply of the ended span has no completion
          // path; `offline` then enters the next span at once if this very
          // transport can already read. The transition is deliberately not
          // `reenter`: that would widen its domain to the whole machine and
          // re-enter the `adoption` region too, resetting an adoption in
          // flight. Its domain is this region, whose active descendants are
          // exited and re-entered regardless.
          { guard: 'spanEnds', target: '.offline', actions: ['applyTransport', 'retireObservation'] },
          { guard: 'transportChanged', actions: 'applyTransport' },
        ],
      },
      states: {
        offline: {
          always: { guard: 'canRead', target: 'connected' },
        },
        connected: {
          initial: 'loading',
          on: {
            // A publication is folded where the region stands: it never ends
            // a read in flight, so an adoption's owed reread still settles.
            TRANSPORT: { guard: 'publicationAdvances', actions: ['applyTransport', 'foldPublication'] },
            // Every adoption response owes a fresh authoritative reread. The
            // transition is internal to this region, so it leaves the
            // `adoption` region untouched, yet it re-enters `loading` and so
            // stops any read already in flight.
            'ADOPTION.REREAD': '.loading.adoptionReread',
          },
          states: {
            loading: {
              entry: 'ownRead',
              initial: 'owed',
              invoke: {
                src: 'readConfiguration',
                input: ({ context }) => ({ port: context.port }),
                onDone: [
                  { guard: 'readSuperseded', target: 'ready' },
                  { target: 'ready', actions: 'adoptObservation' },
                ],
                onError: [
                  { guard: 'readSuperseded', target: 'ready' },
                  { target: 'failed', actions: 'recordReadFailure' },
                ],
              },
              states: {
                /** The read this span owes. */
                owed: {},
                /** The authoritative reread an adoption response owes. It is the
                 * last step of that adoption transaction, which ends when this
                 * read settles — or when a newer read supersedes it, or the
                 * span ends, and the read order passes on. */
                adoptionReread: { tags: 'adoptionInFlight' },
              },
            },
            ready: {},
            failed: {
              // A newer native publication answers the failed read.
              on: { TRANSPORT: { guard: 'publicationAdvances', target: 'ready', actions: ['applyTransport', 'foldPublication'] } },
            },
          },
        },
      },
    },

    /** Where an explicit adoption attempt ended up. */
    adoption: {
      initial: 'idle',
      states: {
        idle: { on: { ADOPT: { guard: 'canAdopt', target: 'submitting', actions: 'clearAdoptionFailure' } } },
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
          // The adoption was submitted on the replaced connection: its outcome
          // is unknown from this moment, and leaving this state stops the
          // invoked request, so no late reply of the old connection can settle
          // it. The new span's own owed read is the reread it needs.
          on: { TRANSPORT: { guard: 'generationReplaced', target: 'uncertain', actions: 'recordAdoptionSevered' } },
        },
        rejected: { on: { ADOPT: { guard: 'canAdopt', target: 'submitting', actions: 'clearAdoptionFailure' } } },
        uncertain: { on: { ADOPT: { guard: 'canAdopt', target: 'submitting', actions: 'clearAdoptionFailure' } } },
      },
    },
  },
});

/** Whether one Session adoption transaction is still in flight: submitted and
 * not yet answered natively, or answered and still awaiting the authoritative
 * reread it owes. Its end is the adoption transaction's terminal point. */
export function adoptionInFlight(snapshot: SnapshotFrom<typeof sessionConfigurationMachine>): boolean {
  return snapshot.hasTag('adoptionInFlight');
}

/** Whether the current connected span knows the native application: it has
 * observed authority — possibly that there is no application — and no read
 * failure is outstanding. A reread in flight does not unlearn it. */
export function applicationKnown(snapshot: SnapshotFrom<typeof sessionConfigurationMachine>): boolean {
  return snapshot.matches({ observation: 'connected' }) && snapshot.context.observed && !snapshot.context.readError;
}

/** Whether the known observation is also current: no read is owed. Adoption is
 * offered only here. */
export function applicationCurrent(snapshot: SnapshotFrom<typeof sessionConfigurationMachine>): boolean {
  return snapshot.matches({ observation: { connected: 'ready' } });
}

/** Whether native status is genuinely unavailable: the transport cannot read,
 * the span's read failed, or only an ended span's observation is retained. A
 * connected span's first read in flight claims nothing, and a reread is
 * known. */
export function observationUnavailable(snapshot: SnapshotFrom<typeof sessionConfigurationMachine>): boolean {
  const { readError, staleApplication } = snapshot.context;
  return !applicationKnown(snapshot)
    && (snapshot.matches({ observation: 'offline' }) || !!readError || staleApplication !== undefined);
}
