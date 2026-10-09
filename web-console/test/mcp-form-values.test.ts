import { expect, it } from 'vitest';
import { formatArguments, parseArguments, parseReferences } from '../src/app/settings/mcp/form-values';

it('round trips argv without losing spaces, empty arguments, quotes or literal variables', () => {
  const args = ['-y', 'server', '--directory', '/a directory/with spaces', '', '$TOKEN', '"quoted"', "it's", 'C:\\MCP\\server.py', '*'];
  expect(parseArguments(formatArguments(args))).toEqual(args);
  expect(parseArguments('-y server --directory "/a b"')).toEqual(['-y','server','--directory','/a b']);
});
it('never interprets shell syntax or expands browser environment variables', () => {
  for (const value of ['$TOKEN','${TOKEN}','server && other','server # ignored']) expect(() => parseArguments(value)).toThrow();
  expect(parseArguments('*.js')).toEqual(['*.js']);
});
it('reference JSON accepts only string environment references, including an explicit empty map', () => {
  expect(parseReferences('')).toEqual({});
  expect(parseReferences('{"Authorization":"$TOKEN"}')).toEqual({Authorization:'$TOKEN'});
  for (const value of ['{broken','[]','null','{"Authorization":"literal"}','{"TOKEN":5}']) expect(() => parseReferences(value)).toThrow();
});
