import Anser from 'anser';
import { expect, it, vi } from 'vitest';
import { parseAnsiLines } from '../src/presentation/primitives/ansi';
it('terminal output uses theme ANSI runs and consumes control sequences without interpreting markup', () => {
  const lines = parseAnsiLines('\u001b[31mred\u001b[0m\n\u001b]8;;https://example.com\u0007<img src=x>\u001b]8;;\u0007');
  expect(lines.map(line => line.map(span => span.text).join(''))).toEqual(['red', '<img src=x>']);
  expect(lines[0][0].style?.color).toBe('var(--dsw-alias-state-error-primary)');
});

it('cursor replay keeps unsupported SGR parameters bounded', () => {
  const input = '\r' + Array.from({ length: 256 }, (_, index) => `\u001b[${1000 + index}mX`).join('');
  const convert = vi.spyOn(Anser, 'ansiToJson');
  try {
    const lines = parseAnsiLines(input);
    expect(lines.flat().map(span => span.text).join('')).toBe('X'.repeat(256));
    expect(String(convert.mock.calls[0][0]).length).toBeLessThanOrEqual(input.length);
  } finally {
    convert.mockRestore();
  }
});

it('cursor replay preserves supported decorations and selective resets', () => {
  const input = '\u001b[1;2;3;4;5;7;8;9;31mA\u001b[21mB\u001b[1mC\u001b[22;23;24;25;27;28;29;39mD';
  expect(parseAnsiLines('\r' + input)).toEqual(parseAnsiLines(input));
});
