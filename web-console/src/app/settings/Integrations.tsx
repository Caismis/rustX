/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource editor controls; see PROVENANCE.md. */
import { mcpTransport } from '../../bindings/mcp';
import { Badge, SettingsCard } from '../../presentation/settings/SettingsContent';
import css from '../../presentation/settings/SettingsContent.module.css';
import { useState } from 'react';
import type { McpWrite, SourceScope, SourceSettings } from '../../../../protocol/app-server/v18';
import { Button } from '../../presentation/primitives/Button';
import { Names, TextField, UnitForm } from './controls';
import { documentAuthoring, inheritedResources } from './projection';

export function StringEntries({ label, value, change, secret = false }: { label: string; value: Record<string, string>; change: (value: Record<string, string>) => void; secret?: boolean }) {
  const [key, setKey] = useState('');
  return <fieldset><legend>{label}</legend>{Object.entries(value).map(([name, entry]) => <div key={name}><TextField label={name} secret={secret} value={entry} change={next => change({ ...value, [name]: next })} /><Button onClick={() => { const next = { ...value }; delete next[name]; change(next); }}>Remove {name}</Button></div>)}<TextField label={`${label} name`} value={key} change={setKey} /><Button disabled={!key || key in value} onClick={() => { change({ ...value, [key]: '' }); setKey(''); }}>Add {label}</Button></fieldset>;
}
/** MCP definitions of exactly one authoring scope.
 *
 * An MCP identity is owned as a whole definition — a Workspace definition
 * shadows the entire same-name User one — so there is no value to merge and no
 * effective document to reconstruct. The native resource inventory names the
 * winning scope of every identity, which is what makes an inherited definition
 * discoverable here without the browser deciding ownership of its own. An
 * inherited identity is presented as a native fact and authors nothing until an
 * explicit override replaces the whole definition. */
export function Integrations({ source, scope }: { source: SourceSettings; scope: SourceScope }) {
  const [selected, select] = useState(''), [name, setName] = useState('');
  const catalog = scope === 'user' ? source.user_mcp : source.workspace_mcp;
  // The MCP document is its own native authority: `rustx.toml` being malformed
  // says nothing about it, and it being malformed says nothing about any other
  // document. Native parses it before every MCP mutation and offers no repair
  // mutation for it, so a document that does not parse admits no editing here.
  const mcp = documentAuthoring(catalog);
  if (mcp.state === 'unavailable') return <p role="alert">Workspace source authority is unavailable.</p>;
  if (mcp.state === 'malformed') return <section aria-label="MCP definitions"><h3>MCP definitions</h3><p>{mcp.path}</p>
    <p role="alert">{mcp.diagnostic}</p>
    <p role="status">MCP editing is unavailable because this document does not parse. Correct the file, then rescan configuration files.</p></section>;
  const authored = mcp.document;
  const inherited = inheritedResources(source, scope, 'mcp', Object.keys(authored));
  const current = authored[selected];
  return <section aria-label="MCP definitions"><h3>MCP definitions</h3><p>Definitions are inert. Agent or Workflow selection creates admitted demand before connection. Workspace replaces a whole same-name definition.</p><p>{mcp.path}</p>
    <div className={css.rows}>{Object.entries(authored).map(([id, entry]) => <SettingsCard key={id} title={id} meta={<Badge>{mcpTransport(entry.definition)}</Badge>} actions={<Button onClick={() => select(id)}>Edit MCP {id}</Button>}><p className={css.hint}>{entry.definition.url ?? entry.definition.command}</p></SettingsCard>)}
      {inherited.map(entry => <SettingsCard key={entry.name} title={entry.name} meta={<><Badge tone={entry.valid ? 'success' : 'error'}>{entry.valid ? 'Valid definition' : 'Invalid definition'}</Badge><Badge>Inherited from User</Badge></>} actions={<Button onClick={() => select(entry.name)}>Override MCP {entry.name}</Button>}><p className={css.hint}>{entry.path} · no override in this Workspace</p></SettingsCard>)}</div>
    <TextField label="New MCP identity" value={name} change={setName} /><Button disabled={!name || name in authored || inherited.some(entry => entry.name === name)} onClick={() => { select(name); setName(''); }}>Add MCP</Button>
    {selected && <UnitForm<McpWrite> key={selected} title={`MCP ${selected}`} revision={mcp.revision} authored={current} blank={{ definition: { type: 'stdio', command: '', args: [] }, retained_env: [], retained_headers: [] }} mutation={authored => ({ kind: 'mcp', id: selected, authored })}>{(value, change) => <>
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
