import { SettingsNotice } from '../SettingsFeedback';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource controls; see PROVENANCE.md. */
import { useTranslation } from '../../../locale/react';
import type { SourceScope, SourceSettings } from '../../../../../protocol/app-server/v80';
import { AvailabilityDetail } from '../../resources/shared/AvailabilityDetail';
import type { PageFocus } from '../machines/navigation';
import { extensionEntries } from '../../resources/shared/inventory';
import { SettingsBackButton, SettingsPageShell, SettingsGroup } from '../../../presentation/settings/SettingsPageShell';
import { settingsContentCss } from '../../../presentation/settings/settingsContentCss';

export function ExtensionDetail({ source, scope, revision, name, onFocus, bodyDisabled = false }: {
  source: SourceSettings; scope: SourceScope; revision?: string; name: string;
  onFocus: (focus?: PageFocus['extensions']) => void;
  bodyDisabled?: boolean;
}) {
  const tx = useTranslation();
  const entry = extensionEntries(source, scope, 'managed_python').find(row => row.name === name);
  return (
    <SettingsPageShell
      className={`${settingsContentCss.settings} ${settingsContentCss.page}`}
      title={name}
      bodyDisabled={bodyDisabled}
      breadcrumb={
        <SettingsBackButton
          label={tx('settings:extension-detail.extensions')}
          current={name}
          onClick={() => onFocus(undefined)}
        />
      }
    >
      <SettingsGroup title={name} description={entry?.path ? <code>{entry.path}</code> : undefined}>
        <div style={{ padding: '14px 16px' }}>
          <p>{tx('settings:extensions.manage-files')}</p>
          <SettingsNotice text={entry?.diagnostics[0] ?? (source.prospective_resources?.sources[`python:${name}`]?.status === 'unavailable' ? tx('settings:extensions.unavailable') : undefined)} />
          <AvailabilityDetail source={source} scope={scope} revision={revision} family="managed_python" name={name} />
        </div>
      </SettingsGroup>
    </SettingsPageShell>
  );
}
