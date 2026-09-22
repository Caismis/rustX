/* Copyright (c) 2026 DeepSeek. MIT. Adapted Settings shell; see PROVENANCE.md. */
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { SourceSettings, SourceMutation, SourceScope } from '../../../../protocol/app-server/v18';
import { RpcFailure, isOutcomeUncertain, type AppServerClient } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import { ResourceInventory } from './ResourceInventory';
import { CatalogEditor } from './CatalogEditor';
import { AgentEditor } from './AgentEditor';
import { Integrations } from './Integrations';
import { RootEditor, type RootSection } from './RootEditor';
import { RuntimeEditor } from './RuntimeEditor';
import { UnitForm, type SaveSource } from './controls';
import css from '../../presentation/settings/SettingsContent.module.css';
import { EditorStateContext, SourceContext, SettingsTransactionStore } from './drafts';
import { SettingsPanel } from '../../presentation/settings/SettingsRoot';
import type { ConnectionController } from '../../connection/controller';
import { ConnectionSettings } from './ConnectionSettings';
import { WorkspaceHostError, type ProductHostWorkspaces, type WorkspaceConfigurationReread } from '../../workspaces/host';
import {
  applicationScope, changeBehavior, changeBehaviorLabel, observedResult, observedResultLabel, observedUnitLabel,
  observedUnits, revisionSelector, selectedRevision, settingsLifecycle, settingsLifecycleLabel, settingsTargetKey,
  settingsTargetLabel, settingsTargetScope, unitApplication, type SettingsTarget,
} from './projection';

const sections = [
  ['overview', 'Overview', 'General'], ['general', 'General', 'General'], ['catalog', 'Providers & Models', 'Models'],
  ['root-model', 'Default model', 'Models'], ['policies', 'Tool Policies', 'Agents & Tools'],
  ['root-tools', 'Tools', 'Agents & Tools'], ['root-skills', 'Skill access', 'Agents & Tools'],
  ['root-plugins', 'Plugins', 'Agents & Tools'], ['root-agents', 'Agents & Workflows', 'Agents & Tools'],
  ['agents', 'Agents', 'Agents & Tools'], ['mcp', 'MCP', 'Integrations'], ['python', 'Managed Python', 'Integrations'],
  ['skills', 'Skills', 'Integrations'], ['workflows', 'Workflows', 'Integrations'], ['advanced', 'Server & source diagnostics', 'Advanced'],
] as const;
type Section = typeof sections[number][0] | 'appearance' | 'connection';
const draftStores = new WeakMap<AppServerClient, Map<string, SettingsTransactionStore>>();
interface SettingsProps {
  client: AppServerClient; target: SettingsTarget; host?: ProductHostWorkspaces; onClose?: () => void;
  theme?: 'light' | 'dark'; setTheme?: (theme: 'light' | 'dark') => void;
  connection?: ConnectionController; initialSection?: 'overview' | 'connection';
}
/** Test-only inspection of the live transaction stores of one client, for the
 * regression that proves a confirmed commit leaves no secret-bearing authored
 * payload reachable. Production code never reads it. */
