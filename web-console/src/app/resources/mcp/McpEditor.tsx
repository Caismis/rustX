import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import type { McpWrite, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v78';
import { useTranslation } from '../../../locale/react';
import { mcpTransport } from '../../../bindings/mcp';
import { Button } from '../../../presentation/primitives/Button';
import { DefinitionNotice, UnitOutcomeNotice, useUnitEditing } from '../../settings/forms/bridge';
import { documentAuthoring } from '../../settings/projection';
import type { McpEditorDraft, McpFieldBuffers, McpFocus } from '../types';
import { AvailabilityDetail } from '../shared/AvailabilityDetail';
import { Advanced,ConfirmAction } from '../../settings/primitives/aria';
import { McpFormFields } from './McpFormFields';
import { formatMcpJson, parseMcpJson } from './json';
import { JsonCodeEditor } from '../../../presentation/primitives/JsonCodeEditor';
import css from './McpPage.module.css';

interface EditorProps {
  source: SourceSettings; scope: SourceScope; revision?: string; focus: McpFocus; scopeControl: ReactNode; close: () => void;
  editorDraft?: McpEditorDraft; onEditorDraftChange: (draft?: McpEditorDraft) => void;
}

export function McpEditor(props: EditorProps) {
  const { source, scope, revision, focus, close } = props;
  const tx = useTranslation();
  if (focus.mode === 'permissions' && focus.name) return <><Button onClick={close}>{tx('settings:mcp.back')}</Button><AvailabilityDetail source={source} scope={scope} revision={revision} family="mcp" name={focus.name}/></>;
  const document = documentAuthoring(scope === 'user' ? source.user_mcp : source.workspace_mcp);
  if (document.state !== 'structured') return <><Button onClick={close}>{tx('settings:catalog.mcp')}</Button><p role="alert">{document.state === 'malformed' ? document.diagnostic : tx('settings:source.not-loaded')}</p></>;
  return <Editor {...props} document={document}/>;
}
function Editor({ source, scope, revision, focus, scopeControl, close, editorDraft, onEditorDraftChange, document }: EditorProps & {
  document: Extract<ReturnType<typeof documentAuthoring<NonNullable<SourceSettings['user_mcp']['authored']>>>, {state:'structured'}>;
}) {
  const tx = useTranslation();
  const [name, setName] = useState(focus.name ?? editorDraft?.name ?? '');
  const [mode, setMode] = useState<McpEditorDraft['mode']>(editorDraft?.mode ?? (focus.mode === 'json' ? 'json' : 'form'));
  const [error, setError] = useState('');
  const [imports, setImports] = useState<Record<string,McpWrite>>({});
  const [pendingImport, setPendingImport] = useState(editorDraft?.pendingImport);
  const [jsonEdit, setJsonEdit] = useState(editorDraft?.json);
  const [formReset, setFormReset] = useState(0);
  const authored = Object.hasOwn(document.document, name) ? document.document[name] : undefined;
  const unit = useUnitEditing<McpWrite>({ draftKey: focus.name ? undefined : JSON.stringify(['create-mcp', name]),
    authored, revision: document.revision,
    onSaved: () => { onEditorDraftChange(undefined); close(); },
    blank: {definition:{type:'stdio',command:'',args:[]},retained_env:[],retained_headers:[]}, mutation: value => ({kind:'mcp',id:name,authored:value}) });
  const pendingSave = useRef(false), card = useRef<HTMLFormElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus({ preventScroll: true }); }, []);
  const [invalidFields, setInvalidFields] = useState<McpFieldBuffers>(editorDraft?.invalidFields ?? {});
  const validity = useCallback((field: keyof McpFieldBuffers, invalidText?: string) => setInvalidFields(current => {
    if (current[field] === invalidText) return current;
    const next = { ...current };
    if (invalidText === undefined) delete next[field];
    else next[field] = invalidText;
    return next;
  }), []);
  const value = pendingImport?.value ?? unit.displayed;
  const validFields = mcpTransport(value.definition) === 'http'
    ? invalidFields.headers === undefined : invalidFields.args === undefined && invalidFields.env === undefined;
  const formatted = mode === 'json' ? formatMcpJson(name, jsonEdit?.pinned ? authored ?? value : value) : '';
  const json = jsonEdit?.text ?? formatted;
  const reviewNeeded = unit.reviewNeeded || !!pendingImport && pendingImport.revision !== document.revision;
  const title = tx(focus.name ? 'settings:mcp.edit' : 'settings:mcp.new');
  const identityTitle = tx('settings:extension-detail.mcp-value',{p0:name});
  const restoresInherited = scope === 'workspace' && unit.shadowed !== undefined;
  const removalLabel = tx(restoresInherited ? 'settings:bridge.use-global-default-value' : 'settings:bridge.remove-value',{p0:identityTitle});
  const duplicate = !focus.name && !!authored && !(unit.submission && unit.outcome.kind === 'saved');
  const definition = value.definition;
  const complete = !!name && !duplicate && (definition.type === 'http' || definition.url != null
    ? !!definition.url && /^https?:\/\//.test(definition.url) : !!definition.command?.trim());
  const chooseMode = (next: 'form'|'json') => {
    if (next === mode) return;
    if (next === 'form') {
      // An unchanged JSON view may represent an unfinished form. Switching
      // views must not force that draft through completed-import validation.
      if (json === formatted) {
        if (jsonEdit?.pinned) unit.discard();
        setJsonEdit(undefined); setMode('form'); setError('');
      }
      else applyJson();
      return;
    }
    setMode(next); setError('');
  };
  const updateJson = (text: string) => {
    if (!unit.writable || duplicate) return;
    // Even invalid text holds an intent at the unit's exact source revision.
    // Pin the current value until this text can replace it as a typed draft.
    const pinned = jsonEdit?.pinned ?? !unit.draft;
    if (!jsonEdit && pinned) unit.edit(unit.displayed);
    setJsonEdit({ text, pinned });
    setImports({}); setError('');
  };
  const changeName = (next: string) => {
    if (next === name) return;
    // Move only this unsaved creation draft. Its transaction is separate
    // from an existing server's editor even if the names overlap.
    if (unit.draft) { setPendingImport({ value: unit.displayed, revision: document.revision }); unit.discard(); }
    setName(next);
  };
  const importValue = (id: string, next: McpWrite) => {
    // Name selection happens before editing so the actor always owns the
    // resulting draft under its final identity.
    if (reviewNeeded) return;
    if (focus.name && id && id !== focus.name) {setError(tx('settings:mcp.name-mismatch'));return;}
    const nextName = focus.name ?? (id || name);
    if (nextName !== name && unit.draft) unit.discard();
    const sameIdentity = nextName === name;
    const http = mcpTransport(next.definition) === 'http';
    // A raw-only transaction's seed may predate review. Retain credentials
    // from the currently observed definition unless the draft names them.
    const retain = jsonEdit?.pinned ? authored : value;
    setPendingImport({revision: document.revision, value: {...next,
      retained_env:next.retained_env ?? (sameIdentity && !http ? retain?.retained_env ?? [] : []),
      retained_headers:next.retained_headers ?? (sameIdentity && http ? retain?.retained_headers ?? [] : []),
    }});
    setImports({}); setJsonEdit(undefined); setName(nextName); setMode('form'); setError('');
  };
  const applyJson = () => {
    if (reviewNeeded) return;
    try {
      const parsed = parseMcpJson(json), ids = Object.keys(parsed);
      if (ids.length === 1) importValue(ids[0], parsed[ids[0]]);
      else { setImports(parsed); setError(''); }
    } catch (cause) { setError(String(cause)); }
  };
  useEffect(() => {
    if (pendingImport && !duplicate && unit.writable && !reviewNeeded) {
      unit.edit(pendingImport.value); setPendingImport(undefined);
    }
  }, [pendingImport, name, duplicate, unit.writable, reviewNeeded]);
  useEffect(() => {
    if (pendingSave.current && (!unit.writable || reviewNeeded)) pendingSave.current = false;
    if (pendingSave.current && !pendingImport && complete && unit.draft && unit.admitted && unit.writable) {
      pendingSave.current = false; unit.submit();
    }
  }, [pendingImport, complete, unit.draft, unit.admitted, unit.writable, reviewNeeded]);
  let jsonReady = false;
  const jsonDirty = json !== formatted || unit.draft && !jsonEdit?.pinned;
  if (mode === 'json' && jsonDirty) {
    try {
      const entries = Object.entries(parseMcpJson(json));
      if (entries.length === 1) {
        const id = entries[0][0] || name;
        jsonReady = !!id && (focus.name ? id === focus.name : !Object.hasOwn(document.document, id));
      }
    } catch { /* Invalid text remains editable; never submit a prior value. */ }
  }
  const blocked = unit.busy || unit.awaitingObservation;
  const canWrite = !blocked && unit.admitted && !reviewNeeded;
  const canSave = canWrite && unit.writable && (mode === 'json' ? jsonReady : complete && validFields && unit.draft && !pendingImport);
  const save = () => {
    if (!canSave) return;
    if (mode === 'json') { pendingSave.current = true; applyJson(); }
    else unit.submit();
  };
  const resetBuffers = () => {
    pendingSave.current = false;
    setJsonEdit(undefined); setPendingImport(undefined); setImports({}); setError('');
    setInvalidFields({});
    setFormReset(current => current + 1);
  };
  const discard = () => { resetBuffers(); unit.discard(); onEditorDraftChange(undefined); };
  const review = () => {
    pendingSave.current = false;
    if (pendingImport) setPendingImport({ ...pendingImport, revision: document.revision });
    unit.review();
  };
  const cancel = () => { discard(); close(); };
  const back = () => {
    const retain = jsonEdit || pendingImport || !validFields || !focus.name && (name || unit.intent);
    onEditorDraftChange(retain ? { name, mode, json: jsonEdit, pendingImport, invalidFields: validFields ? undefined : invalidFields } : undefined);
    close();
  };
  return <>
    <div className={css.back}><Button size="sm" variant="ghost" disabled={blocked} onClick={back}>{tx('settings:mcp.back')}</Button></div>
    <div className={css.editorHeader}><div><h3 ref={heading} tabIndex={-1}>{title}</h3><p>{tx('settings:mcp.form-help')}</p></div><div className={css.modes} role="group" aria-label={tx('settings:mcp.form')}>
      <Button size="sm" disabled={blocked} aria-pressed={mode === 'form'} variant={mode === 'form' ? 'toolbar' : 'ghost'} onClick={() => chooseMode('form')}>{tx('settings:mcp.form')}</Button><Button size="sm" disabled={blocked || duplicate || !validFields || !unit.writable} aria-pressed={mode === 'json'} variant={mode === 'json' ? 'toolbar' : 'ghost'} onClick={() => chooseMode('json')}>JSON</Button></div></div>
    <form ref={card} tabIndex={-1} className={css.form} aria-label={title} data-definition={unit.definition} onSubmit={event => {event.preventDefault();save();}}>
      <fieldset disabled={blocked} className={css.formBody}>
        <DefinitionNotice unit={unit}/>
        <div className={css.sectionHead}><h4>{tx('settings:mcp.connection')}</h4><div className={css.scope}>{tx('settings:mcp.scope')}{scopeControl}</div></div>
        {duplicate && <p role="alert" className={css.error}>{tx('settings:mcp.duplicate')}</p>}
        {mode === 'json' ? <>
          <JsonCodeEditor label={tx('settings:mcp.json')} value={json} allowEmpty={false} minHeight={280}
            error={error ? tx('settings:mcp.invalid-json',{error}) : ''} onChange={updateJson}/>
          <p className={css.hint}>{tx('settings:mcp.json-help')}</p>
          {Object.keys(imports).length > 1 && <div className={css.actions}>{Object.entries(imports).map(([id,data]) => <Button key={id} disabled={reviewNeeded} onClick={() => importValue(id,data)}>{id}</Button>)}</div>}
        </> : <fieldset className={css.formBody}>
          <McpFormFields key={formReset} value={value} change={unit.edit} invalidFields={invalidFields} validity={validity} disabled={duplicate || !unit.writable}
            identity={<label>{tx('settings:mcp.name')}<input value={name} placeholder={tx('settings:mcp.name-placeholder')}
              disabled={!!focus.name || reviewNeeded} onChange={event => changeName(event.target.value.trim())}/></label>}/>
        </fieldset>}
        {unit.override && <div className={css.notice}><p>{tx('settings:mcp.inherited')}</p><Button type="button" onClick={unit.override}>{tx('settings:bridge.override')}</Button></div>}
        {reviewNeeded && <div className={css.notice}><p role="status">{tx(unit.committed && !unit.draft && !pendingImport && !jsonEdit ? 'settings:forms.committed-changed-elsewhere' : 'settings:mcp.review')}</p><Button onClick={review}>{tx('settings:bridge.use-reviewed-revision')}</Button></div>}
        <UnitOutcomeNotice title={identityTitle} outcome={unit.outcome}/>
        {error && mode !== 'json' && <p role="alert" className={css.error}>{tx('settings:mcp.invalid-json',{error})}</p>}
      </fieldset>
      <div className={css.footer}>
        {focus.name && unit.authoredPresent && <span data-removal={restoresInherited?'override-removal':'authored-removal'}><ConfirmAction label={removalLabel} triggerText={tx(restoresInherited?'settings:models.restore':'settings:copy.delete')} title={tx(restoresInherited?'settings:bridge.use-the-global-default-for-value':'settings:mcp.remove-confirm',{p0:identityTitle})} description={tx(restoresInherited?'settings:forms.restore-help':'settings:mcp.remove-help')} confirm={removalLabel} tone={restoresInherited?'restore':'destructive'} settle={card} disabled={!canWrite} onConfirm={() => {
          if (!canWrite) return;
          unit.submit(true);
        }}/></span>}
        <div className={css.footerButtons}>
          {(unit.intent || pendingImport || jsonEdit || !validFields) && <Button size="sm" disabled={blocked} onClick={discard}>{tx('settings:bridge.discard-draft')}</Button>}
          <Button size="sm" variant="outline" disabled={blocked} onClick={cancel}>{tx('settings:mcp.cancel')}</Button>
          <Button size="sm" variant="primary" type="submit" aria-label={tx('settings:mcp.save')} aria-busy={blocked} disabled={!canSave}>{tx(blocked ? 'settings:catalog.saving' : 'settings:mcp.save')}</Button>
        </div>
      </div>
    </form>
    {focus.name&&<Advanced title={tx('resources:mcp.permissions')}><AvailabilityDetail source={source} scope={scope} revision={revision} family="mcp" name={focus.name}/></Advanced>}
  </>;
}
