import { Facts } from '../../presentation/settings/SettingsContent';
/** Read-only native inspection. Labels are formatting, never overlay/default resolution. */
export function NativeFacts({ value }: { value: unknown }) {
  if (value === null || value === undefined) return <span>Not specified</span>;
  if (typeof value === 'boolean') return <span>{value ? 'Yes' : 'No'}</span>;
  if (typeof value !== 'object') return <span>{String(value)}</span>;
  if (Array.isArray(value)) return value.length ? <ul>{value.map((item, index) => <li key={index}><NativeFacts value={item} /></li>)}</ul> : <span>None · empty list</span>;
  const entries = Object.entries(value);
  return entries.length ? <Facts rows={entries.map(([key, entry]) => [key.replaceAll('_', ' '), <NativeFacts value={entry} />])} /> : <span>Empty object · native defaults</span>;
}
