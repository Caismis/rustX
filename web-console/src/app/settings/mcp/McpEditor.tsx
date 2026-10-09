import { useEffect, useRef, useState, type ReactNode } from 'react';
import type { McpWrite, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v38';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { useUnitEditing } from '../forms/bridge';
import { documentAuthoring } from '../projection';
import type { PageFocus } from '../machines/navigation';
import { ExtensionDetail, McpDefinition } from '../extensions/ExtensionDetail';
import { extensionEntries, relationshipLabel, selectionLabel } from '../extensions/inventory';
import { parseMcpJson } from './json';
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
  const [json, setJson] = useState('');
  const [error, setError] = useState('');
  const [imports, setImports] = useState<Record<string,McpWrite>>({});
  const [pendingImport, setPendingImport] = useState<McpWrite>();
  const unit = useUnitEditing<McpWrite>({ authored: document.document[name], revision: document.revision,
    blank: {definition:{type:'stdio',command:'',args:[]},retained_env:[],retained_headers:[]}, mutation: value => ({kind:'mcp',id:name,authored:value}) });
  const submitted = useRef(false);
  const value = unit.displayed;
  const entry = extensionEntries(source, scope, 'mcp').find(entry => entry.name === name);
  const title = tx(focus.name ? 'settings:mcp.edit' : 'settings:mcp.new');
  const duplicate = !focus.name && !!document.document[name];
  useEffect(() => {
    if (unit.outcome.kind === 'submitting' || unit.outcome.kind === 'committed') submitted.current = true;
    if (submitted.current && focus.mode !== 'permissions' && unit.outcome.kind === 'saved' && unit.outcome.observed) close();
  }, [unit.outcome,close,focus.mode]);
  const chooseMode = (next: 'form'|'json') => {
    if (next === 'json') setJson(JSON.stringify(value,null,2));
    setMode(next);setError('');
  };
  const importValue = (id: string, next: McpWrite) => {
    // Name selection happens before editing so the actor always owns the
    // resulting draft under its final identity.
    if (focus.name && id && id !== focus.name) {setError(tx('settings:mcp.name-mismatch'));return;}
    setPendingImport(next); setImports({}); setName(focus.name ?? (id || name)); setMode('form'); setError('');
  };
  useEffect(() => { if (pendingImport && name && !duplicate && unit.writable) {unit.edit(pendingImport);setPendingImport(undefined);} }, [pendingImport,name,duplicate,unit.writable]);
  if (focus.mode === 'permissions' && name) return <ExtensionDetail source={source} scope={scope} revision={revision} models={[]} family="mcp" name={name} backLabel={tx('settings:mcp.back')} onFocus={close}/>;
  return <>
    <div className={css.breadcrumb}><button type="button" onClick={close}>{tx('settings:catalog.mcp')}</button><span>›</span><span>{focus.name ?? title}</span></div>
    <div className={css.editorHeader}><div><h3>{title}</h3><p>{tx('settings:mcp.form-help')}</p></div><div className={css.modes} role="group" aria-label={tx('settings:mcp.form')}>
      <Button size="sm" aria-pressed={mode === 'form'} variant={mode === 'form' ? 'toolbar' : 'ghost'} onClick={() => chooseMode('form')}>{tx('settings:mcp.form')}</Button><Button size="sm" aria-pressed={mode === 'json'} variant={mode === 'json' ? 'toolbar' : 'ghost'} onClick={() => chooseMode('json')}>JSON</Button></div></div>
    <div className={css.top}><label>{tx('settings:mcp.name')}<input value={name} required disabled={!!focus.name || unit.draft || unit.busy} onChange={event => setName(event.target.value.trim())}/></label><div className={css.scope}>{tx('settings:mcp.scope')}{scopeControl}</div></div>
    {entry && <p>{relationshipLabel(tx, entry, scope)} · {entry.path} · {selectionLabel(tx, entry)}</p>}
    {duplicate && <p role="alert">{tx('settings:mcp.duplicate')}</p>}
    {mode === 'json' ? <>
      <p>{tx('settings:mcp.json-help')}</p><label>{tx('settings:mcp.json')}<textarea className={css.json} value={json} onChange={event => setJson(event.target.value)}/></label>
      <Button onClick={() => {try {const parsed = parseMcpJson(json);const ids=Object.keys(parsed);if(ids.length===1) importValue(ids[0],parsed[ids[0]]);else setImports(parsed);setError('');}catch(cause){setError(String(cause));}}}>{tx('settings:mcp.apply-json')}</Button>
      {Object.keys(imports).length > 1 && <div className={css.actions}>{Object.entries(imports).map(([id,data]) => <Button key={id} onClick={() => importValue(id,data)}>{id}</Button>)}</div>}
    </> : name && !duplicate && <McpDefinition source={source} scope={scope} name={name} family="mcp" models={[]} revision={revision} onFocus={close}/>}
    {error && <p role="alert">{tx('settings:mcp.invalid-json',{error})}</p>}
    <Button disabled={unit.busy} onClick={() => {unit.discard();close();}}>{tx('settings:mcp.cancel')}</Button>
  </>;
}
