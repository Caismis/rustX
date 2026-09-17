/** Static dictionary interpolation; no Cordis/Host locale settings authority. */
import { en } from './workspace';
export type Translate = (key: string, params?: Record<string, string | number>) => string;
export const t: Translate = (key, params = {}) => {
  const dictionary: Record<string, string> = { ...en, copy: 'Copy', 'menu.deleteSession': 'Delete Session', 'rename': 'Rename', 'delete.workspace': 'Unregister Workspace' };
  return (dictionary[key] ?? key).replace(/\{(\w+)\}/g, (match, name: string) => String(params[name] ?? match));
};
