/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource controls; see PROVENANCE.md. */
import type { McpWrite, ResourcePermissionSet, SourceScope, SourceSettings, WorkflowToolChoice } from '../../../../../protocol/app-server/v80';
import { useTranslation } from '../../../locale/react';
import { UnitForm } from '../../settings/forms/bridge';
import { documentAuthoring } from '../../settings/projection';
import { ConfigUnavailable } from '../../settings/forms/ConfigUnavailable';
import { ResourcePermissionFields, blankResourcePermissions } from '../../settings/forms/ResourcePermissionFields';
import settingsCss from '../../../presentation/settings/SettingsContent.module.css';
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
  if (mcp.state !== 'structured') return <ConfigUnavailable />;
  const authored = Object.hasOwn(mcp.document, name) ? mcp.document[name] : undefined;
  const blank: McpWrite = {
    definition: { type: 'stdio', command: '', args: [], permissions: blankResourcePermissions },
    retained_env: [],
    retained_headers: [],
  };
  return (
    <section aria-label={tx('resources:mcp.permissions')}>
      {revision !== undefined && (
        <AvailabilityDetail source={source} scope={scope} revision={revision} family="mcp" name={name}
          tools={tools} heading={tx('resources:mcp.availability')} saveLabel={tx('resources:mcp.save-availability')} />
      )}
      <section aria-label={tx('resources:mcp.approval-rules')}>
        <h4>{tx('resources:mcp.approval-rules')}</h4>
        <p className={settingsCss.hint}>{tx('resources:mcp.approval-rules-help')}</p>
        <UnitForm<McpWrite>
          title={tx('resources:mcp.permissions')}
          authored={authored}
          blank={blank}
          revision={mcp.revision}
          draftKey={JSON.stringify(['mcp-permissions', name])}
          removable={false}
          saveLabel={tx('resources:mcp.save-rules')}
          mutation={value => ({ kind: 'mcp', id: name, authored: value })}
        >
          {(value, change) => (
            <ResourcePermissionFields
              value={value.definition.permissions}
              change={(permissions: ResourcePermissionSet | null) =>
                change({
                  ...value,
                  definition: {
                    ...value.definition,
                    permissions: permissionsCleared(permissions) ? undefined : permissions ?? undefined,
                  },
                })
              }
            />
          )}
        </UnitForm>
      </section>
    </section>
  );
}
