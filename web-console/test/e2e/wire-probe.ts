import type { Page } from '@playwright/test';

/** Observe the real socket; fault only a selected acknowledgement after commit.
 * No snapshots, notifications, or semantic results are manufactured here. */
export async function wireProbe(page: Page) {
  const requests: { id: string | number; method: string; params: any }[] = [];
  const responses: { method: string; result: any }[] = [];
  const notifications: { method: string; params: any }[] = [];
  let lose: string | undefined;
  let lost = 0;
  await page.routeWebSocket(/ws:\/\//, socket => {
    const server = socket.connectToServer();
    const methods = new Map<string | number, string>();
    socket.onMessage(message => {
      const request = JSON.parse(String(message));
      if (request.method && request.id !== undefined) {
        requests.push(request); methods.set(request.id, request.method);
      }
      server.send(message);
    });
    server.onMessage(message => {
      const response = JSON.parse(String(message));
      if (response.method) notifications.push(response);
      const method = methods.get(response.id);
      if (method) {
        methods.delete(response.id);
        responses.push({ method, result: response.result });
        if (method === lose && response.result) {
          lose = undefined; lost++;
          // Closing forces immediate uncertainty; no wall-clock request timeout.
          socket.close({ code: 1011, reason: 'Acceptance: committed acknowledgement lost' });
          server.close();
          return;
        }
      }
      socket.send(message);
    });
  });
  return { requests, responses, notifications, loseNext: (method: string) => { lose = method; }, lost: () => lost };
}
