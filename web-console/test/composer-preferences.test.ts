import { expect, it, vi } from 'vitest';
import { ComposerPreferences, COMPOSER_PREFERENCE_KEY } from '../src/app/composer/preferences';
it.each([null, 'queue', 'steer', 'corrupt', '"steer"'])('validates stored %s against the closed union', stored => {
  const getItem = vi.fn(() => stored);
  expect(new ComposerPreferences({ getItem, setItem: vi.fn() }).getSnapshot()).toBe(stored === 'steer' ? 'steer' : 'queue');
  expect(getItem).toHaveBeenCalledExactlyOnceWith(COMPOSER_PREFERENCE_KEY);
});
it('denied storage defaults to queue and denied writes still publish a usable preference', () => {
  const storage = { getItem: () => { throw Error('denied'); }, setItem: vi.fn(() => { throw Error('denied'); }) };
  const owner = new ComposerPreferences(storage), changed = vi.fn();
  const off = owner.subscribe(changed);
  expect(owner.getSnapshot()).toBe('queue'); owner.setBusyEnter('steer'); expect(owner.getSnapshot()).toBe('steer');
  expect(storage.setItem).toHaveBeenCalledExactlyOnceWith(COMPOSER_PREFERENCE_KEY, 'steer'); expect(changed).toHaveBeenCalledTimes(1);
  off(); owner.setBusyEnter('queue'); expect(changed).toHaveBeenCalledTimes(1);
});
it('a new owner reads the persisted preference and ignores values outside the union', () => {
  const values = new Map<string, string>();
  const storage = { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); } };
  const owner = new ComposerPreferences(storage); owner.setBusyEnter('steer');
  expect(new ComposerPreferences(storage).getSnapshot()).toBe('steer');
  owner.setBusyEnter('invalid' as 'queue'); expect(values.get(COMPOSER_PREFERENCE_KEY)).toBe('steer');
});
