import { useEffect, useRef, useState, type ReactNode } from 'react';
import type { McpWrite, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v37';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { mcpTransport } from '../../../bindings/mcp';
import { useUnitEditing, UnitOutcomeNotice } from '../forms/bridge';
import { Choice, ConfirmAction } from '../primitives/aria';
import { documentAuthoring } from '../projection';
import type { PageFocus } from '../machines/navigation';
import { ExtensionDetail } from '../extensions/ExtensionDetail';
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
  const submitted = useRef(false);
  const formRef = useRef<HTMLFormElement>(null);
  const unit = useUnitEditing<McpWrite>({ authored: document.document[name], revision: document.revision,
    blank: {definition:{type:'stdio',command:'',args:[]},retained_env:[],retained_headers:[]}, mutation: value => ({kind:'mcp',id:name,authored:value}) });
  const value = unit.displayed;
  const definition = value.definition;
  const transport = mcpTransport(definition);
  const title = tx(focus.name ? 'settings:mcp.edit' : 'settings:mcp.new');
  const duplicate = !focus.name && !!document.document[name];
  const change = (patch: Partial<McpWrite['definition']>) => unit.edit({...value,definition:{...definition,...patch}});
  useEffect(() => { if (submitted.current && unit.outcome.kind === 'saved' && unit.outcome.observed) close(); }, [unit.outcome,close]);
  const save = () => {
    if (!name.trim() || duplicate) return;
    try { parseMcpJson(JSON.stringify(value)); } catch(cause) {setError(String(cause));return;}
    setError(''); submitted.current = true;
    if (unit.draft) unit.submit(); else unit.apply(value);
  };
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
    <div className={css.editorHeader}><div><h3>{title}</h3><p>{tx('settings:mcp.form-help')}</p></div><div className={css.actions} role="group" aria-label={tx('settings:mcp.form')}>
      <Button size="sm" variant={mode === 'form' ? 'toolbar' : 'ghost'} onClick={() => chooseMode('form')}>{tx('settings:mcp.form')}</Button><Button size="sm" variant={mode === 'json' ? 'toolbar' : 'ghost'} onClick={() => chooseMode('json')}>JSON</Button></div></div>
    <form className={css.form} ref={formRef} aria-label={title} onSubmit={event => {event.preventDefault();save();}}>
      <div className={css.top}><label>{tx('settings:mcp.name')}<input value={name} placeholder={tx('settings:mcp.name-placeholder')} required disabled={!!focus.name || unit.draft || unit.busy} onChange={event => setName(event.target.value.trim())}/></label><div className={css.scope}>{tx('settings:mcp.scope')}{scopeControl}</div></div>
      {duplicate && <p role="alert">{tx('settings:mcp.duplicate')}</p>}
      {mode === 'json' ? <>
        <p>{tx('settings:mcp.json-help')}</p><label>{tx('settings:mcp.json')}<textarea className={css.json} value={json} onChange={event => setJson(event.target.value)}/></label>
        <Button type="button" onClick={() => {try {const parsed = parseMcpJson(json);const ids=Object.keys(parsed);if(ids.length===1) importValue(ids[0],parsed[ids[0]]);else setImports(parsed);setError('');}catch(cause){setError(String(cause));}}}>{tx('settings:mcp.apply-json')}</Button>
        {Object.keys(imports).length > 1 && <div className={css.actions}>{Object.entries(imports).map(([id,data]) => <Button key={id} onClick={() => importValue(id,data)}>{id}</Button>)}</div>}
      </> : <fieldset disabled={!name || duplicate || !unit.writable || unit.busy}>
        <div className={css.short}><Choice label={tx('settings:extension-detail.transport')} value={transport} options={[['stdio',tx('settings:mcp.stdio')],['http','HTTP']]} onChange={next => unit.edit({definition:next === 'http' ? {type:'http',url:''} : {type:'stdio',command:'',args:[]},retained_env:[],retained_headers:[]})}/></div>
        {transport === 'http' ? <label>{tx('settings:extension-detail.mcp-url')}<input type="url" required placeholder={tx('settings:mcp.url-placeholder')} value={definition.url ?? ''} onChange={event => change({url:event.target.value})}/></label> : <>
          <label>{tx('settings:extension-detail.mcp-command')}<input required placeholder={tx('settings:mcp.command-placeholder')} value={definition.command ?? ''} onChange={event => change({command:event.target.value})}/></label>
          <ArgumentsField label={tx('settings:extension-detail.arguments')} value={definition.args ?? []} change={args => change({args})}/>
        </>}
        <details key={transport} className={css.optionalMap}><summary>{tx(transport === 'http' ? 'settings:mcp.headers' : 'settings:mcp.env')}</summary>
          <MapField label={tx(transport === 'http' ? 'settings:mcp.headers' : 'settings:mcp.env')} placeholder={tx(transport === 'http' ? 'settings:mcp.headers-placeholder' : 'settings:mcp.env-placeholder')}
            value={(transport === 'http' ? definition.headers : definition.env) ?? {}} change={value => change(transport === 'http' ? {headers:value} : {env:value})}/>
          {((transport === 'http' ? unit.displayed.retained_headers : unit.displayed.retained_env)?.length ?? 0) > 0 && <p>{tx('settings:mcp.retained')}</p>}
        </details>
      </fieldset>}
      {unit.override && <Button onClick={unit.override}>{tx('settings:bridge.override')} {name}</Button>}
      {error && <p role="alert">{tx('settings:mcp.invalid-json',{error})}</p>}
      <UnitOutcomeNotice title={name} outcome={unit.outcome}/>
      {unit.reviewNeeded && <Button onClick={unit.review}>{tx('settings:bridge.use-reviewed-revision')}</Button>}
      <div className={css.footer}>
        {unit.authoredPresent && <ConfirmAction label={tx('settings:mcp.remove')} title={tx('settings:mcp.remove-confirm')} confirm={tx('settings:mcp.remove')} description={tx('settings:mcp.remove-help')} tone="destructive" settle={formRef} disabled={!unit.admitted} onConfirm={() => {submitted.current=true;unit.submit(true);}}/>}
        <Button type="submit" variant="primary" disabled={!name || duplicate || !unit.writable || !unit.admitted || mode === 'json'}>{tx('settings:mcp.save')}</Button>
        <Button disabled={unit.busy} onClick={() => {unit.discard();close();}}>{tx('settings:mcp.cancel')}</Button>
      </div>
    </form>
  </>;
}
function MapField({label,placeholder,value,change}:{label:string;placeholder:string;value:Record<string,string>;change:(value:Record<string,string>)=>void}) {
  const display=(value:Record<string,string>)=>Object.keys(value).length ? JSON.stringify(value,null,2) : '';
  const [text,setText]=useState(display(value));
  const reflected=useRef(JSON.stringify(value));
  useEffect(()=>{const next=JSON.stringify(value);if(next!==reflected.current){setText(display(value));reflected.current=next;}},[value]);
  const [error,setError]=useState('');
  return <label><textarea aria-label={label} placeholder={placeholder} rows={3} spellCheck={false} value={text} onChange={event=>{setText(event.target.value);try {const parsed:unknown=JSON.parse(event.target.value.trim() || '{}');if(!parsed || typeof parsed!=='object' || Array.isArray(parsed) || !Object.values(parsed).every(v=>typeof v==='string')) throw new Error('Expected string values');reflected.current=JSON.stringify(parsed);change(parsed as Record<string,string>);setError('');event.target.setCustomValidity('');}catch(cause){setError(String(cause));event.target.setCustomValidity(String(cause));}}}/>{error && <span role="alert">{error}</span>}</label>;
}

function ArgumentsField({label,value,change}:{label:string;value:string[];change:(value:string[])=>void}) {
  const [text,setText]=useState(value.map(arg=>JSON.stringify(arg)).join(' '));
  const reflected=useRef(JSON.stringify(value));
  useEffect(()=>{const next=JSON.stringify(value);if(next!==reflected.current){setText(value.map(arg=>JSON.stringify(arg)).join(' '));reflected.current=next;}},[value]);
  return <label>{label}<input value={text} onChange={event=>{
    setText(event.target.value);
    try { const tokens=event.target.value.match(/"(?:\\.|[^"\\])*"|\S+/g) ?? [];
      if(tokens.some(token=>token.startsWith('"') && !token.endsWith('"'))) throw new Error('Unclosed quote');
      const args=tokens.map(token=>token.startsWith('"')?JSON.parse(token):token);reflected.current=JSON.stringify(args);change(args);event.target.setCustomValidity('');
    } catch(cause){event.target.setCustomValidity(String(cause));}
  }}/></label>;
}
