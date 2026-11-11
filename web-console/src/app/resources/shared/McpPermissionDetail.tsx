/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource controls; see PROVENANCE.md. */
import type { McpWrite, ResourcePermissionSet, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v78';
import { useTranslation } from '../../../locale/react';
import { UnitForm } from '../../settings/forms/bridge';
import { documentAuthoring } from '../../settings/projection';
import { ConfigUnavailable } from '../../settings/forms/ConfigUnavailable';
import { ResourcePermissionFields, blankResourcePermissions } from '../../settings/forms/ResourcePermissionFields';
import { AvailabilityDetail } from './AvailabilityDetail';

/** Source availability plus three-mode MCP server permission overrides. */
export function McpPermissionDetail({
  source,
  scope,
  revision,
  name,
}: {
  source: SourceSettings;
  scope: SourceScope;
  revision?: string;
  name: string;
}) {
  const tx = useTranslation();
  const mcp = documentAuthoring(scope === 'user' ? source.user_mcp : source.workspace_mcp);
  if (revision === undefined || mcp.state !== 'structured') return <ConfigUnavailable />;
  const authored = Object.hasOwn(mcp.document, name) ? mcp.document[name] : undefined;
  const blank: McpWrite = {
    definition: { type: 'stdio', command: '', args: [], permissions: blankResourcePermissions },
    retained_env: [],
    retained_headers: [],
  };
  return (
    <section aria-label={tx('resources:mcp.permissions')}>
      <AvailabilityDetail source={source} scope={scope} revision={revision} family="mcp" name={name} />
      <UnitForm<McpWrite>
        title={tx('resources:mcp.permissions')}
        authored={authored}
        blank={blank}
        revision={mcp.revision}
        mutation={value => ({ kind: 'mcp', id: name, authored: value })}
      >
        {(value, change) => (
          <ResourcePermissionFields
            value={value.definition.permissions}
            change={(permissions: ResourcePermissionSet | null) =>
              change({
                ...value,
                definition: { ...value.definition, permissions: permissions ?? undefined },
              })
            }
          />
        )}
      </UnitForm>
    </section>
  );
}
