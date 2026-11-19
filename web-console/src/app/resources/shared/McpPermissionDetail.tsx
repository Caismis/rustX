/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource controls; see PROVENANCE.md. */
import type { ResourcePermissionSet, SourceScope, SourceSettings, WorkflowToolChoice } from '../../../../../protocol/app-server/v80';
import { useTranslation } from '../../../locale/react';
import { UnitForm } from '../../settings/forms/bridge';
import { documentAuthoring } from '../../settings/projection';
import { ConfigUnavailable } from '../../settings/forms/ConfigUnavailable';
import { ResourcePermissionFields } from '../../settings/forms/ResourcePermissionFields';
import css from '../mcp/McpPage.module.css';
import { AvailabilityDetail } from './AvailabilityDetail';

function permissionsCleared(permissions: ResourcePermissionSet | null | undefined): boolean {
  if (permissions == null) return true;
  return permissions.plan === 'default'
    && permissions.automatic === 'default'
    && permissions.full_access === 'default';
}

/** Source availability plus three-mode MCP server permission overrides.
 *
 * Permissions ride the MCP identity mutation but keep a distinct transaction
 * draft key so this card never shares Save/Remove with the connection editor.
 * Remove is disabled: clearing overrides writes `permissions: undefined` while
 * preserving the server definition. */
export function McpPermissionDetail({
  source,
  scope,
  revision,
  name,
  tools,
}: {
  source: SourceSettings;
  scope: SourceScope;
  revision?: string;
  name: string;
  tools?: readonly WorkflowToolChoice[];
}) {
  const tx = useTranslation();
  const mcp = documentAuthoring(scope === 'user' ? source.user_mcp : source.workspace_mcp);
  const authored = mcp.state === 'structured' && Object.hasOwn(mcp.document, name) ? mcp.document[name] : undefined;
  return (
    <section className={css.permissionGroups} aria-label={tx('resources:mcp.permissions')}>
      <section className={css.serverRules} aria-label={tx('resources:mcp.approval-rules')}>
        <h4>{tx('resources:mcp.approval-rules')}</h4>
        <p className={css.hint}>{tx('resources:mcp.approval-rules-help')}</p>
        {mcp.state === 'structured' && authored ? <UnitForm<{ rules: ResourcePermissionSet | null }>
          title={tx('resources:mcp.permissions')}
          authored={{ rules: authored.definition.permissions ?? null }}
          blank={{ rules: null }}
          revision={mcp.revision}
          draftKey={JSON.stringify(['mcp-permissions', name])}
          removable={false}
          saveLabel={tx('resources:mcp.save-rules')}
          mutation={value => ({ kind: 'mcp', id: name, authored: value === null ? null : {
            ...authored, definition: { ...authored.definition, permissions: value.rules ?? undefined },
          } })}
        >
          {(value, change) => (
            <div className={css.modeFields}><ResourcePermissionFields
              value={value.rules}
              change={permissions => change({ rules: permissionsCleared(permissions) ? null : permissions })}
            /></div>
          )}
        </UnitForm> : mcp.state === 'structured' && scope === 'workspace'
          ? <p className={css.hint}>{tx('resources:mcp.approval-rules-inherited')}</p>
          : <ConfigUnavailable />}
      </section>
      {revision !== undefined && (
        <AvailabilityDetail source={source} scope={scope} revision={revision} family="mcp" name={name}
          tools={tools} heading={tx('resources:mcp.availability')} saveLabel={tx('resources:mcp.save-availability')}
          help={tx('resources:mcp.availability-help')} />
      )}
    </section>
  );
}
