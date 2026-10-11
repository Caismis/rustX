import { useEffect, useRef, useState } from 'react';
import type { McpWrite } from '../../../../../protocol/app-server/v44';
import { useTranslation } from '../../../locale/react';
import { mcpTransport } from '../../../bindings/mcp';
import { Choice } from '../primitives/aria';
import { formatArguments, parseArguments, formatReferences, parseReferences } from './form-values';
import css from './McpPage.module.css';

/** Intermediate text stays in the field; every valid value goes straight to
 * the native-source transaction draft. Invalid text blocks Save and JSON. */
function DraftText<T>({ value, format, parse, change, validity, field, label, error, placeholder, multiline = false }: {
  value: T; format: (value: T) => string; parse: (text: string) => T; change: (value: T) => void;
  validity: (field: string, valid: boolean) => void; field: string; label: string; error: string; placeholder: string; multiline?: boolean;
}) {
  const encoded = format(value), reflected = useRef(encoded);
  const [text, setText] = useState(encoded), [invalid, setInvalid] = useState(false);
  useEffect(() => {
    if (encoded === reflected.current) return;
    reflected.current = encoded; setText(encoded); setInvalid(false); validity(field, true);
  }, [encoded, field, validity]);
  useEffect(() => () => validity(field, true), [field, validity]);
  const update = (text: string) => {
    setText(text);
    try {
      const next = parse(text); reflected.current = format(next);
      setInvalid(false); validity(field, true); change(next);
    } catch { setInvalid(true); validity(field, false); }
  };
  return <label>{multiline ? <span className={css.visuallyHidden}>{label}</span> : label}
    {multiline ? <textarea rows={4} value={text} placeholder={placeholder} aria-invalid={invalid || undefined} onChange={event => update(event.target.value)}/>
      : <input value={text} placeholder={placeholder} aria-invalid={invalid || undefined} onChange={event => update(event.target.value)}/>}
    {invalid && <span role="alert" className={css.error}>{error}</span>}
  </label>;
}

export function McpFormFields({ value, change, validity }: {
  value: McpWrite; change: (value: McpWrite) => void; validity: (field: string, valid: boolean) => void;
}) {
  const tx = useTranslation(), definition = value.definition, http = mcpTransport(definition) === 'http';
  const patch = (next: Partial<McpWrite['definition']>) => change({...value, definition: {...definition, ...next}});
  return <>
    <div className={css.short}><Choice label={tx('settings:mcp.type')} value={http ? 'http' : 'stdio'}
      options={[[ 'stdio', tx('settings:mcp.stdio') ],[ 'http', 'HTTP' ]]}
      onChange={next => { if (next !== (http ? 'http' : 'stdio')) change({definition:next === 'http' ? {type:'http',url:''} : {type:'stdio',command:'',args:[]},retained_env:[],retained_headers:[]}); }}/></div>
    {http ? <label>{tx('settings:mcp.url')}<input value={definition.url ?? ''} placeholder={tx('settings:mcp.url-placeholder')} onChange={event => patch({url:event.target.value})}/></label>
      : <>
        <label>{tx('settings:mcp.command')}<input value={definition.command ?? ''} placeholder={tx('settings:mcp.command-placeholder')} onChange={event => patch({command:event.target.value})}/></label>
        <DraftText field="args" label={tx('settings:mcp.arguments')} value={definition.args ?? []} format={formatArguments} parse={parseArguments}
          change={args => patch({args})} validity={validity} error={tx('settings:mcp.args-error')} placeholder={tx('settings:mcp.args-placeholder')}/>
      </>}
    <details className={css.optionalMap} key={http ? 'http' : 'stdio'}><summary>{tx(http ? 'settings:mcp.headers' : 'settings:mcp.env')}</summary>
      <DraftText field={http ? 'headers' : 'env'} label={tx(http ? 'settings:mcp.headers-json' : 'settings:mcp.env-json')} multiline
        value={(http ? definition.sensitive_headers : definition.sensitive_env) ?? {}} format={formatReferences} parse={parseReferences}
        change={references => patch(http ? {sensitive_headers:references} : {sensitive_env:references})} validity={validity}
        error={tx('settings:mcp.references-error')} placeholder={tx(http ? 'settings:mcp.headers-placeholder' : 'settings:mcp.env-placeholder')}/>
    </details>

  </>;
}
