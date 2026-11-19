import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Tab, TabList, TabPanel, Tabs } from 'react-aria-components';
import type { SourceScope, SourceSettings, WorkflowToolSource } from '../../../../../protocol/app-server/v80';
import { useTranslation } from '../../../locale/react';
import { CatalogSearch } from '../../../presentation/catalog/CatalogControls';
import catalog from '../../../presentation/catalog/Catalog.module.css';
import { Button } from '../../../presentation/primitives/Button';
import { documentAuthoring } from '../../settings/projection';
import type { LoadMcpTools, McpEditorDraft, McpFocus } from '../types';
import { McpPermissionDetail } from '../shared/McpPermissionDetail';
import { McpEditor } from './McpEditor';
import { McpIcon } from './McpIcon';
import { McpToolCard } from './McpToolCard';
import css from './McpPage.module.css';

type Section = 'tools' | 'permissions' | 'connection';

interface Props {
  source: SourceSettings;
  scope: SourceScope;
  revision?: string;
  focus: McpFocus & { name: string };
  scopeLabel: ReactNode;
  onClose: () => void;
  editorDraft?: McpEditorDraft;
  onEditorDraftChange: (draft?: McpEditorDraft) => void;
  loadMcpTools: LoadMcpTools;
  listingAuthority: string;
  listingAvailable: boolean;
}

function useToolListing({ enabled, name, scope, authority, load }: {
  enabled: boolean; name: string; scope: SourceScope; authority: string; load: LoadMcpTools;
}) {
  const [attempt, setAttempt] = useState(0);
  const requestKey = JSON.stringify([name, scope, authority, attempt]);
  const [listing, setListing] = useState<{ key: string; result?: WorkflowToolSource }>();
  const current = listing?.key === requestKey ? listing : undefined;
  useEffect(() => {
    if (!enabled || current) return;
    let owns = true;
    void load(scope, name, scope, () => owns).then(result => {
      if (owns) setListing({ key: requestKey, result: result?.kind === 'mcp' && result.scope === scope && result.source_id === name ? result : undefined });
    }, () => {
      if (owns) setListing({ key: requestKey });
    });
    return () => { owns = false; };
  }, [enabled, current, load, name, requestKey, scope]);
  return { current: enabled ? current : undefined, refresh: () => setAttempt(value => value + 1) };
}

export function McpServerDetail({ source, scope, revision, focus, scopeLabel, onClose, editorDraft, onEditorDraftChange, loadMcpTools, listingAuthority, listingAvailable }: Props) {
  const tx = useTranslation();
  const initialSection = focus.mode === 'permissions' ? 'permissions' : focus.mode === 'connection' ? 'connection' : 'tools';
  const [section, setSection] = useState<Section>(initialSection);
  const [openedConnection, setOpenedConnection] = useState(initialSection === 'connection');
  const [openedListing, setOpenedListing] = useState(initialSection !== 'connection');
  const [query, setQuery] = useState('');
  const closeEditor = useRef<(() => void) | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus({ preventScroll: true }); }, []);
  const { current, refresh } = useToolListing({
    enabled: listingAvailable && openedListing, name: focus.name, scope, authority: listingAuthority, load: loadMcpTools,
  });
  const tools = current?.result?.state === 'listed' ? current.result.tools : undefined;
  const mcp = documentAuthoring(scope === 'user' ? source.user_mcp : source.workspace_mcp);
  const authored = mcp.state === 'structured' && Object.hasOwn(mcp.document, focus.name) ? mcp.document[focus.name] : undefined;
  const normalized = query.trim().toLocaleLowerCase();
  const filtered = tools?.filter(tool => !normalized || [tool.name, tool.description].some(value => value?.toLocaleLowerCase().includes(normalized)));
  const loading = listingAvailable && !current;

  return <>
    <div className={css.back}><Button size="sm" variant="ghost" onClick={() => closeEditor.current ? closeEditor.current() : onClose()}>{tx('settings:mcp.back')}</Button></div>
    <header className={css.serverHeading}>
      <span className={catalog.fileTile}><McpIcon size={18} /></span>
      <h3 ref={heading} tabIndex={-1}>{focus.name}</h3>
      <div className={css.scope}>{tx('settings:mcp.scope')}{scopeLabel}</div>
    </header>
    <Tabs selectedKey={section} onSelectionChange={key => {
      const next = key as Section;
      if (next === 'connection') setOpenedConnection(true);
      else setOpenedListing(true);
      setSection(next);
    }}>
      <TabList aria-label={tx('resources:mcp.detail-tabs')} className={catalog.tabs}>
        <Tab id="tools" className={catalog.tab}>{tx('resources:mcp.tools')}</Tab>
        <Tab id="permissions" className={catalog.tab}>{tx('resources:mcp.permissions')}</Tab>
        <Tab id="connection" className={catalog.tab}>{tx('settings:mcp.connection')}</Tab>
      </TabList>
      <TabPanel id="tools" className={css.serverPanel}>
        <div className={css.sectionHead}>
          <h4>{tools ? tx('resources:mcp.tools-count', { count: tools.length }) : tx('resources:mcp.tools')}</h4>
          {tools && <div className={css.toolToolbar}>
            {tools.length > 0 && <CatalogSearch label={tx('resources:mcp.find-tools')} value={query} onChange={setQuery} />}
            <Button size="sm" variant="outline" onClick={refresh}>{tx('resources:refresh')}</Button>
          </div>}
        </div>
        {loading ? <p role="status" className={css.hint}>{tx('resources:mcp.tools-loading')}</p>
          : !listingAvailable ? <p role="status" className={css.hint}>{tx('resources:mcp.tools-unavailable')}</p>
          : !tools ? <div className={css.toolFeedback}>
            <p role="status" className={css.hint}>{tx('resources:mcp.tools-failed')}</p>
            <Button size="sm" variant="outline" onClick={refresh}>{tx('resources:mcp.tools-retry')}</Button>
          </div>
          : tools.length === 0 ? <p role="status" className={css.hint}>{tx('resources:mcp.tools-empty')}</p>
          : filtered?.length === 0 ? <p role="status" className={css.hint}>{tx('resources:mcp.tools-no-match')}</p>
          : authored && mcp.state === 'structured' && <ul className={css.toolCards} role="list">
            {filtered?.map(tool => <McpToolCard key={tool.name} tool={tool} server={focus.name} authored={authored} revision={mcp.revision} />)}
          </ul>}
      </TabPanel>
      <TabPanel id="permissions" className={css.serverPanel}>
        <McpPermissionDetail source={source} scope={scope} revision={revision} name={focus.name} tools={tools} />
      </TabPanel>
      {openedConnection && <TabPanel id="connection" className={css.serverPanel} shouldForceMount>
        <McpEditor source={source} scope={scope} focus={focus} scopeControl={scopeLabel} close={onClose}
          editorDraft={editorDraft} onEditorDraftChange={onEditorDraftChange} backRef={closeEditor} />
      </TabPanel>}
    </Tabs>
  </>;
}
