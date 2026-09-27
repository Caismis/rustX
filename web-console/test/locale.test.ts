import { compactTokens } from '../src/app/agent/TurnTail';
import { describe, expect, it, vi } from 'vitest';
import { LocaleController, documentLanguage, LOCALE_STORAGE_KEY, type LocaleEnvironment, type LocaleId } from '../src/locale/controller';
import { displayText, interpolate, message, searchVocabulary, translator } from '../src/locale/translation';
function environment(stored: string | null, languages: string[] = []): LocaleEnvironment {
  return { read: () => stored, write: vi.fn(), languages: () => languages, documentLanguage: vi.fn() };
}
describe('one browser locale owner', () => {
  it.each([
    ['en', ['zh-CN'], 'en'], ['zh', ['en-US'], 'zh'], ['invalid', ['zh'], 'zh'],
    [null, ['zh'], 'zh'], [null, ['zh-CN'], 'zh'], [null, ['zh-Hant-TW'], 'zh'],
    [null, ['fr', 'zh'], 'en'], [null, ['en-US'], 'en'], [null, [], 'en'],
  ] as [string | null, string[], LocaleId][])('resolves %s / %j to %s', (stored, languages, active) => {
    const env = environment(stored, languages), owner = new LocaleController(env);
    expect(owner.getSnapshot()).toEqual({ active, revision: 0 });
    expect(env.documentLanguage).toHaveBeenCalledExactlyOnceWith(documentLanguage(active));
    expect(env.write).not.toHaveBeenCalled();
  });
  it('denied browser APIs are optional; writes never prevent live updates', () => {
    const denied = () => { throw Error('Denied'); };
    const owner = new LocaleController({ read: denied, write: denied, languages: denied, documentLanguage: denied });
    expect(owner.getSnapshot().active).toBe('en');
    owner.setLocale('zh'); expect(owner.getSnapshot()).toEqual({ active: 'zh', revision: 1 });
  });
  it('storage failure still uses available browser languages', () => {
    const env = environment(null, ['zh']); env.read = () => { throw Error('Denied'); };
    expect(new LocaleController(env).getSnapshot().active).toBe('zh');
  });
  it('publishes stable frozen snapshots synchronously, persists explicit unchanged choices, unsubscribes', () => {
    const env = environment(null), owner = new LocaleController(env), seen: unknown[] = [];
    const initial = owner.getSnapshot(); expect(Object.isFrozen(initial)).toBe(true);
    const off = owner.subscribe(() => seen.push(owner.getSnapshot()));
    owner.setLocale('en'); expect(owner.getSnapshot()).toBe(initial); expect(seen).toEqual([]);
    expect(env.write).toHaveBeenCalledWith('en');
    owner.setLocale('zh'); expect(seen).toEqual([{ active: 'zh', revision: 1 }]);
    expect(env.documentLanguage).toHaveBeenLastCalledWith('zh-CN');
    expect(env.write).toHaveBeenLastCalledWith('zh');
    expect(owner.getSnapshot()).toBe(seen[0]);
    off(); owner.setLocale('en'); expect(seen).toHaveLength(1);
    expect(owner.getSnapshot().revision).toBe(2);
    expect(env.documentLanguage).toHaveBeenLastCalledWith('en');
    expect(LOCALE_STORAGE_KEY).toBe('rustx-locale-v1');
  });
  it('interpolates values literally and leaves opaque failures unchanged', () => {
    expect(interpolate('{name} {n} {missing}', { name: '$& <native>', n: 0 })).toBe('$& <native> 0 {missing}');
    expect(interpolate('{constructor}')).toBe('{constructor}');
    const tx = translator('zh');
    expect(tx('workspace:actions.session.aria', { name: '/raw/name' })).toContain('/raw/name');
    expect(displayText(tx, 'Native failure ENGLISH: {n}')).toBe('Native failure ENGLISH: {n}');
    expect(displayText(tx, message('settings:page.general'))).toBe('通用设置');
  });
});

it('presentation formatting uses the selected rustX locale', () => {
  expect(compactTokens(translator('en'), 12000)).toBe('12K');
  expect(compactTokens(translator('zh'), 12000)).toBe('1.2万');
});


it('invariant search vocabulary preserves opaque strings and renders nested messages in both built-ins', () => {
  const raw = 'native.ID /路径 {name}';
  expect(searchVocabulary(raw)).toBe(raw);
  const nested = message('commands:tree.node-detail', { conversation: raw, origin: 'fork', parent: message('commands:copy.parent-value', { p0: 'node-原样' }) });
  expect(searchVocabulary(nested)).toBe(`${displayText(translator('en'), nested)} ${displayText(translator('zh'), nested)}`);
  expect(searchVocabulary(nested)).toContain('node-原样');
});
