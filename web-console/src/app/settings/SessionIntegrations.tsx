import { useEffect, useState } from 'react';
import type { MethodResult, SessionPersistentState } from '../../../../protocol/app-server/v5';
import { type AppServerClient, isOutcomeUncertain } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import css from './Settings.module.css';
type Read = Extract<MethodResult, { type: 'settings' }>;
export function SessionIntegrations({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const [state, setState] = useState<Read>();
  const [draft, setDraft] = useState<SessionPersistentState>();
  const [busy, setBusy] = useState(false), [message, setMessage] = useState('');
  const read = () => client.request({ method: 'settings/read', params: { session_id: sessionId } }, 'settings');
  const load = async () => { try { const next = await read(); setState(next); setDraft(next.settings); } catch { setMessage('Session settings unavailable'); } };
  useEffect(() => { let active = true; void read().then(next => { if (active) { setState(next); setDraft(next.settings); } }).catch(() => { if (active) setMessage('Session settings unavailable'); }); return () => { active = false; }; }, [client, sessionId]);
  const save = async () => {
    if (!state || !draft || busy) return;
    setBusy(true);
    let committed = false;
    const settings: SessionPersistentState = { ...state.settings, skill_paths: draft.skill_paths, no_automatic_skills: draft.no_automatic_skills, no_direct_tools: draft.no_direct_tools, no_builtin_tools: draft.no_builtin_tools, exclude_tools: draft.exclude_tools };
    try { await client.request({ method: 'settings/replace', params: { session_id: sessionId, expected_revision: state.revision, settings } }, 'settings_replaced'); committed = true; const next = await read(); setState(next); setDraft(next.settings); setMessage('Session selection saved for cold resolution; admitted work is unchanged.'); }
    catch (error) { setMessage(committed || isOutcomeUncertain(error) ? 'Outcome uncertain; no replay. Draft retained. Review the authoritative reread before retrying.' : 'Save refused; draft retained. Review the authoritative reread before retrying.'); try { setState(await read()); } catch { /* Retain draft and original revision. */ } }
    finally { setBusy(false); }
  };
  if (!draft || !state) return <p>{message || 'Loading Session selections…'}</p>;
  return <fieldset disabled={busy} className={css.card}><legend>Session Skill / Tool selection</legend><p className={css.hint}>Scope: Session · Target: {sessionId} · Revision: {state.revision}</p>
    <p>Session selection cannot change MCP definitions, native Tool policies, approvals, or Host credentials.</p>
    <label><input type="checkbox" checked={draft.no_automatic_skills} onChange={e => setDraft({ ...draft, no_automatic_skills: e.target.checked })} />No automatic Skills</label>
    <label>Explicit Skill paths (one per line)<textarea value={(draft.skill_paths ?? []).join('\n')} onChange={e => setDraft({ ...draft, skill_paths: e.target.value.split('\n').filter(Boolean) })} /></label>
    <label><input type="checkbox" checked={draft.no_direct_tools} onChange={e => setDraft({ ...draft, no_direct_tools: e.target.checked })} />No direct Tools</label>
    <label><input type="checkbox" checked={draft.no_builtin_tools} onChange={e => setDraft({ ...draft, no_builtin_tools: e.target.checked })} />No built-in Tools</label>
    <label>Tool exclusions (one per line)<textarea value={(draft.exclude_tools ?? []).join('\n')} onChange={e => setDraft({ ...draft, exclude_tools: e.target.value.split('\n').filter(Boolean) })} /></label>
    <Button onClick={() => void load()}>Discard Session selection draft</Button><Button onClick={() => void save()}>Save Session integrations</Button>{message && <p role="status">{message}</p>}
  </fieldset>;
}
