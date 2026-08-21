/** Deterministic keyboard/rendering tests for native Session selectors. */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  BoundarySelector,
  SessionSelector,
} from "../src/ui/components/session-selector.ts";
import { TreeSelector } from "../src/ui/components/tree-selector.ts";
import type { SessionNodeView } from "../src/protocol/types.ts";
import { plainText } from "../src/ui/theme.ts";
import { sessionView } from "./support/fixtures.ts";

const sessions = [
  {
    id: "session-1",
    name: "current work",
    updated_at: "2026-08-21T00:00:00Z",
    active_node: "node-1",
    active: true,
  },
  {
    id: "session-2",
    name: "saved review",
    updated_at: "2026-08-20T00:00:00Z",
    active_node: "node-2",
    active: false,
  },
];

const boundary = {
  surface_revision: 4,
  message: {
    id: "user-c",
    content: [{ type: "text" as const, text: "try the alternate approach" }],
    source: "human" as const,
    kind: "message" as const,
  },
};

describe("native Session selectors", () => {
  it("filters persisted Sessions and emits the highlighted selection", () => {
    const selector = new SessionSelector({ sessions });
    let selected: string | undefined;
    selector.onSelect = (session) => {
      selected = session.id;
    };

    for (const character of "saved") selector.handleInput(character);
    selector.handleInput("\r");

    assert.equal(selected, "session-2");
    assert.match(
      selector.render(80).map(plainText).join("\n"),
      /saved review/,
    );
  });

  it("keeps fork selection as a native historical boundary", () => {
    const selector = new BoundarySelector({
      boundaries: [boundary],
      title: "Fork from user message",
    });
    let selectedRevision: number | undefined;
    selector.onSelect = (value) => {
      selectedRevision = value.surface_revision;
    };
    selector.handleInput("\r");

    assert.equal(selectedRevision, 4);
    assert.match(
      selector.render(100).map(plainText).join("\n"),
      /alternate approach/,
    );
  });

  it("distinguishes activating a node from creating a branch", () => {
    const root: SessionNodeView = {
      id: "node-1",
      conversation_id: "conv-test",
      origin: { type: "new" },
    };
    const branch: SessionNodeView = {
      id: "node-2",
      parent: "node-1",
      conversation_id: "conv-2",
      origin: { type: "fork", source_session: "session-1", source_node: "node-1", source_surface_revision: 4, source_user_message: "user-c" },
    };
    const session = sessionView({
      node_count: 2,
    });
    const selector = new TreeSelector({ session, nodes: [root, branch], boundaries: [boundary] });
    const selections: string[] = [];
    selector.onSelect = (selection) => {
      selections.push(selection.kind);
    };

    // The initial highlight is the last item, which is the branch boundary.
    selector.handleInput("\r");
    selector.handleInput("\u001b[A");
    selector.handleInput("\r");

    assert.deepEqual(selections, ["branch", "node"]);
    assert.match(
      selector.render(120).map(plainText).join("\n"),
      /Session tree/,
    );
  });

  it("renders a root node with the root glyph", () => {
    const selector = new TreeSelector({
      session: sessionView(),
      nodes: [{ id: "node-1", conversation_id: "conv-test", origin: { type: "new" } }],
      boundaries: [],
    });
    const rendered = selector.render(120).map(plainText).join("\n");

    assert.match(rendered, /├─ node-1/);
    assert.doesNotMatch(rendered, /└─ node-1/);
  });

  it("keeps an exhausted node stream at its loaded end while history continues", () => {
    const initialNodes = Array.from({ length: 32 }, (_, index) => node(`node-${index}`));
    const initialBoundaries = Array.from({ length: 32 }, (_, index) => boundaryAt(`user-${index}`));
    const selector = new TreeSelector({
      session: sessionView(),
      nodes: initialNodes,
      nextNodeOffset: 32,
      boundaries: initialBoundaries,
      nextHistoryOffset: 32,
    });
    const requests: Array<{ nodeOffset: number; historyOffset: number }> = [];
    const nodeIds = initialNodes.map((item) => item.id);
    const historyIds = initialBoundaries.map((item) => item.message.id);
    selector.onLoadMore = () => {
      const request = selector.nextPageRequest();
      if (request === undefined) return;
      requests.push(request);
      if (requests.length === 1) {
        const pageNodes = Array.from({ length: 8 }, (_, index) => node(`node-${index + 32}`));
        const pageBoundaries = Array.from({ length: 32 }, (_, index) => boundaryAt(`user-${index + 32}`));
        nodeIds.push(...pageNodes.map((item) => item.id));
        historyIds.push(...pageBoundaries.map((item) => item.message.id));
        selector.appendPage({
          nodes: pageNodes,
          nextNodeOffset: undefined,
          boundaries: pageBoundaries,
          nextHistoryOffset: 64,
        });
      } else {
        const pageBoundaries = Array.from({ length: 32 }, (_, index) => boundaryAt(`user-${index + 64}`));
        historyIds.push(...pageBoundaries.map((item) => item.message.id));
        selector.appendPage({
          nodes: [],
          nextNodeOffset: undefined,
          boundaries: pageBoundaries,
          nextHistoryOffset: undefined,
        });
      }
    };

    // The initial highlight is the last row, so one native navigation event
    // starts the first continuation. The second call models the next load
    // after the history stream remains active.
    selector.handleInput("\u001b[B");
    selector.onLoadMore?.();

    assert.deepEqual(requests, [
      { nodeOffset: 32, historyOffset: 32 },
      { nodeOffset: 40, historyOffset: 64 },
    ]);
    assert.equal(new Set(nodeIds).size, nodeIds.length);
    assert.equal(new Set(historyIds).size, historyIds.length);
    assert.equal(selector.nextPageRequest(), undefined);
  });

  it("keeps an exhausted history stream at its loaded end while nodes continue", () => {
    const initialNodes = Array.from({ length: 32 }, (_, index) => node(`node-${index}`));
    const initialBoundaries = Array.from({ length: 32 }, (_, index) => boundaryAt(`user-${index}`));
    const selector = new TreeSelector({
      session: sessionView(),
      nodes: initialNodes,
      nextNodeOffset: 32,
      boundaries: initialBoundaries,
      nextHistoryOffset: 32,
    });
    const requests: Array<{ nodeOffset: number; historyOffset: number }> = [];
    const nodeIds = initialNodes.map((item) => item.id);
    const historyIds = initialBoundaries.map((item) => item.message.id);
    selector.onLoadMore = () => {
      const request = selector.nextPageRequest();
      if (request === undefined) return;
      requests.push(request);
      if (requests.length === 1) {
        const pageNodes = Array.from({ length: 32 }, (_, index) => node(`node-${index + 32}`));
        const pageBoundaries = Array.from({ length: 8 }, (_, index) => boundaryAt(`user-${index + 32}`));
        nodeIds.push(...pageNodes.map((item) => item.id));
        historyIds.push(...pageBoundaries.map((item) => item.message.id));
        selector.appendPage({
          nodes: pageNodes,
          nextNodeOffset: 64,
          boundaries: pageBoundaries,
          nextHistoryOffset: undefined,
        });
      } else {
        const pageNodes = Array.from({ length: 32 }, (_, index) => node(`node-${index + 64}`));
        nodeIds.push(...pageNodes.map((item) => item.id));
        selector.appendPage({
          nodes: pageNodes,
          nextNodeOffset: undefined,
          boundaries: [],
          nextHistoryOffset: undefined,
        });
      }
    };

    selector.handleInput("\u001b[B");
    selector.onLoadMore?.();

    assert.deepEqual(requests, [
      { nodeOffset: 32, historyOffset: 32 },
      { nodeOffset: 64, historyOffset: 40 },
    ]);
    assert.equal(new Set(nodeIds).size, nodeIds.length);
    assert.equal(new Set(historyIds).size, historyIds.length);
    assert.equal(selector.nextPageRequest(), undefined);
  });

  it("stops paired paging when both streams exhaust together", () => {
    const selector = new TreeSelector({
      session: sessionView(),
      nodes: Array.from({ length: 32 }, (_, index) => node(`node-${index}`)),
      nextNodeOffset: 32,
      boundaries: Array.from({ length: 32 }, (_, index) => boundaryAt(`user-${index}`)),
      nextHistoryOffset: 32,
    });
    const requests: Array<{ nodeOffset: number; historyOffset: number }> = [];
    selector.onLoadMore = () => {
      const request = selector.nextPageRequest();
      if (request === undefined) return;
      requests.push(request);
      selector.appendPage({
        nodes: Array.from({ length: 8 }, (_, index) => node(`node-${index + 32}`)),
        nextNodeOffset: undefined,
        boundaries: Array.from({ length: 8 }, (_, index) => boundaryAt(`user-${index + 32}`)),
        nextHistoryOffset: undefined,
      });
    };

    selector.handleInput("\u001b[B");
    selector.onLoadMore?.();

    assert.deepEqual(requests, [{ nodeOffset: 32, historyOffset: 32 }]);
    assert.equal(selector.nextPageRequest(), undefined);
  });
});

function node(id: string): SessionNodeView {
  return {
    id,
    conversation_id: `conversation-${id}`,
    origin: { type: "new" },
  };
}

function boundaryAt(id: string): typeof boundary {
  return {
    surface_revision: Number(id.replace("user-", "")) + 1,
    message: {
      id,
      content: [{ type: "text", text: id }],
      source: "human",
      kind: "message",
    },
  };
}
