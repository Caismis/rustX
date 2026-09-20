/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource editor controls; see PROVENANCE.md. */
import { mcpTransport } from '../../bindings/mcp';
import { Badge, SettingsCard } from '../../presentation/settings/SettingsContent';
import css from '../../presentation/settings/SettingsContent.module.css';
import { useState } from 'react';
import type { McpWrite, SourceScope, SourceSettings } from '../../../../protocol/app-server/v14';
import { Button } from '../../presentation/primitives/Button';
import { Names, TextField, UnitForm, type SaveSource } from './controls';

export function StringEntries({ label, value, change, secret = false }: { label: string; value: Record<string, string>; change: (value: Record<string, string>) => void; secret?: boolean }) {
  const [key, setKey] = useState('');
  return <fieldset><legend>{label}</legend>{Object.entries(value).map(([name, entry]) => <div key={name}><TextField label={name} secret={secret} value={entry} change={next => change({ ...value, [name]: next })} /><Button onClick={() => { const next = { ...value }; delete next[name]; change(next); }}>Remove {name}</Button></div>)}<TextField label={`${label} name`} value={key} change={setKey} /><Button disabled={!key || key in value} onClick={() => { change({ ...value, [key]: '' }); setKey(''); }}>Add {label}</Button></fieldset>;
}
export function Integrations({ source, scope, save }: { source: SourceSettings; scope: SourceScope; save: SaveSource }) {
  const [selected, select] = useState(''), [name, setName] = useState('');
  const catalog = scope === 'user' ? source.user_mcp : source.workspace_mcp;
  const current = catalog.authored?.[selected];
  return <section aria-label="MCP definitions"><h3>MCP definitions</h3><p>Definitions are inert. Agent or Workflow selection creates admitted demand before connection. Workspace replaces a whole same-name definition.</p><p>{catalog.path}</p>
    {catalog.diagnostic && <p role="alert">{catalog.diagnostic}</p>}
    <div className={css.rows}>{Object.entries(catalog.authored ?? {}).map(([id, entry]) => <SettingsCard key={id} title={id} meta={<Badge>{mcpTransport(entry.definition)}</Badge>} actions={<Button onClick={() => select(id)}>Edit MCP {id}</Button>}><p className={css.hint}>{entry.definition.url ?? entry.definition.command}</p></SettingsCard>)}</div>
    <TextField label="New MCP identity" value={name} change={setName} /><Button disabled={!name || name in (catalog.authored ?? {})} onClick={() => { select(name); setName(''); }}>Add MCP</Button>
    {selected && <UnitForm<McpWrite> key={selected} title={`MCP ${selected}`} revision={catalog.revision} initial={current ?? { definition: { type: 'stdio', command: '', args: [] }, retained_env: [], retained_headers: [] }} mutation={authored => ({ kind: 'mcp', scope, id: selected, authored })} save={save}>{(value, change) => <>
      <label>Transport<select value={mcpTransport(value.definition)} onChange={e => change({ ...value, definition: e.target.value === 'http' ? { type: 'http', url: '' } : { type: 'stdio', command: '', args: [] }, retained_env: [], retained_headers: [] })}><option value="stdio">stdio</option><option value="http">HTTP</option></select></label>
      {mcpTransport(value.definition) === 'http' ? <TextField label="MCP URL" required value={value.definition.url} change={url => change({ ...value, definition: { ...value.definition, url } })} /> : <><TextField label="MCP command" required value={value.definition.command} change={command => change({ ...value, definition: { ...value.definition, command } })} /><Names label="Arguments" value={value.definition.args ?? []} change={args => change({ ...value, definition: { ...value.definition, args } })} /><TextField label="Working directory" value={value.definition.cwd} change={cwd => change({ ...value, definition: { ...value.definition, cwd: cwd || null } })} /></>}
      <StringEntries label="Environment references ($VARIABLE)" value={value.definition.sensitive_env ?? {}} change={sensitive_env => change({ ...value, definition: { ...value.definition, sensitive_env } })} />
      {mcpTransport(value.definition) === 'http' && <StringEntries label="Header references ($VARIABLE)" value={value.definition.sensitive_headers ?? {}} change={sensitive_headers => change({ ...value, definition: { ...value.definition, sensitive_headers } })} />}
      <StringEntries label="Literal environment" secret value={value.definition.env ?? {}} change={env => change({ ...value, definition: { ...value.definition, env } })} />
      {mcpTransport(value.definition) === 'http' && <StringEntries label="Literal headers" secret value={value.definition.headers ?? {}} change={headers => change({ ...value, definition: { ...value.definition, headers } })} />}
      <Names label="Retain existing environment keys" value={value.retained_env ?? []} change={retained_env => change({ ...value, retained_env })} /><Names label="Retain existing header keys" value={value.retained_headers ?? []} change={retained_headers => change({ ...value, retained_headers })} />
    </>}</UnitForm>}
  </section>;
}
