// @vitest-environment node
import { expect, it } from 'vitest';
import { parseDogfoodArgs } from '../scripts/dogfood-args';

it('parses one optional scenario without a Workspace trust mode', () => {
  for (const [args, scenario] of [
    [[], 'web_console_dogfood'],
    [['web_chat_history'], 'web_chat_history'],
  ] as const) {
    expect(parseDogfoodArgs(args)).toEqual({ scenario });
  }
});

it('rejects unknown flags and extra scenarios before starting the fixture', () => {
  expect(() => parseDogfoodArgs(['--untrusted'])).toThrow('Unknown dogfood launcher flag');
  expect(() => parseDogfoodArgs(['--unknown'])).toThrow('Unknown dogfood launcher flag: --unknown');
  expect(() => parseDogfoodArgs(['web_chat_history', '--unknown'])).toThrow('Unknown dogfood launcher flag: --unknown');
  expect(() => parseDogfoodArgs(['-u'])).toThrow('Unknown dogfood launcher flag: -u');
  expect(() => parseDogfoodArgs(['web_chat_history', 'web_console_dogfood'])).toThrow('Expected at most one dogfood scenario');
});
