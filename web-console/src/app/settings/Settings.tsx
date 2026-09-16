/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-settings-general/SettingsRoot and ui-settings-models/ProviderEditor; see PROVENANCE.md. */
import { RequestPolicy } from './RequestPolicy';
import { useEffect, useRef, useState } from 'react';
import type { MethodResult, ModelCatalogView, Request1, SessionModelConfig } from '../../../../protocol/app-server/v5';
import { RpcFailure, isOutcomeUncertain, type AppServerClient } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import { CatalogEditor } from './CatalogEditor';
import css from './Settings.module.css';

type Read = Extract<MethodResult, { type: 'source_settings' }>;
type Scope = 'user' | 'workspace' | 'session';
export function Settings({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const [state, setState] = useState<Read>();
  const [draft, setDraft] = useState<Read>();
  const [busy, setBusy] = useState(false), [message, setMessage] = useState(''), [error, setError] = useState('');
  const root = useRef<HTMLElement>(null);
  const alive = useRef(true), writing = useRef(false);
  const read = () => client.request({ method: 'settings/sourcesRead', params: { session_id: sessionId } }, 'source_settings');
  const load = async () => {
    setError('');
    try { const value = await read(); if (alive.current) { setState(value); setDraft(structuredClone(value)); } }
    catch (cause) { if (alive.current) setError(String(cause)); }
  };
  useEffect(() => { alive.current = true; void load(); return () => { alive.current = false; }; }, [client, sessionId]);
  const save = async (operation: Request1) => {
    if (writing.current) return;
    const invalid = root.current?.querySelector<HTMLInputElement>('input:invalid');
    if (invalid) { invalid.reportValidity(); return; }
    writing.current = true; setBusy(true); setError(''); setMessage('');
    try {
      if (operation.method === 'settings/selectModel') await client.request(operation, 'settings_replaced');
      else await client.request(operation, 'source_settings');
      const fresh = await read();
      if (alive.current) { setState(fresh); setDraft(structuredClone(fresh)); setMessage('Source committed. Applies on fresh / cold Session resolution. Loaded runtimes and admitted attempts are unchanged.'); }
    } catch (cause) {
      if (!alive.current) return;
      const conflict = cause instanceof RpcFailure && ['source_conflict', 'stale_settings'].includes(cause.error.data?.kind ?? '');
      setError(conflict ? 'Conflict: this scope changed. Your draft is preserved. Review the refreshed effective state, then explicitly retry Save or discard the draft.' : isOutcomeUncertain(cause) ? 'Save outcome uncertain. No request was replayed. Reconnect, then reload authoritative settings before retrying.' : `Save failed: ${String(cause)}`);
      // A repair read is safe; never replay a side-effecting request.
      try { const fresh = await read(); if (alive.current) setState(fresh); } catch { /* Keep the error and draft visible until an explicit reload. */ }
    } finally { writing.current = false; if (alive.current) setBusy(false); }
  };
  const snapshot = client.getSnapshot().views[sessionId]?.snapshot;
  if (!state || !draft) return <section className={css.settings} aria-label="Settings"><h2>Settings</h2>{error ? <><p role="alert">{error}</p><Button onClick={() => void load()}>Retry loading settings</Button></> : <p role="status">Loading settings…</p>}</section>;
  const projection = state.projection, authored = draft.projection;
  const origin = projection.provenance['agent.model.model'];
  const effectiveSource = origin?.kind === 'explicit' ? 'Session' : origin?.kind === 'project' ? 'Workspace' : origin?.kind === 'user' ? 'User' : origin?.kind === 'builtin' ? 'Built-in' : 'Unavailable';
  const selection = (scope: Scope) => scope === 'session' ? draft.session_selection : authored[scope].selection;
  const edit = (scope: Scope, value: SessionModelConfig | null) => setDraft(current => current && (scope === 'session' ? { ...current, session_selection: value } : { ...current, projection: { ...current.projection, [scope]: { ...current.projection[scope], selection: value } } }));
  const submitSelection = (scope: Scope) => {
    const value = selection(scope) ?? null;
    return scope === 'session'
      ? save({ method: 'settings/selectModel', params: { session_id: sessionId, expected_revision: state.session_revision, selection: value } })
      : save({ method: 'settings/sourcesWrite', params: { session_id: sessionId, expected_revision: projection[scope].revision, mutation: { kind: scope === 'user' ? 'user_model' : 'workspace_model', selection: value } } });
  };
  return <section ref={root} className={css.settings} aria-label="Settings"><header><h2>Settings · Provider / Models</h2><Button disabled={busy} onClick={() => void load()}>Reload / discard draft</Button></header>
    {error && <p role="alert" className={css.error}>{error}</p>}{message && <p role="status">{message}</p>}
    <article className={css.card}><h3>Effective · read only</h3><dl><dt>Prospective model</dt><dd>{projection.effective?.model ?? 'Unconfigured or invalid sources'}</dd><dt>Winning source</dt><dd>{effectiveSource}{origin && 'document' in origin ? ` · ${origin.document}` : ''}</dd><dt>Effective output limit</dt><dd>{projection.effective_request?.maxOutputTokens ?? 'Unavailable'}</dd><dt>Effective reasoning profile</dt><dd>{projection.effective_request?.reasoningProfile ?? 'None'}</dd><dt>Current runtime desired model</dt><dd>{snapshot?.model?.configured.model ?? 'No loaded runtime observation'}</dd><dt>Current admitted attempt model</dt><dd>{snapshot?.attempt?.model?.primary.model ?? 'No admitted attempt observed'}</dd></dl>
      <details><summary>Effective request policy and provenance</summary><dl>{Object.entries(projection.effective_request?.requestParams ?? {}).map(([name, value]) => <div key={name}><dt>{name}</dt><dd>{JSON.stringify(value)}</dd></div>)}</dl>{Object.entries(projection.provenance).filter(([name]) => name.startsWith('agent.model.')).map(([name, source]) => <p key={name}>{name}: {source.kind}{'document' in source ? ` · ${source.document}` : ''}</p>)}<p>Runtime incarnation: {client.getSnapshot().views[sessionId]?.target?.runtime_incarnation ?? 'unavailable'}</p></details>
      {!projection.resolution_available && <p role="alert">Native resolution is unavailable. Review catalog and source selections before cold loading.</p>}
      <p className={css.hint}>Native Rust resolves all values and provenance. Source edits do not rewrite already loaded runtimes or frozen attempts.</p></article>
    <h3>Model selection</h3>
    {(['user', 'workspace', 'session'] as const).map(scope => {
      const value = selection(scope), label = scope === 'user' ? 'User' : scope === 'workspace' ? 'Workspace' : 'Session';
      const blocked = busy || (scope === 'workspace' && !projection.workspace.active);
      const revision = scope === 'session' ? state.session_revision : projection[scope].revision;
      return <fieldset key={scope} className={css.card} disabled={blocked}><legend>{label}</legend>
        <p className={css.hint}>{scope === 'session' ? `Session ${sessionId}` : projection[scope].document}<br />Revision: {revision}</p>
        {scope === 'workspace' && <p>{projection.workspace.active ? 'Trusted native project · active' : 'Untrusted native project · inactive and read only. Opening Settings does not grant trust.'}</p>}
        <label>{label} model<select aria-label={`${label} model`} value={value?.model ?? ''} onChange={event => edit(scope, event.target.value ? { model: event.target.value } : null)}><option value="">Unset · inherit lower authorized layer</option>{value && !(projection.catalog.models.models ?? []).some(model => model.model === value.model) && <option value={value.model} disabled>{value.model} · unavailable in current source catalog</option>}{(projection.catalog.models.models ?? []).map(model => <option key={model.model} value={model.model}>{model.model}</option>)}</select></label>
        {scope !== 'session' && <p className={css.hint}>Authored model: {projection[scope].authored?.model ?? 'omitted'} · Authored request fields: {Object.keys(projection[scope].authored?.request_params ?? {}).join(', ') || 'none'}</p>}
        {value && <SelectionDetails value={value} models={projection.catalog.models} change={next => edit(scope, next)} />}
        <div className={css.actions}><Button onClick={() => edit(scope, null)}>Reset {label}</Button><Button variant="primary" onClick={() => void submitSelection(scope)}>{busy ? 'Saving…' : `Save ${label}`}</Button></div>
        <p className={css.hint}>Exact scope only · fresh / cold resolution. Reset removes the selection.</p>
      </fieldset>;
    })}
    <CatalogEditor catalog={{ ...draft.projection.catalog, revision: projection.catalog.revision }} disabled={busy} change={catalog => setDraft(current => current && ({ ...current, projection: { ...current.projection, catalog } }))}
      save={() => void save({ method: 'settings/sourcesWrite', params: { session_id: sessionId, expected_revision: projection.catalog.revision, mutation: { kind: 'catalog', providers: authored.catalog.providers } } })} />
  </section>;
}
function SelectionDetails({ value, models, change }: { value: SessionModelConfig; models: ModelCatalogView; change: (value: SessionModelConfig) => void }) {
  return <details><summary>Whole selection request policy</summary>
    <label>Reasoning profile<select value={value.reasoningProfile ?? ''} onChange={e => change({ ...value, reasoningProfile: e.target.value || null })}><option value="">Catalog default</option>{(models.models ?? []).find(model => model.model === value.model)?.reasoningProfiles?.map(profile => <option key={profile.id} value={profile.id}>{profile.id}</option>)}</select></label>
    <label>Output token override<input type="number" min="1" value={value.maxOutputTokens ?? ''} onChange={e => change({ ...value, maxOutputTokens: e.target.value ? Number(e.target.value) : null })} /></label>
    <RequestPolicy value={value.requestParams ?? {}} change={requestParams => change({ ...value, requestParams })} />
    <label>Summary model<select value={value.summaryModel?.mode === 'explicit' ? value.summaryModel.model : ''} onChange={e => change({ ...value, summaryModel: e.target.value ? { mode: 'explicit', model: e.target.value } : { mode: 'session' } })}><option value="">Follow Session model</option>{(models.models ?? []).map(model => <option key={model.model} value={model.model}>{model.model}</option>)}</select></label>
    <p className={css.hint}>Unspecified profile/output uses native catalog defaults. Existing request and summary policy is retained until a different model is explicitly selected.</p>
  </details>;
}
