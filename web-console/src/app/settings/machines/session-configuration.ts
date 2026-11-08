import { assign, fromPromise, setup, type SnapshotFrom } from 'xstate';
import type { ConfigurationApplication } from '../../../../../protocol/app-server/v76';
import type { AppServerClient } from '../../../client/app-server';
import type { ClientView, ConnectionState } from '../../../client/contracts';

/** The native Session configuration authority of exactly one Session. */
export interface SessionConfigurationPort {
  read(): Promise<ConfigurationApplication | null>;
}

export function createSessionConfigurationPort(client: AppServerClient, sessionId: string): SessionConfigurationPort {
  return {
    read: async () => {const generation=client.getSnapshot().generation;return await client.reads.readApplication(sessionId,()=>client.getSnapshot().generation===generation)??null;},
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
 * configuration invalidation signal. */
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
  /** The failure of the read this span owes, until an authoritative
   * observation answers it. */
  readError: string;
}

export type SessionConfigurationEvent = { type: 'TRANSPORT' } & SessionTransport;

const newer = (next: ConfigurationApplication, current?: ConfigurationApplication) =>
  !current || BigInt(next.version) > BigInt(current.version);

/** Session configuration observation.
 *
 * Native prepares and commits Session configuration automatically; this actor
 * only knows what native last reported. It is the whole transport/read
 * obligation, as states:
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
 *   - `loading` — exactly one read in flight: the span's owed read.
 *   - `ready` — the span's authoritative observation is current.
 *   - `failed` — the span's read failed. It is answered by the next native
 *     publication; never by a timer or by unrelated traffic.
 *
 * A native publication inside the span is folded as the authoritative
 * observation it is: monotonically, in whatever state the actor is in, with
 * no read. It answers a failed read.
 *
 * Leaving the connected span — a replaced connection generation, or a
 * transport that can no longer read — retires the span's observation and read
 * failure at that transition and stops the read in flight, so a reply of the
 * ended span can publish neither an application nor a read failure. */
export const sessionConfigurationMachine = setup({
  types: {
    context: {} as SessionConfigurationContext,
    events: {} as SessionConfigurationEvent,
    input: {} as { port: SessionConfigurationPort } & SessionTransport,
  },
  actors: {
    readConfiguration: fromPromise(({ input }: { input: { port: SessionConfigurationPort } }) => input.port.read()),
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
  },
  actions: {
    ownRead: assign({ readPublicationOrder: ({ context }) => context.publicationOrder }),
    applyTransport: assign(({ event }) => event.type !== 'TRANSPORT' ? {} : {
      connection: event.connection, generation: event.generation, publication: event.publication,
    }),
    /** The connected span ended, and with it the application-version domain,
     * its observation and its read failure. What it observed stays available
     * only as stale presentation data. */
    retireObservation: assign({
      staleApplication: ({ context }) => context.application ?? context.staleApplication,
      application: () => undefined,
      observed: () => false,
      readError: () => '',
    }),
    /** Fold one authoritative read. Inside one connected span a native
     * application version is monotonic, so an obsolete result never regresses a
     * newer one. Across spans there is nothing to compare at all, and this
     * observation establishes the new span's baseline. */
    foldRead: assign({
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
  },
}).createMachine({
  id: 'sessionConfiguration',
  context: ({ input }) => ({
    port: input.port, connection: input.connection, generation: input.generation,
    publication: input.publication, observed: false, readError: '',
    publicationOrder: 0, readPublicationOrder: 0,
  }),
  initial: 'observation',
  states: {
    /** Does this browser currently know the native Session application? */
    observation: {
      initial: 'offline',
      on: {
        TRANSPORT: [
          // Entering `offline` exits whatever the ended span had reached and
          // stops its read, so a reply of the ended span has no completion
          // path; `offline` then enters the next span at once if this very
          // transport can already read.
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
            // A publication is folded where the actor stands.
            TRANSPORT: { guard: 'publicationAdvances', actions: ['applyTransport', 'foldPublication'] },
          },
          states: {
            loading: {
              entry: 'ownRead',
              invoke: {
                src: 'readConfiguration',
                input: ({ context }) => ({ port: context.port }),
                onDone: [
                  { guard: 'readSuperseded', target: 'ready' },
                  { target: 'ready', actions: 'foldRead' },
                ],
                onError: [
                  { guard: 'readSuperseded', target: 'ready' },
                  { target: 'failed', actions: 'recordReadFailure' },
                ],
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
  },
});

/** Whether the current connected span knows the native application: it has
 * observed authority — possibly that there is no application — and no read
 * failure is outstanding. A reread in flight does not unlearn it. */
export function applicationKnown(snapshot: SnapshotFrom<typeof sessionConfigurationMachine>): boolean {
  return snapshot.matches({ observation: 'connected' }) && snapshot.context.observed && !snapshot.context.readError;
}

/** Whether the known observation is also current: no read is owed. */
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
