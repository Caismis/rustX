/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource controls; see PROVENANCE.md. */
import type { McpWrite, ResourcePermissionSet, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v80';
import { useTranslation } from '../../../locale/react';
import { UnitForm } from '../../settings/forms/bridge';
import { documentAuthoring } from '../../settings/projection';
import { ConfigUnavailable } from '../../settings/forms/ConfigUnavailable';
import { ResourcePermissionFields, blankResourcePermissions } from '../../settings/forms/ResourcePermissionFields';
import { AvailabilityDetail } from './AvailabilityDetail';

function permissionsCleared(permissions: ResourcePermissionSet | null | undefined): boolean {
  if (permissions == null) return true;
  return permissions.plan === 'default'
    && permissions.automatic === 'default'
    && permissions.full_access === 'default';
}

/** Source availability plus three-mode MCP server permission overrides.
 *
 * Permissions ride the same MCP unit transaction as the connection editor:
 * one shared draft means one document write carries both field sets on Save.
 * Remove is disabled: clearing overrides writes `permissions: undefined` while
 * preserving the server definition. */
export function McpPermissionDetail({
  source,
  scope,
  revision,
  name,
  embedded,
}: {
  source: SourceSettings;
  scope: SourceScope;
  revision?: string;
  name: string;
  /** Set when rendered inside the connection editor, which owns the shared
   * unit's outcome and review chrome. */
  embedded?: boolean;
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
        <AvailabilityDetail source={source} scope={scope} revision={revision} family="mcp" name={name} />
      )}
      {/* Same unit identity as the connection editor's McpWrite mutation:
          one shared draft, one CAS write per save — a Command+Plan+Automatic
          batch lands in a single document write. */}
      <UnitForm<McpWrite> embedded={embedded}
        title={tx('resources:mcp.permissions')}
        authored={authored}
        blank={blank}
        revision={mcp.revision}
        removable={false}
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
  );
}
