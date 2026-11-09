import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import type { McpWrite, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { mcpTransport } from '../../../bindings/mcp';
import { Button } from '../../../presentation/primitives/Button';
import { DefinitionNotice, UnitOutcomeNotice, useUnitEditing } from '../../settings/forms/bridge';
import { documentAuthoring } from '../../settings/projection';
import type {McpFocus} from '../types';
import { AvailabilityDetail } from '../shared/AvailabilityDetail';
import { Advanced,ConfirmAction } from '../../settings/primitives/aria';
import { McpFormFields } from './McpFormFields';
import { formatMcpJson, parseMcpJson } from './json';
import { JsonCodeEditor } from '../../../presentation/primitives/JsonCodeEditor';
import css from './McpPage.module.css';

export function McpEditor({ source, scope, revision, focus, scopeControl, close }: {
  source: SourceSettings; scope: SourceScope; revision?: string; focus: McpFocus; scopeControl: ReactNode; close: () => void;
}) {
  const tx = useTranslation();
  if (focus.mode === 'permissions' && focus.name) return <><Button onClick={close}>{tx('settings:mcp.back')}</Button><AvailabilityDetail source={source} scope={scope} revision={revision} family="mcp" name={focus.name}/></>;
  const document = documentAuthoring(scope === 'user' ? source.user_mcp : source.workspace_mcp);
  if (document.state !== 'structured') return <><Button onClick={close}>{tx('settings:catalog.mcp')}</Button><p role="alert">{document.state === 'malformed' ? document.diagnostic : tx('settings:source.not-loaded')}</p></>;
  return <Editor source={source} scope={scope} revision={revision} focus={focus} scopeControl={scopeControl} close={close} document={document}/>;
}
function Editor({ source,scope,revision,focus, scopeControl, close, document }: {
  source: SourceSettings; scope: SourceScope; revision?: string; focus: McpFocus; scopeControl: ReactNode; close: () => void;
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
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus({ preventScroll: true }); }, []);
  const [invalidFields, setInvalidFields] = useState<Record<string, boolean>>({});
  const validity = useCallback((field: string, valid: boolean) => setInvalidFields(current => {
    if (!!current[field] === !valid) return current;
    return {...current,[field]:!valid};
  }), []);
  const validFields = !Object.values(invalidFields).some(Boolean);
  const value = pendingImport ?? unit.displayed;
  const title = tx(focus.name ? 'settings:mcp.edit' : 'settings:mcp.new');
  const identityTitle = tx('settings:extension-detail.mcp-value',{p0:name});
  const restoresInherited = scope === 'workspace' && unit.shadowed !== undefined;
  const removalLabel = tx(restoresInherited ? 'settings:bridge.use-global-default-value' : 'settings:bridge.remove-value',{p0:identityTitle});
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
  const cancel = () => { if (!duplicate) unit.discard(); close(); };
  return <>
    <div className={css.back}><Button size="sm" variant="ghost" disabled={blocked} onClick={close}>{tx('settings:mcp.back')}</Button></div>
    <div className={css.editorHeader}><div><h3 ref={heading} tabIndex={-1}>{title}</h3><p>{tx('settings:mcp.form-help')}</p></div><div className={css.modes} role="group" aria-label={tx('settings:mcp.form')}>
      <Button size="sm" disabled={blocked} aria-pressed={mode === 'form'} variant={mode === 'form' ? 'toolbar' : 'ghost'} onClick={() => chooseMode('form')}>{tx('settings:mcp.form')}</Button><Button size="sm" disabled={blocked || !validFields || !unit.writable} aria-pressed={mode === 'json'} variant={mode === 'json' ? 'toolbar' : 'ghost'} onClick={() => chooseMode('json')}>JSON</Button></div></div>
    <form ref={card} tabIndex={-1} className={css.form} aria-label={title} data-definition={unit.definition} onSubmit={event => {event.preventDefault();save();}}>
      <fieldset disabled={blocked} className={css.formBody}>
        <DefinitionNotice unit={unit}/>
        <div className={css.sectionHead}><h4>{tx('settings:mcp.connection')}</h4><div className={css.scope}>{tx('settings:mcp.scope')}{scopeControl}</div></div>
        {duplicate && <p role="alert" className={css.error}>{tx('settings:mcp.duplicate')}</p>}
        {mode === 'json' ? <>
          <JsonCodeEditor label={tx('settings:mcp.json')} value={json} allowEmpty={false} minHeight={280}
            error={error ? tx('settings:mcp.invalid-json',{error}) : ''} onChange={text => {setJson(text);setError('');}}/>
          <p className={css.hint}>{tx('settings:mcp.json-help')}</p>
          {Object.keys(imports).length > 1 && <div className={css.actions}>{Object.entries(imports).map(([id,data]) => <Button key={id} onClick={() => importValue(id,data)}>{id}</Button>)}</div>}
        </> : <fieldset className={css.formBody}>
          <McpFormFields value={value} change={unit.edit} validity={validity} disabled={duplicate || !unit.writable}
            identity={<label>{tx('settings:mcp.name')}<input value={name} placeholder={tx('settings:mcp.name-placeholder')}
              disabled={!!focus.name || unit.reviewNeeded} onChange={event => changeName(event.target.value.trim())}/></label>}/>
        </fieldset>}
        {unit.override && <div className={css.notice}><p>{tx('settings:mcp.inherited')}</p><Button type="button" onClick={unit.override}>{tx('settings:bridge.override')}</Button></div>}
        {unit.reviewNeeded && <div className={css.notice}><p role="status">{tx('settings:mcp.review')}</p><Button onClick={unit.review}>{tx('settings:bridge.use-reviewed-revision')}</Button></div>}
        <UnitOutcomeNotice title={identityTitle} outcome={unit.outcome}/>
        {error && mode !== 'json' && <p role="alert" className={css.error}>{tx('settings:mcp.invalid-json',{error})}</p>}
      </fieldset>
      <div className={css.footer}>
        {focus.name && unit.authoredPresent && <span data-removal={restoresInherited?'override-removal':'authored-removal'}><ConfirmAction label={removalLabel} triggerText={tx(restoresInherited?'settings:models.restore':'settings:copy.delete')} title={tx(restoresInherited?'settings:bridge.use-the-global-default-for-value':'settings:mcp.remove-confirm',{p0:identityTitle})} description={tx(restoresInherited?'settings:forms.restore-help':'settings:mcp.remove-help')} confirm={removalLabel} tone={restoresInherited?'restore':'destructive'} settle={card} disabled={blocked || !unit.admitted} onConfirm={() => {submitted.current=true;unit.submit(true);}}/></span>}
        <div className={css.footerButtons}>
          {unit.intent && <Button size="sm" disabled={blocked} onClick={unit.discard}>{tx('settings:bridge.discard-draft')}</Button>}
          <Button size="sm" variant="outline" disabled={blocked} onClick={cancel}>{tx('settings:mcp.cancel')}</Button>
          <Button size="sm" variant="primary" type="submit" aria-label={tx('settings:mcp.save')} aria-busy={blocked} disabled={!canSave}>{tx(blocked ? 'settings:catalog.saving' : 'settings:mcp.save')}</Button>
        </div>
      </div>
    </form>
    {focus.name&&<Advanced title={tx('resources:mcp.permissions')}><AvailabilityDetail source={source} scope={scope} revision={revision} family="mcp" name={focus.name}/></Advanced>}
  </>;
}
