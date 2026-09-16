// @vitest-environment node
import { expect, it } from 'vitest';
import { parseDogfoodArgs } from '../scripts/dogfood-args';

it('parses the optional scenario independently of the trust flag', () => {
  for (const [args, scenario, trusted] of [
    [[], 'web_console_dogfood', true],
    [['web_chat_history'], 'web_chat_history', true],
    [['--untrusted'], 'web_console_dogfood', false],
    [['web_console_dogfood', '--untrusted'], 'web_console_dogfood', false],
    [['--untrusted', 'web_console_dogfood'], 'web_console_dogfood', false],
    [['web_chat_history', '--untrusted'], 'web_chat_history', false],
    [['--untrusted', 'web_chat_history'], 'web_chat_history', false],
  ] as const) {
    expect(parseDogfoodArgs(args)).toEqual({ scenario, trusted });
  }
});

it('rejects unknown flags and extra scenarios before starting the fixture', () => {
  expect(() => parseDogfoodArgs(['--unknown'])).toThrow('Unknown dogfood launcher flag: --unknown');
  expect(() => parseDogfoodArgs(['web_chat_history', '--unknown'])).toThrow('Unknown dogfood launcher flag: --unknown');
  expect(() => parseDogfoodArgs(['-u'])).toThrow('Unknown dogfood launcher flag: -u');
  expect(() => parseDogfoodArgs(['web_chat_history', 'web_console_dogfood'])).toThrow('Expected at most one dogfood scenario');
});
