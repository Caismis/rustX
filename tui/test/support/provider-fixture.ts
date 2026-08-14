/**
 * A deterministic local provider fixture for the real-child integration test.
 *
 * This is a **test double for a model provider**, not client-side provider
 * logic: it serves a fixed SSE body so the spawned rustX process exercises its
 * own real adapter, its own credential resolution, and its own streaming path
 * with no network and no credential in CI.
 *
 * It lives under `test/` for exactly that reason. Nothing in `src/` speaks
 * HTTP, resolves an endpoint, or knows a provider wire format.
 */

import { createServer, type Server } from "node:http";
import { once } from "node:events";
import type { AddressInfo } from "node:net";

/** A minimal OpenAI Chat Completions stream that says "Hello world". */
export const PLAIN_TEXT_SSE = [
  'data: {"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"gpt-test","choices":[{"index":0,"delta":{"role":"assistant","content":"Hello"},"finish_reason":null}]}',
  "",
  'data: {"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"gpt-test","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":null}]}',
  "",
  'data: {"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"gpt-test","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}',
  "",
  'data: {"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"gpt-test","choices":[],"usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}}',
  "",
  "data: [DONE]",
  "",
  "",
].join("\n");

export class ProviderFixture {
  readonly #server: Server;
  readonly #bodies: string[] = [];
  #port = 0;

  private constructor(server: Server) {
    this.#server = server;
  }

  /** Starts the fixture on an ephemeral loopback port. */
  static async start(sse: string = PLAIN_TEXT_SSE): Promise<ProviderFixture> {
    const server = createServer((request, response) => {
      const chunks: Buffer[] = [];
      request.on("data", (chunk: Buffer) => chunks.push(chunk));
      request.on("end", () => {
        fixture.#bodies.push(Buffer.concat(chunks).toString("utf8"));
        response.writeHead(200, {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
        });
        response.end(sse);
      });
    });
    const fixture = new ProviderFixture(server);

    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    fixture.#port = (server.address() as AddressInfo).port;
    return fixture;
  }

  /** The base URL to configure in the runtime's model catalog. */
  url(path = "/v1"): string {
    return `http://127.0.0.1:${this.#port}${path}`;
  }

  /** Every provider request body the runtime actually sent. */
  get requestBodies(): readonly string[] {
    return this.#bodies;
  }

  async stop(): Promise<void> {
    this.#server.close();
    this.#server.closeAllConnections?.();
    await once(this.#server, "close");
  }
}
