import { useState } from 'react';
import type { ResourceFamily, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v22';
import { Badge } from '../../../presentation/settings/SettingsContent';
import { Button } from '../../../presentation/primitives/Button';
import { NativeFacts } from '../../components/NativeFacts';
import { admitsAuthoring } from '../capability';
import { Advanced, FilterTabs, ResourceList, Search, type ResourceRow } from '../primitives/aria';
import { documentAuthoring, extensionFamilyLabel, type ExtensionFamily } from '../projection';
import type { PageFocus } from '../machines/navigation';
import { TextField } from '../forms/controls';
import {
  allExtensionEntries, collectionDiagnostics, extensionEntries, preparationLabel, relationshipLabel, resourceFamilies,
  selectionLabel, validityLabel, type ExtensionEntry,
} from './inventory';
import { ExtensionDetail } from './ExtensionDetail';
import { NativeExtensions } from './NativeExtensions';
import css from '../../../presentation/settings/SettingsContent.module.css';
import workflow from '../../../presentation/settings/SettingsWorkflow.module.css';

type Filter = 'all' | ExtensionFamily;
const filters: readonly (readonly [Filter, string])[] = [
  ['all', 'All'], ['mcp', 'MCP'], ['skill', 'Skills'], ['agent', 'Agents'],
  ['workflow', 'Workflows'], ['managed_python', 'Python'], ['native', 'Native'],
];

/** The one Extensions resource-management surface.
 *
 * Every resource kind is listed, searched and opened here — MCP servers, Skill
 * packages, named Agents, Workflow programs, Managed Python sources and the
 * native extensions. That does not make them symmetrical: what each kind
 * supports comes from the native capability matrix, so a family with no
 * authoring operation on the wire shows inventory, diagnostics and its
 * supported selection, and shows no Edit, Save or Delete at all.
 *
 * The facts about one resource stay separate facts. A valid definition is not
 * a prepared one, a prepared one is not one the root Agent may use, and one
 * the root Agent may use is not a connected one. Nothing here probes a
 * resource: opening the page issues no preparation and no connection. */
export function ExtensionsPage({ source, scope, revision, models, focus, onFocus }: {
  source: SourceSettings; scope: SourceScope; revision?: string; models: string[];
  focus?: PageFocus['extensions']; onFocus: (focus?: PageFocus['extensions']) => void;
}) {
  const [filter, setFilter] = useState<Filter>('all');
  const [query, setQuery] = useState('');
  if (focus) {
    return <ExtensionDetail source={source} scope={scope} revision={revision} models={models}
      family={focus.family} name={focus.name} onFocus={onFocus} />;
  }
  const entries = filter === 'all' ? allExtensionEntries(source, scope)
    : filter === 'native' ? [] : extensionEntries(source, scope, filter as ResourceFamily);
  // The MCP document is its own native authority with no repair mutation: while
  // it does not parse, no MCP identity can be authored in it.
  const mcp = documentAuthoring(scope === 'user' ? source.user_mcp : source.workspace_mcp);
  const matches = entries.filter(entry => entry.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  return <section aria-label="Extensions">
    <h3>Extensions</h3>
    <p>Everything this Agent can be extended with. Defining a resource is not the same as preparing it, and preparing it is not the same as allowing the root Agent to use it — each is shown as its own fact.</p>
    <FilterTabs label="Extension kinds" value={filter} onChange={setFilter} options={filters}>
      {filter === 'native'
        ? <NativeExtensions source={source} scope={scope} revision={revision} />
        : <>
          <div className={workflow.toolbar}>
            <Search label="Find an extension" value={query} onChange={setQuery} placeholder="Find by identity" />
          </div>
          <ResourceList label={`${filters.find(([id]) => id === filter)![1]} extensions`}
            rows={matches.map(entry => extensionRow(entry, scope))}
            onOpen={id => { const entry = matches.find(item => rowId(item) === id); if (entry) onFocus({ kind: 'extension', family: entry.family, name: entry.name }); }}
            empty={query ? `No extension identity matches ${query}.` : 'No definition of this kind is present in this native projection.'} />
          {filter === 'mcp' && mcp.state === 'malformed' && <>
            <p role="alert" className={css.error}>{mcp.diagnostic}</p>
            <p role="status">MCP editing is unavailable because this document does not parse. Correct {mcp.path}, then rescan configuration files on Advanced.</p>
          </>}
          {filter !== 'all' && admitsAuthoring(filter as ExtensionFamily) && !(filter === 'mcp' && mcp.state !== 'structured')
            && <NewResource family={filter as ExtensionFamily} exists={entries.map(entry => entry.name)}
              open={name => onFocus({ kind: 'extension', family: filter as ExtensionFamily, name })} />}
          {filter !== 'all' && !admitsAuthoring(filter as ExtensionFamily)
            && <p className={css.hint}>This protocol has no operation that authors a {extensionFamilyLabel(filter as ExtensionFamily)} definition. Inventory, diagnostics and root selection are supported; authoring belongs to the native resource source.</p>}
          <CollectionDiagnostics source={source} families={filter === 'all' ? resourceFamilies : [filter as ResourceFamily]} />
        </>}
    </FilterTabs>
  </section>;
}

function rowId(entry: ExtensionEntry): string { return `${entry.family}:${entry.name}`; }

/** One resource row. Each native fact is its own badge, because collapsing
 * them would claim something native never said. */
function extensionRow(entry: ExtensionEntry, scope: SourceScope): ResourceRow {
  const preparation = preparationLabel(entry);
  const selection = selectionLabel(entry);
  return {
    id: rowId(entry), name: entry.name,
    facts: <>
      <Badge>{extensionFamilyLabel(entry.family)}</Badge>
      <Badge>{entry.owner === 'user' ? 'User' : 'Workspace'}</Badge>
      <Badge>{relationshipLabel(entry, scope)}</Badge>
      <Badge tone={entry.valid === undefined ? undefined : entry.valid ? 'success' : 'error'}>{validityLabel(entry)}</Badge>
      {preparation && <Badge>{preparation}</Badge>}
      {selection && <Badge>{selection}</Badge>}
    </>,
    detail: <>
      <p className={css.hint}>{entry.path}</p>
      {entry.shadowed && <p className={css.hint}>Shadows {entry.shadowed}. The losing definition is not parsed or prepared.</p>}
      {entry.diagnostics.map((reason, index) => <p className={css.error} key={index}>{reason}</p>)}
    </>,
  };
}

/** Diagnostics native attributes to a family's source document or collection
 * rather than to any one identity — a resource document that does not parse,
 * an unreadable directory, an exceeded catalog bound. They belong to the
 * family, so they are listed once here and never on the rows of the resources
 * those documents hold. Skill discovery reports its own typed diagnostics,
 * which name packages rather than Skill identities, so they are listed here on
 * the same terms. */
function CollectionDiagnostics({ source, families }: { source: SourceSettings; families: readonly ResourceFamily[] }) {
  const diagnostics = collectionDiagnostics(source, families);
  const skills = families.includes('skill') ? source.prospective_resources?.skill_diagnostics ?? [] : [];
  if (!diagnostics.length && !skills.length) return null;
  return <section aria-label="Source diagnostics">
    <h4>Source diagnostics</h4>
    {diagnostics.map((item, index) => <p className={css.error} key={index}>
      {extensionFamilyLabel(item.subject.family)} source{item.file ? ` ${item.file}` : ''}: {item.reason}
    </p>)}
    {!!skills.length && <Advanced title={`Skill discovery diagnostics (${skills.length})`}><NativeFacts value={skills} /></Advanced>}
  </section>;
}

function NewResource({ family, exists, open }: { family: ExtensionFamily; exists: readonly string[]; open: (name: string) => void }) {
  const [name, setName] = useState('');
  const label = extensionFamilyLabel(family);
  return <div className={css.actions}>
    <TextField label={`New ${label} identity`} value={name} change={setName} />
    <Button disabled={!name || exists.includes(name)} onClick={() => { open(name); setName(''); }}>Add {label}</Button>
  </div>;
}