export function settingsTransactionStores(client: AppServerClient): readonly SettingsTransactionStore[] {
  return [...(draftStores.get(client)?.values() ?? [])];
}
export function Settings({ client, target, host, onClose = () => {}, theme = 'light', setTheme, connection, initialSection }: SettingsProps) {
  const transport = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const scope: SourceScope = settingsTargetScope(target);
  const targetKey = settingsTargetKey(target);
  const [section, setSection] = useState<Section>(initialSection ?? 'overview');
  const [source, setSource] = useState<SourceSettings>();
  // Read and write outcomes are separate facts. A successful authoritative read
  // clears only the read error it answers; it never erases a distinct write,
  // conflict or application failure.
  const [readError, setReadError] = useState(''), [writeError, setWriteError] = useState('');
  const [message, setMessage] = useState(''), [busy, setBusy] = useState(false);
  const [targetValid, setTargetValid] = useState(false);
  // Separate facts, never one counter: `epoch` fences the target, authority and
  // connection lifetime of this *presentation*; `reads` is the single ordering
  // identity of every authoritative source read of that presentation.
  const epoch = useRef(0), reads = useRef(0), writing = useRef<number | undefined>(undefined);
  // Level-triggered observation state. `observation` is the latest accepted whole
  // projection; `publications` mirrors the client's per-scope application map;
  // `commits` counts acknowledged source writes whose committed revision no
  // accepted projection has been read after yet; `converging` owns the single
  // convergence worker by the epoch that started it.
  const observation = useRef<SourceSettings | undefined>(undefined), converging = useRef<number | undefined>(undefined);
  // `outstanding` counts authoritative reads whose response has not landed yet,
  // so an owner can await the settlement of a read that superseded its own
  // instead of racing it with a redundant read or releasing the obligation.
  // `waiting` holds every such owner's resolver — the convergence worker and a
  // save whose write-owned reread was superseded can wait at the same time — so
  // a still-outstanding read wakes all of them when it lands. The hand-off is
  // event-driven, never a poll or timer, and never a single-owner slot that one
  // waiter could silently take from another.
  const outstanding = useRef(0), waiting = useRef(new Set<() => void>());
  const publications = useRef(transport.configuration), commits = useRef(0), observedCommits = useRef(0);
  publications.current = transport.configuration;
  const endpoint = transport.endpoint ?? '';
  const identity = JSON.stringify([endpoint, transport.authorityRevision, targetKey]);
  // Stable primitive dependency: a fresh target object literal must not restart
  // the read/convergence lifetime on every parent render.
  const workspaceTargetId = target.kind === 'workspace' ? target.id : undefined;
  // Every unit's editing transaction is owned by one store per target/authority
  // lifetime, so section navigation and editor remounts never orphan settlement.
  let stores = draftStores.get(client);
  if (!stores) { stores = new Map(); draftStores.set(client, stores); }
  let transactions = stores.get(identity);
  if (!transactions) { transactions = new SettingsTransactionStore(); stores.set(identity, transactions); }
  // A section change still remounts the editor subtree (its local picker state
  // must not leak across sections); the transaction store is deliberately not
  // keyed by section so settlement survives that remount.
  const editorKey = identity + ':' + section;
  /** The one read-order invariant every authoritative source read obeys —
   * effect/startup reads, explicit refresh, convergence reads, save recovery
   * reads and Workspace write-owned rereads alike.
   *
   * Ordering is by *reservation*, never by delivery and never by what has
   * already been accepted: each read reserves the next identity when it is
   * initiated, and may publish presentation state — a projection, a read
   * failure, a commit observation — only while it still owns the current read
   * sequence of the current Settings lifetime. Once a newer authoritative read
   * has been initiated for this lifetime, every older read and write-owned
   * reread is superseded and silent, whatever order the responses arrive in. */
  const owns = useCallback((at: number, read_: number) => at === epoch.current && read_ === reads.current, []);
  /** Wake every owner waiting on a read settlement. */
  const wake = useCallback(() => { const woken = [...waiting.current]; waiting.current.clear(); for (const resolve of woken) resolve(); }, []);
  /** Wait for the next authoritative read of this lifetime to settle, however it
   * settles. Used by an owner whose own read was superseded, so the superseding
   * read is awaited rather than raced with a redundant one. */
  const awaitSettlement = useCallback(() => new Promise<void>(resolve => { waiting.current.add(resolve); }), []);
  /** One authoritative read of this exact target. A write acknowledgement never
   * passes through here: it is not a projection and never replaces the read
   * model. */
  const read = useCallback(async (): Promise<SourceSettings> => {
    if (workspaceTargetId !== undefined) {
      if (!host?.configureWorkspace) throw new Error('Workspace Settings requires an authorized Product Host connection.');
      const result = await host.configureWorkspace(workspaceTargetId, endpoint, { kind: 'read' });
      if (result.kind !== 'read') throw new Error('Workspace Host returned a non-read result for a read');
      return result.projection;
    }
    return (await client.request({ method: 'configuration/sourcesRead', params: { target: { kind: 'user' as const } } }, 'source_settings')).projection;
  }, [client, endpoint, host, workspaceTargetId]);
  /** Submit one exact native mutation. On the Workspace Host path the write
   * operation also attempts its own authoritative reread and returns it as an
   * independent outcome, so a failed reread can never erase the confirmed
   * commit. The User Settings path keeps its single authoritative read on the
   * shared `refresh` path. */
  const write = useCallback(async (expected_revision: string, mutation: SourceMutation): Promise<{ acknowledgement: SourceSettings; reread?: WorkspaceConfigurationReread }> => {
    if (workspaceTargetId !== undefined) {
      if (!host?.configureWorkspace) throw new Error('Workspace Settings requires an authorized Product Host connection.');
      const result = await host.configureWorkspace(workspaceTargetId, endpoint, { kind: 'write', expected_revision, mutation });
      if (result.kind !== 'write') throw new Error('Workspace Host returned a non-write result for a write');
      return { acknowledgement: result.commit.acknowledgement, reread: result.commit.reread };
    }
    const acknowledgement = (await client.request({ method: 'configuration/sourceWrite', params: { target: { kind: 'user' as const }, expected_revision, mutation } }, 'source_settings')).projection;
    return { acknowledgement };
  }, [client, endpoint, host, workspaceTargetId]);
  /** A reconcile is not a projection: it re-derives native state and the caller
   * then issues a separate authoritative read. */
  const reconcile = useCallback(async (): Promise<void> => {
    if (workspaceTargetId !== undefined) {
      if (!host?.configureWorkspace) throw new Error('Workspace Settings requires an authorized Product Host connection.');
      const result = await host.configureWorkspace(workspaceTargetId, endpoint, { kind: 'reconcile' });
      if (result.kind !== 'reconcile') throw new Error('Workspace Host returned a non-reconcile result for a reconcile');
      return;
    }
    await client.request({ method: 'configuration/reconcile', params: { target: { kind: 'user' as const } } }, 'configuration_application');
  }, [client, endpoint, host, workspaceTargetId]);
  /** Adopt one whole authoritative projection. Only authoritative reads reach
   * here — an ordinary `refresh` result or a write-owned reread — and only
   * after `owns` has confirmed the read identity reserved at their initiation
   * is still current. A source-write acknowledgement confirms one authoring
   * mutation and supplies its committed revision, but is not an application
   * observation and never replaces the read model. */
  const accept = useCallback((next: SourceSettings) => {
    observation.current = next;
    // The adopted projection is authoritative for every unit's transaction:
    // retire an acknowledged mutation whose committed revision it now carries,
    // even when the editor — or the whole Settings dialog — that submitted it
    // has since unmounted and this store outlived it.
    transactions.observeAll(next);
    setSource(next); setTargetValid(true); setReadError('');
  }, [transactions]);
  /** Adopt an authoritative read owned by an enclosing write operation, under
   * exactly the invariant `owns` states: it commits presentation state only
   * while the identity its operation reserved at initiation is still current.
   * A superseded reread neither replaces the newer projection nor discharges
   * the commit observation its write still owes — that obligation stays with
   * the level-triggered convergence owner. */
  const adopt = useCallback((next: SourceSettings, afterCommits: number, at: number, read_: number) => {
    if (!owns(at, read_)) return false;
    if (afterCommits > observedCommits.current) observedCommits.current = afterCommits;
    accept(next);
    return true;
  }, [accept, owns]);
  /** One authoritative read. Resolves true only when this read's own projection
   * was adopted; a newer *initiated* read wins instead. An adopted read was
   * issued after every commit acknowledged before it started, so it observes
   * them. A superseded read reports nothing at all — neither a projection nor a
   * read failure — because the read that superseded it owns the presentation. */
  const refresh = useCallback(async () => {
    const at = epoch.current, read_ = ++reads.current, afterCommits = commits.current;
    ++outstanding.current;
    try {
      const next = await read();
      if (!owns(at, read_)) return false;
      if (afterCommits > observedCommits.current) observedCommits.current = afterCommits;
      accept(next); return true;
    } catch (cause) {
      if (owns(at, read_)) { setReadError(String(cause)); setTargetValid(false); }
      throw cause;
    } finally {
      if (at === epoch.current) --outstanding.current;
      // Wake every owner waiting on any read settlement, including a failed one:
      // a superseded read is not evidence that the obligation was satisfied.
      wake();
    }
  }, [accept, owns, read, wake]);
  /** The outstanding publication obligation: an application version published
   * for this target's scope that the accepted projection has not reached yet.
   * Level, not edge — it survives reads, acknowledgements and worker restarts
   * until an authoritative read carries at least that version for the scope.
   * With no projection this lifetime, any start owes the first authoritative
   * read (0n), which also retries a failed initial read on later triggers. */
  const obligation = useCallback(() => {
    const held = observation.current, published = publications.current ?? {};
    if (!held) return 0n;
    const scopeKey = applicationScope(held.target), publication = published[scopeKey];
    if (!publication) return undefined;
    const settled = held.application?.scope === scopeKey ? held.application.version : undefined;
    return settled === undefined || BigInt(settled) < BigInt(publication.version) ? BigInt(publication.version) : undefined;
  }, []);
  /** The single convergence worker for the current lifetime. Each pass observes
   * one real outstanding obligation with one authoritative read, then
   * re-evaluates the level: a publication that arrived while the read was
   * outstanding is still owed and drives exactly one more bounded read. A newer
   * one-shot read (explicit refresh, save recovery) may supersede the worker's
   * read; because a superseded read is not a satisfied obligation, the owner
   * keeps the work and waits for the superseding read to settle before
   * re-evaluating, rather than exiting or racing it with a redundant read. It
   * releases ownership only when no publication or acknowledgement obligation
   * remains. No timers, no polling, no write replay. */
  const converge = useCallback(async () => {
    if (converging.current !== undefined) return;
    const owner = epoch.current;
    converging.current = owner;
    try {
      for (;;) {
        // Wait out any authoritative read already in flight before evaluating:
        // the owner must neither race a legitimate newer one-shot read with a
        // redundant read of its own nor act on state that read is about to
        // replace. Settlement (success or failure) wakes this wait; there is no
        // polling, and an epoch change wakes it so a fenced owner can exit.
        while (outstanding.current > 0 && epoch.current === owner) await awaitSettlement();
        const at = epoch.current, required = obligation();
        if (required === undefined && commits.current === observedCommits.current) break;
        const adopted = await refresh();
        if (at !== epoch.current) break;
        // A superseded read is not a satisfied obligation: re-evaluate the level
        // under the same owner, waiting out the read that superseded it.
        if (!adopted) continue;
        if (required === undefined) continue;
        const remaining = obligation();
        if (remaining === undefined || remaining > required) continue;
        // The read was issued after `required` was published, so settling below
        // it is native evidence missing, not a reason to spin. Absent application
        // data is not measurable; a measurably stale application is reported.
        const scopeKey = applicationScope(observation.current!.target);
        const settled = observation.current?.application?.scope === scopeKey ? observation.current.application.version : undefined;
        if (settled === undefined) break;
        setWriteError(`Native ${scopeKey} published application version ${required}, but the authoritative read issued after it settled at ${settled}.`);
        break;
      }
    } catch { /* refresh already owns reporting this failure; a later publication, refresh or reconnect may retry. */ }
    finally { if (converging.current === owner) converging.current = undefined; }
  }, [awaitSettlement, refresh, obligation]);
  useEffect(() => {
    ++epoch.current; observation.current = undefined; converging.current = undefined; outstanding.current = 0;
    // Wake every owner still awaiting a read from the previous lifetime so each
    // can observe the epoch change and release its ownership.
    wake();
    commits.current = 0; observedCommits.current = 0;
    setSource(undefined); setTargetValid(false); setBusy(false); setReadError(''); setWriteError(''); setMessage('');
    if (transport.connection === 'connected') void converge();
    return () => { ++epoch.current; };
  }, [identity, transport.generation, transport.connection, converge, wake]);
  // Native source publications observed on this connection. `owed` is a level,
  // not an edge: until this target's own projection carries at least the version
  // published for its scope, the observation obligation stands — an older
  // acknowledgement landing in between cannot discharge or cancel it.
  const publicationsList = Object.entries(transport.configuration ?? {}).filter(([scopeKey]) => scopeKey.startsWith('source:'));
  const observed = publicationsList.map(([scopeKey, value]) => `${scopeKey}=${value.version}`).join(' ');
  const published = source && publicationsList.find(([scopeKey]) => scopeKey === applicationScope(source.target))?.[1].version;
  const settled = source?.application?.version;
  const owed = published !== undefined && (settled === undefined || BigInt(settled) < BigInt(published));
  useEffect(() => { if (observed) void converge(); }, [observed, converge]);
  useEffect(() => { if (owed) void converge(); }, [owed, settled, converge]);
  const save: SaveSource = async (mutation, expected_revision) => {
    if (writing.current === epoch.current || !targetValid || transport.connection !== 'connected') return undefined;
    const at = epoch.current;
    // A Workspace write owns an authoritative reread. That reread is an
    // ordinary read of this same model, so its ordering identity is reserved
    // now, when the operation that issues it is initiated — never when the
    // enclosing response happens to be delivered. Any newer read initiated for
    // this lifetime therefore supersedes it — whether or not that newer read has
    // been accepted yet — even when the held write response lands last.
    const read_ = workspaceTargetId !== undefined ? ++reads.current : undefined;
    writing.current = at; setBusy(true); setWriteError(''); setMessage('');
    try {
      const outcome = await write(expected_revision, mutation);
      // The acknowledgement confirms exactly this mutation and supplies its
      // committed revision. It is a durable fact about the transaction that
      // submitted it, recorded before the reread: a committed mutation stays
      // committed even if the reread fails — and even if this whole Settings
      // presentation was retired while the acknowledgement was in flight.
      // Presentation retirement may stop an old operation from updating the
      // current UI, but it must never erase or reinterpret a definitive
      // acknowledgement, so the committed revision is returned to the
      // transaction owner regardless. Everything below it is presentation and
      // convergence state of this lifetime alone, and stays fenced.
      const committed = selectedRevision(outcome.acknowledgement, revisionSelector(mutation));
      if (at !== epoch.current) return committed;
      const commit = ++commits.current;
      if (outcome.reread) {
        // The Workspace Host write owns its own authoritative reread. Fence it
        // by the identity reserved at initiation exactly like any other read:
        // it may commit presentation state only while it still owns the current
        // read sequence. A stale or superseded reread never replaces or clears a
        // newer projection, never publishes a read failure the newer read has
        // already superseded, never turns the committed write into a failure,
        // and never triggers a replay. A reread that is still current reports
        // its own failure; a superseded one leaves its outstanding commit
        // obligation to the level-triggered convergence worker.
        const current = outcome.reread.status === 'observed'
          ? adopt(outcome.reread.projection, commit, at, read_!)
          : owns(at, read_!);
        if (!current) {
          // Superseded. A newer authoritative read owns this presentation's read
          // sequence, so this operation waits for that read to settle instead of
          // racing it with a redundant one — the saved notice is only truthful
          // once the current authoritative read has landed, exactly as on the
          // User path. Waiting is bounded by that outstanding read; the commit
          // observation this reread did not discharge stays with the
          // level-triggered convergence owner either way.
          while (at === epoch.current && observedCommits.current < commit && outstanding.current > 0) await awaitSettlement();
          if (at !== epoch.current) return committed;
          void converge();
        } else if (outcome.reread.status === 'observed') {
          void converge();
        } else {
          setReadError(`Saved, but the authoritative reread failed. Application status is uncertain. ${String(outcome.reread.error)}`);
          setTargetValid(false);
        }
        setMessage('Source saved. Native coordination owns application.');
      } else {
        // User Settings owns no in-operation reread; the shared authoritative
        // read path observes the commit here. The saved notice is only truthful
        // once that read has settled, so it waits for the read obligation.
        while (at === epoch.current && observedCommits.current < commit) {
          try { await refresh(); } catch { break; }
        }
        if (at !== epoch.current) return committed;
        setMessage('Source saved. Native coordination owns application.');
        void converge();
      }
      return committed;
    } catch (cause) {
      if (at !== epoch.current) return undefined;
      const conflict = (cause instanceof RpcFailure && cause.error.data?.kind === 'source_conflict') || (cause instanceof WorkspaceHostError && cause.kind === 'source_conflict');
      const uncertain = isOutcomeUncertain(cause) || (cause instanceof WorkspaceHostError && cause.uncertain);
      setWriteError(conflict ? 'Source changed. Your draft and base revision are preserved.' : uncertain ? 'Save outcome uncertain. Rereading authority without replaying the write.' : String(cause));
      try { await refresh(); } catch { /* Keep draft and invalid target until an authoritative read succeeds. */ }
      return undefined;
    } finally { if (writing.current === at) writing.current = undefined; if (at === epoch.current) setBusy(false); }
  };
  const selected = source?.[scope];
  const models = Object.keys(source?.resolved?.models ?? source?.user.authored?.models ?? {});
  const roots = [source?.user_resource_root ? source.user_resource_root + '/skills' : '', source?.workspace_resource_root ? source.workspace_resource_root + '/skills' : ''];
  const lifecycle = settingsLifecycle({ connection: transport.connection, hasSource: !!source, targetValid, readError });
  const editor = selected && <fieldset disabled={busy || !targetValid || transport.connection !== 'connected'} className={css.editor}>
    {section === 'catalog' && <CatalogEditor source={source!} scope={scope} revision={selected.revision} save={save} />}
    {(section === 'general' || section === 'policies') && <RuntimeEditor document={selected.authored ?? {}} resolved={source!.resolved} scope={scope} revision={selected.revision} save={save} policyOnly={section === 'policies'} processPolicyImpacts={source!.process_policy_impacts} />}
    {section.startsWith('root-') && <RootEditor document={selected.authored ?? {}} resolved={source!.resolved} scope={scope} revision={selected.revision} save={save} section={section as RootSection} models={models} skillRoots={roots} />}
    {section === 'mcp' && <Integrations source={source!} scope={scope} save={save} />}
    {section === 'agents' && <AgentEditor source={source!} scope={scope} models={models} save={save} />}
    {['mcp', 'agents', 'python', 'skills', 'workflows'].includes(section) && source?.prospective_resources && <ResourceInventory resources={source.prospective_resources} family={section} scope={scope} />}
  </fieldset>;
  return <SettingsPanel rows={[{ id: 'appearance', label: 'Appearance' }, ...(connection ? [{ id: 'connection', label: 'Connection' }] : []), ...sections.map(([id, label, group]) => ({ id, label, group }))]} activeId={section} onSelect={id => setSection(id as Section)} onClose={onClose}>
    {section === 'connection' && connection ? <ConnectionSettings connection={connection} client={client} /> : section === 'appearance' ?
      <section><h2>Appearance</h2><label>Theme<select aria-label="Theme" value={theme} onChange={event => setTheme?.(event.target.value as 'light' | 'dark')}><option value="light">Light</option><option value="dark">Dark</option></select></label></section> :
      <section className={css.settings} aria-label="Settings" aria-busy={busy}>
        <h2>{settingsTargetLabel(target)}</h2>
        {scope === 'workspace' && <p className={css.hint}>Bound to this exact authorized Workspace. Session focus never retargets this editor.</p>}
        <p role="status" data-lifecycle={lifecycle}>{settingsLifecycleLabel(lifecycle)}</p>
        <Button disabled={busy || transport.connection !== 'connected'} onClick={() => void refresh().catch(() => {})}>Read current sources</Button>
        {readError && <p role="alert">Source read failed. {readError}</p>}
        {writeError && <p role="alert">{writeError}</p>}
        {message && <p role="status">{message}</p>}
        {selected && <p>{selected.path} · Revision: {selected.revision}</p>}
        {selected?.diagnostic && <p role="alert">{selected.diagnostic}</p>}
        {source?.prospective_diagnostic && <p role="status">{source.prospective_diagnostic}</p>}
        {scope === 'workspace' && <p>Remove an override to reset to the global default. An explicit empty selection means none.</p>}
        <h3>{sections.find(([id]) => id === section)?.[1]}</h3>
        <SourceContext value={source}><EditorStateContext value={transactions}><div key={editorKey}>{editor}
          {selected?.diagnostic && !selected.authored && <UnitForm title="Repair malformed source" blank="" revision={selected.revision} save={save} removable={false}
            mutation={document => ({ kind: 'repair_config', document: document ?? '' })}>
            {(value, change) => <label>Replacement TOML<textarea value={value} onChange={event => change(event.target.value)} /></label>}
          </UnitForm>}
        </div></EditorStateContext></SourceContext>
        {section === 'overview' && <p>Definitions and defaults belong to this source. Session selections and explicit adoption belong to each Session.</p>}
        {section === 'advanced' && source && <>
          <h3>Application observation</h3>
          <ul>{observedUnits.map(unit => {
            const result = observedResult(unitApplication(source.application, unit));
            return <li key={unit}>{observedUnitLabel(unit)}: <strong>{observedResultLabel(result)}</strong>
              {result.state === 'failed' && <> — {result.diagnostic}</>}
              {result.state === 'ready' && <> — cache impact {result.impact}</>}
            </li>;
          })}</ul>
          <p>Applied, Preparing, Failed and Restart pending are native observations of this exact source scope. They are never Session adoption and never one global success state.</p>
          <h3>Change behavior</h3>
          <ul>{Object.keys(source.process_policy_impacts).map(key => <li key={key}>{key}: {changeBehaviorLabel(changeBehavior(source.process_policy_impacts, key))}</li>)}</ul>
          <h3>Process bindings</h3><pre>{JSON.stringify(source.process_bindings, null, 2)}</pre>
          {source.application?.units.process_bindings?.status === 'process_restart' && <p role="status">Saved desired values differ from the current process binding. Restart required.</p>}
          {source.application?.units.process_bindings?.status === 'applied' && <p role="status">Saved process policy is active.</p>}
          <details><summary>Resolved preview — source resolution only</summary><pre>{JSON.stringify({ resolved: source.resolved, provenance: source.provenance }, null, 2)}</pre></details>
          <details><summary>Source and application diagnostics</summary><pre>{JSON.stringify(source, null, 2)}</pre></details>
          <Button disabled={busy || !targetValid} onClick={() => { const at = epoch.current; void reconcile().then(() => { if (at === epoch.current) return refresh(); }).catch(cause => { if (at === epoch.current) setWriteError(String(cause)); }); }}>Rescan configuration files</Button>
        </>}
      </section>}
  </SettingsPanel>;
}
