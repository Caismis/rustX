import { expect, it } from 'vitest';
import { findCopy } from '../scripts/i18n-audit';
import { translator } from '../src/locale/translation';
import { en, zh, type SettingsKey } from '../src/locale/dictionaries/settings';
it('checks multiline JSX, accessibility, tooltip labels, defaults and callback copy with AST positions', () => {
  const violations = findCopy('example.tsx', `
    function Panel({ placeholder = 'Write a message' }) {
      return <section aria-description={busy ? 'Please wait' : 'Ready'}>
        New
        Conversation
        <Input placeholder={placeholder} title="Help" />
        <Tooltip label={\`Show \${count} more\`} />
        <Confirm confirm="Delete permanently" />
        <Block labels={{ collapseAria: () => { return 'Hide lines'; }, expandAria: n => \`Show \${n} lines\` }} />
      </section>;
    }
  `);
  expect(violations.map(v => v.text)).toEqual(expect.arrayContaining(['Write a message', 'Please wait', 'Ready', 'New Conversation', 'Help', 'Show {p0} more', 'Show {p0} lines', 'Hide lines', 'Delete permanently']));
  expect(violations.every(v => v.line > 1)).toBe(true);
});
it('permits opaque expressions and one reasoned literal without exempting nearby copy', () => {
  const violations = findCopy('example.tsx', `<><p>{error.message}</p><button aria-label={tx('settings:page.general')}>rustX</button><span>{/* i18n-raw: immutable wire identity */ 'native.Tool'}</span><p>Translate this</p></>`);
  expect(violations.map(v => v.text)).toEqual(['Translate this']);
});
it('the built-in dictionary contract is exact and public keys stay bounded', () => {
  expect(Object.keys(en).sort()).toEqual(Object.keys(zh).sort());
  expect(Object.values(zh).every(value => value.length > 0)).toBe(true);
  // This function is intentionally never called. tsc verifies that each
  // negative type contract remains an error, rather than a runtime fallback.
  function typeContract() {
    // @ts-expect-error Missing keys cannot satisfy a namespace.
    const incomplete: Record<SettingsKey, string> = {};
    // @ts-expect-error There is no arbitrary-key translation path.
    translator('en')('settings:this-key-does-not-exist');
    // @ts-expect-error Exactly two locale ids are built in.
    translator('fr');
    return incomplete;
  }
  expect(typeContract).toBeTypeOf('function');
});

it('requires stable option values so localized labels cannot become filter identities', () => {
  const source = `<select><option>{tx('inspector:inspector.request')}</option><option value="response">{tx('inspector:inspector.response')}</option></select>`;
  expect(findCopy('example.tsx', source).map(row => row.kind)).toEqual(['identity']);
});
