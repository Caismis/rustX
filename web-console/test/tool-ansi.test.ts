import { expect, it } from 'vitest';
import { parseAnsiLines } from '../src/presentation/primitives/ansi';
it('terminal output uses theme ANSI runs and consumes control sequences without interpreting markup', () => {
  const lines = parseAnsiLines('\u001b[31mred\u001b[0m\n\u001b]8;;https://example.com\u0007<img src=x>\u001b]8;;\u0007');
  expect(lines.map(line => line.map(span => span.text).join(''))).toEqual(['red', '<img src=x>']);
  expect(lines[0][0].style?.color).toBe('var(--dsw-alias-state-error-primary)');
});
