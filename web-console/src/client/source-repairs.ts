import { readSourceRepairFields } from '../../../protocol/app-server/source-observation';
import { laterRepairSequence, olderRepairSequence, retainSourceRepair, sourceRepairDecision, sourceRepairKey } from '../../../protocol/app-server/source-repair';
import type { ConfigRepairReceipt, SourceTarget } from '../../../protocol/app-server/v57';
import type { ConnectionRequest, SourceRepairObservation } from './contracts';

/** Retained per-source observations are records ordered least recently
 * observed first; the retention rule itself is the protocol's. */
function retainRepair<V>(sources: Readonly<Record<string, V>>, key: string, value: V): [Record<string, V>, V | undefined] {
  const { retained, forgotten } = retainSourceRepair(Object.entries(sources), key, value);
  return [Object.fromEntries(retained), forgotten];
}

/** What one connection's repair observer reads from and writes to its client. */
export interface SourceRepairPort {
  request: ConnectionRequest;
  /** The connection generation and its negotiated authority, if initialized. */
  connection(): { generation: number; authorityId?: string };
  /** Whether `generation` is still the initialized connection of `authorityId`. */
  current(generation: number, authorityId?: string): boolean;
  held(): Readonly<Record<string, SourceRepairObservation>>;
  publish(sourceRepairs: Record<string, SourceRepairObservation>): void;
}

/** The source repair observations of one App Server connection (Issue #464):
 * notices and authoritative reads folded by the shared protocol ordering and
 * retention rules, and the bounded recovery reads an ambiguous notice needs.
 * Source-scoped: never keyed by a Session, and replaced on every connection. */
export class SourceRepairObserver {
  /** Repair observations this connection delivered before its authority was
   * negotiated; attributed to that authority once `initialize` succeeds.
   * Finite, under the same retention as `sourceRepairs`. */
  private unattributed: Record<string, Omit<SourceRepairObservation, 'authorityId'>> = {};
  /** The highest native sequence at which this connection forgot a source.
   * Whatever a source absent from `sourceRepairs` held was current at or
   * before it, so a notice of an absent source ordered at or after it is never
   * older than what was forgotten. One ordered before it may be the forgotten
   * history of an evicted source, or the first notice of a source never seen:
   * it is ambiguous, never applied, and the source is recovered by an
   * authoritative read instead. One value, however many sources came and went. */
  private forgottenThrough = '0';
  /** This connection's Workspace repair reads in flight, each with the
   * highest sequence at which its own source was forgotten while it was in
   * flight. Anything forgotten before a read was sent is older than its
   * answer, since native snapshots after receiving it; anything forgotten
   * meanwhile is compared exactly. Each leaves when its read settles. */
  private reads = new Set<{ key: string; forgotten: string }>();
  /** Workspaces awaiting a recovery read: one entry per source, finite
   * (`retainSourceRepair`), read one at a time. */
  private recoveries: Record<string, SourceTarget> = {};
  private recovering = false;

  constructor(private readonly port: SourceRepairPort) {}

