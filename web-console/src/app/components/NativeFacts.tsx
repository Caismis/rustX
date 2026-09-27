import { useTranslation } from '../../locale/react';
import { Facts } from '../../presentation/settings/SettingsContent';
/** Read-only native inspection. Labels are formatting, never overlay/default resolution. */
export function NativeFacts({ value }: { value: unknown }) {
  const tx = useTranslation();
  if (value === null || value === undefined) return <span>{tx('common:native-facts.not-specified')}</span>;
  if (typeof value === 'boolean') return <span>{value ? tx('common:native-facts.yes') : tx('common:native-facts.no')}</span>;
  if (typeof value !== 'object') return <span>{String(value)}</span>;
  if (Array.isArray(value)) return value.length ? <ul>{value.map((item, index) => <li key={index}><NativeFacts value={item} /></li>)}</ul> : <span>{tx('common:native-facts.none-empty-list')}</span>;
  const entries = Object.entries(value);
  return entries.length ? <Facts rows={entries.map(([key, entry]) => [key.replaceAll('_', ' '), <NativeFacts value={entry} />])} /> : <span>{tx('common:native-facts.empty-object-native-defaults')}</span>;
}
