export function parseDogfoodArgs(args: readonly string[]) {
  let scenario: string | undefined;
  let trusted = true;
  for (const arg of args) {
    if (arg === '--untrusted') trusted = false;
    else if (arg.startsWith('-')) throw new Error(`Unknown dogfood launcher flag: ${arg}`);
    else if (scenario !== undefined) throw new Error('Expected at most one dogfood scenario');
    else scenario = arg;
  }
  return { scenario: scenario ?? 'web_console_dogfood', trusted };
}