  /** A new connection starts with nothing held or forgotten. */
  connect() {
    this.unattributed = {};
    this.forgottenThrough = '0';
    this.recoveries = {};
  }
  /** Unattributed notices die with the connection that delivered them. */
  disconnect() { this.unattributed = {}; }
  /** `initialize` named the authority: earlier notices belong to it. */
  attribute(authorityId: string) {
    const unattributed = this.unattributed;
    this.unattributed = {};
    for (const observation of Object.values(unattributed)) this.observe({ ...observation, authorityId });
  }
  /** One `configuration/sourceRepairChanged` notice. Until this connection's
   * `initialize` succeeds, the published `authorityId` may still name the
   * authority of a previous connection to the same endpoint. */
  notice(target: SourceTarget, receipt: ConfigRepairReceipt | undefined, sequence: string, authorityId?: string) {
    if (authorityId) { this.observe({ authorityId, target, receipt, sequence }); return; }
    const key = sourceRepairKey(target);
    const held = this.unattributed[key];
    if (!held || olderRepairSequence(held.sequence, sequence)) [this.unattributed] = retainRepair<Omit<SourceRepairObservation, 'authorityId'>>(this.unattributed, key, { target, receipt, sequence });
  }
  /** Applies one native observation of this connection's authority unless
   * the shared ordering rule finds it not newer than what this connection
   * holds, or may have held, of its source; presentation follows receipt ids.
   * An ambiguous notice is recovered by reading its source again. */
  private observe(observation: SourceRepairObservation, forgotten?: string) {
    const key = sourceRepairKey(observation.target);
    const decision = sourceRepairDecision(this.port.held()[key], observation, forgotten, this.forgottenThrough);
    if (decision === 'recover') this.recover(observation.target);
    if (decision !== 'apply') return;
    const [sourceRepairs, evicted] = retainRepair(this.port.held(), key, observation);
    if (evicted) this.forget(evicted);
    this.port.publish(sourceRepairs);
  }
  /** Records that this connection no longer holds `evicted`'s source. */
  private forget({ target, sequence }: SourceRepairObservation) {
    this.forgottenThrough = laterRepairSequence(this.forgottenThrough, sequence);
    const key = sourceRepairKey(target);
    for (const read of this.reads) if (read.key === key) read.forgotten = laterRepairSequence(read.forgotten, sequence);
  }
  /** Reads `target`'s repair observations authoritatively and folds them in:
   * `configuration/sourcesRead` is read-only on the server and answers for
   * exactly the canonical target it was asked about. An answer older than
   * what this connection meanwhile observed of the source — and then forgot —
   * changes nothing, and an answer from a connection that has since ended is
   * dropped. */
  private async read(target: SourceTarget) {
    const { generation, authorityId } = this.port.connection();
    if (!authorityId) return;
    const read = target.kind === 'workspace' ? { key: sourceRepairKey(target), forgotten: '0' } : undefined;
    if (read) this.reads.add(read);
    try {
      const current = () => this.port.current(generation, authorityId);
      const result = await this.port.request({ method: 'configuration/sourcesRead', params: { target } }, 'source_settings', undefined, current);
      const projection = await readSourceRepairFields(result.projection, async (reference, offset) =>
        (await this.port.request({ method: 'configuration/sourceValueRead', params: { target, reference, offset, max_bytes: 65_536 } }, 'source_value_chunk', undefined, current)).chunk, current);
      if (!current()) return;
      const sequence = projection.repair_sequence;
      // The User is never forgotten, so nothing it held can be newer.
      this.observe({ authorityId, target: { kind: 'user' }, receipt: projection.user_repair ?? undefined, sequence }, '0');
      if (read) this.observe({ authorityId, target, receipt: projection.workspace_repair ?? undefined, sequence }, read.forgotten);
    } finally {
      if (read) this.reads.delete(read);
    }
  }
  /** Reads `target` again because what this connection holds of it may be
   * missing: a notice of it was ambiguous, or a Session of its Workspace was
   * attached while the client held nothing of it. Nonblocking, sequential and
   * bounded: one read in flight, one queued entry per source; a source asked
   * for again while its read is in flight is read once more after it, since
   * that read may predate the reason. A failed read proves nothing, and
   * nothing is read once the connection ended. */
  private recover(target: SourceTarget) {
    [this.recoveries] = retainRepair(this.recoveries, sourceRepairKey(target), target);
    if (this.recovering) return;
    this.recovering = true;
    const { generation } = this.port.connection();
    void (async () => {
      try {
        for (let next = Object.entries(this.recoveries)[0]; next && this.port.current(generation); next = Object.entries(this.recoveries)[0]) {
          delete this.recoveries[next[0]];
          await this.read(next[1]).catch(() => undefined);
        }
      } finally {
        this.recovering = false;
      }
    })();
  }
  /** A Session of Workspace `directory` was attached: native now observes
   * that source for this connection. Native announces the current level of
   * each source it newly admits for this connection — a receipt or none — so
   * a source native admits anew needs no read. But native forgets by
   * addressing and this client by observation: once this client has
   * forgotten a source, an absent one may be one native still observes and
   * so never announces again, and it is read rather than assumed healthy. */
  visit(directory: string) {
    const target: SourceTarget = { kind: 'workspace', directory };
    if (olderRepairSequence('0', this.forgottenThrough) && !this.port.held()[sourceRepairKey(target)]) this.recover(target);
  }
}
