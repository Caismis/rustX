import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import type { McpWrite, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v38';
import { useTranslation } from '../../../locale/react';
import { mcpTransport } from '../../../bindings/mcp';
import { Button } from '../../../presentation/primitives/Button';
import { UnitOutcomeNotice, useUnitEditing } from '../forms/bridge';
import { documentAuthoring } from '../projection';
import type { PageFocus } from '../machines/navigation';
import { ExtensionDetail } from '../extensions/ExtensionDetail';
import { ConfirmAction } from '../primitives/aria';
import { McpFormFields } from './McpFormFields';
import { formatMcpJson, parseMcpJson } from './json';
import css from './McpPage.module.css';

export function McpEditor({ source, scope, revision, focus, scopeControl, close }: {
  source: SourceSettings; scope: SourceScope; revision?: string; focus: PageFocus['mcp']; scopeControl: ReactNode; close: () => void;
}) {
  const tx = useTranslation();
  const document = documentAuthoring(scope === 'user' ? source.user_mcp : source.workspace_mcp);
  if (document.state !== 'structured') return <><Button onClick={close}>{tx('settings:catalog.mcp')}</Button><p role="alert">{document.state === 'malformed' ? document.diagnostic : tx('settings:source.not-loaded')}</p></>;
  return <Editor source={source} scope={scope} revision={revision} focus={focus} scopeControl={scopeControl} close={close} document={document}/>;
}
function Editor({ source, scope, revision, focus, scopeControl, close, document }: {
  source: SourceSettings; scope: SourceScope; revision?: string; focus: PageFocus['mcp']; scopeControl: ReactNode; close: () => void;
  document: Extract<ReturnType<typeof documentAuthoring<NonNullable<SourceSettings['user_mcp']['authored']>>>, {state:'structured'}>;
}) {
  const tx = useTranslation();
  const [name, setName] = useState(focus.name ?? '');
  const [mode, setMode] = useState<'form'|'json'>(focus.mode === 'json' ? 'json' : 'form');
  const [error, setError] = useState('');
  const [imports, setImports] = useState<Record<string,McpWrite>>({});
  const [pendingImport, setPendingImport] = useState<McpWrite>();
  const unit = useUnitEditing<McpWrite>({ authored: document.document[name], revision: document.revision,
    blank: {definition:{type:'stdio',command:'',args:[]},retained_env:[],retained_headers:[]}, mutation: value => ({kind:'mcp',id:name,authored:value}) });
  const [json, setJson] = useState(() => formatMcpJson(name, unit.displayed));
  const submitted = useRef(false), pendingSave = useRef(false), card = useRef<HTMLFormElement>(null);
  const [invalidFields, setInvalidFields] = useState<Record<string, boolean>>({});
  const validity = useCallback((field: string, valid: boolean) => setInvalidFields(current => {
    if (!!current[field] === !valid) return current;
    return {...current,[field]:!valid};
  }), []);
  const validFields = !Object.values(invalidFields).some(Boolean);
  const value = pendingImport ?? unit.displayed;
  const title = tx(focus.name ? 'settings:mcp.edit' : 'settings:mcp.new');
  const duplicate = !focus.name && !!document.document[name] && !(submitted.current && unit.outcome.kind === 'saved');
  const definition = value.definition;
  const complete = !!name && !duplicate && (definition.type === 'http' || definition.url != null
    ? !!definition.url && /^https?:\/\//.test(definition.url) : !!definition.command?.trim());
  useEffect(() => {
    if (unit.outcome.kind === 'submitting' || unit.outcome.kind === 'committed') submitted.current = true;
    if (submitted.current && focus.mode !== 'permissions' && unit.outcome.kind === 'saved' && unit.outcome.observed) close();
  }, [unit.outcome,close,focus.mode]);
  const chooseMode = (next: 'form'|'json') => {
    if (next === mode) return;
    if (next === 'form') {
      // An unchanged JSON view may represent an unfinished form. Switching
      // views must not force that draft through completed-import validation.
      if (json === formatMcpJson(name, pendingImport ?? value)) { setMode('form'); setError(''); }
      else applyJson();
      return;
    }
    setJson(formatMcpJson(name, pendingImport ?? value));
    setMode(next); setError('');
  };
  const changeName = (next: string) => {
    if (next === name) return;
    // Move only this unsaved creation draft. Never discard or import into a
    // duplicate identity's existing transaction.
    if (!duplicate && unit.draft) { setPendingImport(unit.displayed); unit.discard(); }
    setName(next);
  };
  const importValue = (id: string, next: McpWrite) => {
    // Name selection happens before editing so the actor always owns the
    // resulting draft under its final identity.
    if (focus.name && id && id !== focus.name) {setError(tx('settings:mcp.name-mismatch'));return;}
    const nextName = focus.name ?? (id || name);
    if (nextName !== name && !duplicate && unit.draft) unit.discard();
    const sameIdentity = nextName === name;
    const http = mcpTransport(next.definition) === 'http';
    setPendingImport({...next,
      retained_env:next.retained_env ?? (sameIdentity && !http ? value.retained_env : []),
      retained_headers:next.retained_headers ?? (sameIdentity && http ? value.retained_headers : []),
    });
    setImports({}); setName(nextName); setMode('form'); setError('');
  };
  const applyJson = () => {
    try {
      const parsed = parseMcpJson(json), ids = Object.keys(parsed);
      if (ids.length === 1) importValue(ids[0], parsed[ids[0]]);
      else { setImports(parsed); setError(''); }
    } catch (cause) { setError(String(cause)); }
  };
  useEffect(() => { if (pendingImport && !duplicate && unit.writable) {unit.edit(pendingImport);setPendingImport(undefined);} }, [pendingImport,name,duplicate,unit.writable]);
  useEffect(() => {
    if (pendingSave.current && !unit.writable) pendingSave.current = false;
    if (pendingSave.current && !pendingImport && complete && unit.draft && unit.admitted && unit.writable) {
      pendingSave.current = false; submitted.current = true; unit.submit();
    }
  }, [pendingImport,complete,unit.draft,unit.admitted,unit.writable]);
  let jsonReady = false;
  if (mode === 'json') {
    try {
      const entries = Object.entries(parseMcpJson(json));
      if (entries.length === 1) {
        const id = entries[0][0] || name;
        jsonReady = !!id && (focus.name ? id === focus.name : !document.document[id]);
      }
    } catch { /* Invalid text remains editable; never submit a prior value. */ }
  }
  const blocked = unit.busy || unit.awaitingObservation;
  const canSave = !blocked && unit.admitted && unit.writable && !unit.reviewNeeded && (mode === 'json' ? jsonReady : complete && validFields && unit.draft && !pendingImport && unit.writable);
  const save = () => {
    if (!canSave) return;
    if (mode === 'json') { pendingSave.current = true; applyJson(); }
    else { submitted.current = true; unit.submit(); }
  };
  if (focus.mode === 'permissions' && name) return <ExtensionDetail source={source} scope={scope} revision={revision} models={[]} family="mcp" name={name} backLabel={tx('settings:mcp.back')} onFocus={close}/>;
  return <>
    <div className={css.back}><Button size="sm" variant="ghost" disabled={blocked} onClick={close}>{tx('settings:mcp.back')}</Button></div>
    <div className={css.editorHeader}><div><h3>{title}</h3><p>{tx('settings:mcp.form-help')}</p></div><div className={css.modes} role="group" aria-label={tx('settings:mcp.form')}>
      <Button size="sm" disabled={blocked} aria-pressed={mode === 'form'} variant={mode === 'form' ? 'toolbar' : 'ghost'} onClick={() => chooseMode('form')}>{tx('settings:mcp.form')}</Button><Button size="sm" disabled={blocked || !validFields} aria-pressed={mode === 'json'} variant={mode === 'json' ? 'toolbar' : 'ghost'} onClick={() => chooseMode('json')}>JSON</Button></div></div>
    <form ref={card} tabIndex={-1} className={css.form} aria-label={title} onSubmit={event => {event.preventDefault();save();}}>
      <fieldset disabled={blocked} className={css.formBody}>
        <div className={css.top}>{mode === 'form' && <label>{tx('settings:mcp.name')}<input value={name} placeholder={tx('settings:mcp.name-placeholder')} disabled={!!focus.name || unit.reviewNeeded} onChange={event => changeName(event.target.value.trim())}/></label>}<div className={css.scope}>{tx('settings:mcp.scope')}{scopeControl}</div></div>
        {duplicate && <p role="alert" className={css.error}>{tx('settings:mcp.duplicate')}</p>}
        {mode === 'json' ? <>
          <label>{tx('settings:mcp.json')}<textarea className={css.json} value={json} onChange={event => {setJson(event.target.value);setError('');}}/></label>
          <p className={css.hint}>{tx('settings:mcp.json-help')}</p>
          {Object.keys(imports).length > 1 && <div className={css.actions}>{Object.entries(imports).map(([id,data]) => <Button key={id} onClick={() => importValue(id,data)}>{id}</Button>)}</div>}
        </> : <fieldset disabled={duplicate || !unit.writable} className={css.formBody}>
          <McpFormFields value={value} change={unit.edit} validity={validity}/>
        </fieldset>}
        {unit.override && <div className={css.notice}><p>{tx('settings:mcp.inherited')}</p><Button onClick={unit.override}>{tx('settings:bridge.override')}</Button></div>}
        {unit.reviewNeeded && <div className={css.notice}><p role="status">{tx('settings:mcp.review')}</p><Button onClick={unit.review}>{tx('settings:bridge.use-reviewed-revision')}</Button></div>}
        <UnitOutcomeNotice title={tx('settings:extension-detail.mcp-value',{p0:name})} outcome={unit.outcome}/>
        {error && <p role="alert" className={css.error}>{tx('settings:mcp.invalid-json',{error})}</p>}
      </fieldset>
      <div className={css.footer}>
        {focus.name && unit.authoredPresent && <ConfirmAction label={tx('settings:mcp.remove')} title={tx('settings:mcp.remove-confirm')} description={tx('settings:mcp.remove-help')} confirm={tx('settings:mcp.remove')} tone="destructive" settle={card} disabled={blocked || !unit.admitted} onConfirm={() => {submitted.current=true;unit.submit(true);}}/>}
        <div className={css.footerButtons}>
          <Button variant="primary" type="submit" disabled={!canSave}>{tx('settings:mcp.save')}</Button>
          <Button type="button" disabled={blocked} onClick={() => {if (!duplicate) unit.discard();close();}}>{tx('settings:mcp.cancel')}</Button>
        </div>
      </div>
    </form>
  </>;
}
