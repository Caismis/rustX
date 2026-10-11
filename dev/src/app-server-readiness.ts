/** Native process startup announcement, not an App Server protocol message. */
export function appServerEndpoint(line: string): string | undefined {
  if (line.length > 512) return undefined;
  const match = /^rustx app-server listening ws:\/\/127\.0\.0\.1:(\d+)\r?$/.exec(line);
  if (!match) return undefined;
  const port = Number(match[1]);
  return Number.isInteger(port) && port > 0 && port <= 65535 ? `ws://127.0.0.1:${port}` : undefined;
}
