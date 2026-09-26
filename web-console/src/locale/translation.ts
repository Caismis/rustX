import * as common from './dictionaries/common';
import * as inspector from './dictionaries/inspector';
import * as agent from './dictionaries/agent';
import * as workspace from './dictionaries/workspace';
import * as interactions from './dictionaries/interactions';
import * as commands from './dictionaries/commands';
import * as artifacts from './dictionaries/artifacts';
import * as settings from './dictionaries/settings';
import * as trajectory from './dictionaries/trajectory';
import * as tools from './dictionaries/tools';
import * as sidebar from './dictionaries/sidebar';
import { documentLanguage, type LocaleId } from './controller';
const dictionaries = { common, inspector, agent, workspace, interactions, commands, artifacts, settings, trajectory, tools, sidebar };
export type Namespace = keyof typeof dictionaries;
export type TranslationKey = { [N in Namespace]: `${N}:${keyof typeof dictionaries[N]['en'] & string}` }[Namespace];
export type Translate = ((key: TranslationKey, params?: Readonly<Record<string, string | number>>) => string) & { readonly locale: LocaleId; readonly language: string };
export function interpolate(template: string, params: Readonly<Record<string, string | number>> = {}): string {
  return template.replace(/\{(\w+)\}/g, (match, name: string) => Object.hasOwn(params, name) ? String(params[name]) : match);
}
function bind(locale: LocaleId): Translate {
  const translate = (key: TranslationKey, params?: Readonly<Record<string, string | number>>) => {
    const split = key.indexOf(':');
    const namespace = key.slice(0, split) as Namespace;
    // The public key union verifies both namespace and key. There is no
    // missing-key fallback: both built-ins must satisfy the exact namespace.
    const dictionary = dictionaries[namespace][locale] as Record<string, string>;
    return interpolate(dictionary[key.slice(split + 1)], params);
  };
  return Object.assign(translate, { locale, language: documentLanguage(locale) });
}
const builtins = { en: bind('en'), zh: bind('zh') };
export function translator(locale: LocaleId): Translate { return builtins[locale]; }

/** Deferred browser-authored copy may be retained by a notice or async picker.
 * Raw strings are opaque native/user facts; never inspect or translate them. */
export interface Message { readonly key: TranslationKey; readonly params?: Readonly<Record<string, string | number | Message>> }
export type DisplayText = string | Message;
export const message = (key: TranslationKey, params?: Message['params']): Message => ({ key, params });
export function displayText(tx: Translate, value: DisplayText): string {
  if (typeof value === 'string') return value;
  return tx(value.key, Object.fromEntries(Object.entries(value.params ?? {}).map(([key, part]) => [key, typeof part === 'object' ? displayText(tx, part) : part])));
}
