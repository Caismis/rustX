import type { Translate } from '../../../locale/translation';
import { useTranslation } from '../../../locale/react';
import { useState } from 'react';
import type { ResourceFamily, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v23';
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
  const tx = useTranslation();
  const filters: readonly (readonly [Filter, string])[] = [
  ['all', tx('settings:copy.all')], ['mcp', 'MCP'], ['skill', tx('settings:tools-page.skills')], ['agent', tx('settings:copy.agents')],
  ['workflow', tx('settings:copy.workflows')], ['managed_python', tx('settings:copy.python')], ['native', tx('settings:copy.native')],
];
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
  return <section aria-label={tx('settings:extensions-page.extensions')}>
    <h3>{tx('settings:extensions-page.extensions')}</h3>
    <p>{tx('settings:extensions-page.everything-this-agent-can-be-extended-with-defining-a-resource-i')}</p>
    <FilterTabs label={tx('settings:extensions-page.extension-kinds')} value={filter} onChange={setFilter} options={filters}>
      {filter === 'native'
        ? <NativeExtensions source={source} scope={scope} revision={revision} />
        : <>
          <div className={workflow.toolbar}>
            <Search label={tx('settings:extensions-page.find-an-extension')} value={query} onChange={setQuery} placeholder={tx('settings:extensions-page.find-by-identity')} />
          </div>
          <ResourceList label={tx('settings:extensions-page.value-extensions', { p0: filters.find(([id]) => id === filter)![1] })}
            rows={matches.map(entry => extensionRow(tx, entry, scope))}
            onOpen={id => { const entry = matches.find(item => rowId(item) === id); if (entry) onFocus({ kind: 'extension', family: entry.family, name: entry.name }); }}
            empty={query ? tx('settings:copy.no-extension-identity-matches-value', { p0: query }) : tx('settings:copy.no-definition-of-this-kind-is-present-in-this-native-projection')} />
          {filter === 'mcp' && mcp.state === 'malformed' && <>
            <p role="alert" className={css.error}>{mcp.diagnostic ?? tx('settings:source.not-loaded')}</p>
            <p role="status">{tx('settings:extensions-page.mcp-editing-is-unavailable-because-this-document-does-not-parse')}{' '}{mcp.path}{tx('settings:extensions-page.then-rescan-configuration-files-on-advanced')}</p>
          </>}
          {filter !== 'all' && admitsAuthoring(filter as ExtensionFamily) && !(filter === 'mcp' && mcp.state !== 'structured')
            && <NewResource family={filter as ExtensionFamily} exists={entries.map(entry => entry.name)}
              open={name => onFocus({ kind: 'extension', family: filter as ExtensionFamily, name })} />}
          {filter !== 'all' && !admitsAuthoring(filter as ExtensionFamily)
            && <p className={css.hint}>{tx('settings:extension-detail.this-protocol-has-no-operation-that-authors-a')}{' '}{extensionFamilyLabel(tx, filter as ExtensionFamily)} {tx('settings:extensions-page.definition-inventory-diagnostics-and-root-selection-are-supporte')}</p>}
          <CollectionDiagnostics source={source} families={filter === 'all' ? resourceFamilies : [filter as ResourceFamily]} />
        </>}
    </FilterTabs>
  </section>;
}

function rowId(entry: ExtensionEntry): string { return `${entry.family}:${entry.name}`; }

/** One resource row. Each native fact is its own badge, because collapsing
 * them would claim something native never said. */
function extensionRow(tx: Translate, entry: ExtensionEntry, scope: SourceScope): ResourceRow {
  const preparation = preparationLabel(tx, entry);
  const selection = selectionLabel(tx, entry);
  return {
    id: rowId(entry), name: entry.name,
    facts: <>
      <Badge>{extensionFamilyLabel(tx, entry.family)}</Badge>
      <Badge>{entry.owner === 'user' ? tx('settings:extension-detail.user') : tx('settings:extension-detail.workspace')}</Badge>
      <Badge>{relationshipLabel(tx, entry, scope)}</Badge>
      <Badge tone={entry.valid === undefined ? undefined : entry.valid ? 'success' : 'error'}>{validityLabel(tx, entry)}</Badge>
      {preparation && <Badge>{preparation}</Badge>}
      {selection && <Badge>{selection}</Badge>}
    </>,
    detail: <>
      <p className={css.hint}>{entry.path}</p>
      {entry.shadowed && <p className={css.hint}>{tx('settings:extensions-page.shadows')}{' '}{entry.shadowed}{tx('settings:extensions-page.the-losing-definition-is-not-parsed-or-prepared')}</p>}
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
  const tx = useTranslation();
  const diagnostics = collectionDiagnostics(source, families);
  const skills = families.includes('skill') ? source.prospective_resources?.skill_diagnostics ?? [] : [];
  if (!diagnostics.length && !skills.length) return null;
  return <section aria-label={tx('settings:extensions-page.source-diagnostics')}>
    <h4>{tx('settings:extensions-page.source-diagnostics')}</h4>
    {diagnostics.map((item, index) => <p className={css.error} key={index}>
      {extensionFamilyLabel(tx, item.subject.family)} {tx('settings:extensions-page.source')}{item.file ? tx('settings:extensions-page.value', { p0: item.file }) : ''}: {item.reason}
    </p>)}
    {!!skills.length && <Advanced title={tx('settings:extensions-page.skill-discovery-diagnostics-value', { p0: skills.length })}><NativeFacts value={skills} /></Advanced>}
  </section>;
}

function NewResource({ family, exists, open }: { family: ExtensionFamily; exists: readonly string[]; open: (name: string) => void }) {
  const tx = useTranslation();
  const [name, setName] = useState('');
  const label = extensionFamilyLabel(tx, family);
  return <div className={css.actions}>
    <TextField label={tx('settings:extensions-page.new-value-identity', { p0: label })} value={name} change={setName} />
    <Button disabled={!name || exists.includes(name)} onClick={() => { open(name); setName(''); }}>{tx('settings:extensions-page.add')}{' '}{label}</Button>
  </div>;
}
