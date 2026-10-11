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

it.each([512, 1024, 2048])('bounds the real pre-Anser replay with %i long-token transitions', n => {
  const input = '\r\x1b[' + '0'.repeat(n) + '31m' + '\x1b[1mA\x1b[22mB'.repeat(n);
  const convert = vi.spyOn(Anser, 'ansiToJson');
  try {
    const lines = parseAnsiLines(input);
    const intermediate = String(convert.mock.calls[0][0]);
    expect(lines.flat().map(span => span.text).join('')).toBe('AB'.repeat(n));
    expect(intermediate.length).toBe(22 * n - 4);
    // Eight one-digit attributes (15), two RGB groups (16 each), two
    // separators and ESC[...m (3): at most 52 bytes per opening. Each
    // rendered cell and final state can require one reset (4) + opening.
    expect(intermediate.length).toBeLessThanOrEqual(2 * n + 56 * (2 * n + 1));
  } finally { convert.mockRestore(); }
});

function captureReplay(input: string) {
  const convert = vi.spyOn(Anser, 'ansiToJson');
  try {
    const lines = parseAnsiLines(input);
    return { lines, intermediate: String(convert.mock.calls[0][0]) };
  } finally { convert.mockRestore(); }
}

it.each([512, 1024, 2048])('normalizes every supported color form at size %i', n => {
  const zeros = '0'.repeat(n);
  for (const color of ['31', '41', '91', '101', '38;5;208', '48;5;208', '38;2;255;128;0', '48;2;255;128;0']) {
    const long = color.split(';').map(value => zeros + value).join(';');
    const suffix = '\x1b[01mA\x1b[022mB'.repeat(n);
    const { lines, intermediate } = captureReplay('\r\x1b[' + long + 'm' + suffix);
    expect(lines).toEqual(parseAnsiLines('\x1b[' + color + 'm' + suffix));
    expect(intermediate.length).toBeLessThanOrEqual(2 * n + 56 * (2 * n + 1));
    expect(intermediate).not.toContain(zeros);
    for (const match of intermediate.matchAll(/\x1b\[[^m]*m/g)) expect(match[0].length).toBeLessThanOrEqual(52);
  }
});

it.each([512, 1024, 2048])('bounds repeated foreground/background changes at size %i', n => {
  const zeros = '0'.repeat(n);
  const pair = '\x1b[' + zeros + '31;' + zeros + '104mA\x1b[38;2;' + zeros + '255;128;0;48;5;' + zeros + '208mB';
  const { lines, intermediate } = captureReplay('\r' + pair.repeat(n));
  expect(lines).toEqual(parseAnsiLines('\x1b[31;104mA\x1b[38;2;255;128;0;48;5;208mB'.repeat(n)));
  expect(intermediate.length).toBeLessThanOrEqual(2 * n + 56 * (2 * n + 1));
});

it.each([
  ['38', ''], ['48;2;1;4', ''], ['38;5', ''],
  ['38;5;256', ''], ['48;2;256;1;4', ''], ['38;2;1;;4', ''],
  ['38;2;1:2;4;5', ''], ['38;2;999999999999999999999999;4;5', ''],
  ['48;7;1;4;31', ''], ['38;;1;4', ''], ['38;9:1;1;4', ''],
  ['38;2;999;1;4;3', '\x1b[3m'], ['48;5;999;4', '\x1b[4m'],
])('consumes rejected color groups without promoting payload attributes: %s', (invalid, remaining) => {
  for (const replay of ['', '\r']) {
    const prefix = '\x1b[31;44m';
    expect(parseAnsiLines(replay + prefix + '\x1b[' + invalid + 'mX'))
      .toEqual(parseAnsiLines(prefix + remaining + 'X'));
  }
});

it('preserves canonical attributes, defaults, resets and styles across replay boundaries', () => {
  const { intermediate } = captureReplay('\r\x1b[9;8;7;5;4;3;2;1;38;2;255;255;255;48;2;255;255;255mX');
  expect(intermediate).toBe('\x1b[1;2;3;4;5;7;8;9;38;2;255;255;255;48;2;255;255;255mX');
  expect(intermediate.length).toBe(53);
  const input = '\x1b[031;044mabc\n\rde\x1b[039mF\x1b[049mG\x1b[000mH\n\rI';
  expect(parseAnsiLines(input)).toEqual(parseAnsiLines('\x1b[31;44mabc\nde\x1b[39mF\x1b[49mG\x1b[0mH\nI'));
});

it('preserves carriage return, erase, backspace, tabs and wide Unicode cells', () => {
  for (const [input, expected] of [
    ['\x1b[31mabcdef\r\x1b[32mXY', '\x1b[32mXY\x1b[31mcdef\x1b[32m'],
    ['abcdef\rXY\x1b[K', 'XY'], ['abc\bZ', 'abZ'],
    ['A\tB\rX', 'X       B'], ['你ab\r好', '好ab'], ['e\u0301x\rZ', 'Zx'],
  ]) expect(parseAnsiLines(input)).toEqual(parseAnsiLines(expected));
});


it.each([
  ['31', { color: 'var(--dsw-alias-state-error-primary)' }],
  ['91', { color: 'var(--dsw-alias-state-error-secondary)' }],
  ['41', { backgroundColor: 'rgb(187, 0, 0)' }],
  ['101', { backgroundColor: 'rgb(255, 85, 85)' }],
  ['38;5;208', { color: 'rgb(255, 135, 0)' }],
  ['48;5;208', { backgroundColor: 'rgb(255, 135, 0)' }],
  ['38;2;255;128;0', { color: 'rgb(255, 128, 0)' }],
  ['48;2;255;128;0', { backgroundColor: 'rgb(255, 128, 0)' }],
])('renders the supported color %s without losing its semantic value', (color, style) => {
  expect(parseAnsiLines('\r\x1b[' + color + 'mX')).toEqual([[{ text: 'X', style }]]);
});
