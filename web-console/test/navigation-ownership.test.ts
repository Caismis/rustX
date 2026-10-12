import { expect, it } from 'vitest';
import { NavigationEpoch } from '../src/client/navigation';

it('failed preparation retires itself without revoking committed controls', () => {
  const navigation = new NavigationEpoch(), committed = navigation.captureCommitted();
  const pending = navigation.begin('conversation');
  expect(committed()).toBe(true); expect(pending.current()).toBe(true);
  pending.finish();
  expect(pending.current()).toBe(false); expect(committed()).toBe(true);
});
it('superseded cleanup cannot revoke or restore a newer intent or committed route', () => {
  const navigation = new NavigationEpoch(), committed = navigation.captureCommitted();
  const first = navigation.begin('conversation'), next = navigation.begin('sidebar');
  first.finish(); expect(first.current()).toBe(false); expect(next.current()).toBe(true); expect(committed()).toBe(true);
  navigation.invalidate(); // Synchronous route publication.
  const destination = navigation.captureCommitted();
  first.finish(); next.finish();
  expect(destination()).toBe(true); expect(committed()).toBe(false);
});
it('Settings retires every pending source, while idle dismissal preserves controls', () => {
  for (const kind of ['sidebar', 'conversation'] as const) {
    const navigation = new NavigationEpoch(), committed = navigation.captureCommitted();
    const pending = navigation.begin(kind);
    navigation.retire(); expect(pending.current()).toBe(false); expect(committed()).toBe(true);
    const continuation = navigation.capture(); navigation.retire(); navigation.retire('sidebar');
    expect(continuation()).toBe(true); expect(committed()).toBe(true);
  }
});
it('overlay dismissal is scoped and programmatic closure has no retirement', () => {
  const navigation = new NavigationEpoch(), fork = navigation.begin('conversation');
  navigation.retire('sidebar'); expect(fork.current()).toBe(true);
  const sidebar = navigation.begin('sidebar'); navigation.retire('sidebar');
  expect(sidebar.current()).toBe(false);
  navigation.invalidate(); const destination = navigation.captureCommitted();
  sidebar.finish(); expect(destination()).toBe(true);
});
it('catalog read freshness cannot revoke controls or manufacture a sidebar route owner', () => {
  const navigation = new NavigationEpoch(), committed = navigation.captureCommitted();
  const pending = navigation.begin('conversation');
  navigation.supersede(); expect(pending.current()).toBe(false); expect(committed()).toBe(true);
  const read = navigation.capture();
  navigation.retire('sidebar'); navigation.retire(); // Idle dismissal/Settings.
  expect(read()).toBe(true);
  navigation.supersede(); expect(read()).toBe(false); expect(committed()).toBe(true);
});
