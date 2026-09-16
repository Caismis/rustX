/** Native process startup announcement, not an App Server protocol message. */
export function appServerEndpoint(line: string): string | undefined {
  if (line.length > 512) return undefined;
  return /^rustx app-server listening (ws:\/\/127\.0\.0\.1:\d+)\r?$/.exec(line)?.[1];
}
