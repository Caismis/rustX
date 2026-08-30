/** Deterministic keyboard/rendering tests for native Session selectors. */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  BoundarySelector,
  SessionSelector,
} from "../src/ui/components/session-selector.ts";
import {
  PopupFrame,
  type PopupContent,
} from "../src/ui/components/popup-frame.ts";
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

  // A Session is unnamed until someone names it, so its row is the first
  // message Rust derived — and a row is searched by whichever of the two it
  // is actually showing.
  it("shows and searches an unnamed Session by its first message", () => {
    const selector = new SessionSelector({
      sessions: [
        {
          id: "session-3",
          preview: "restore the auth module",
          updated_at: "2026-08-19T00:00:00Z",
          active_node: "node-3",
          active: false,
        },
        {
          id: "session-4",
          updated_at: "2026-08-18T00:00:00Z",
          active_node: "node-4",
          active: false,
        },
      ],
    });
    const rendered = selector.render(80).map(plainText).join("\n");
    assert.match(rendered, /restore the auth module/);
    assert.match(rendered, /\(no messages\)/);

    let selected: string | undefined;
    selector.onSelect = (session) => {
      selected = session.id;
    };
    for (const character of "auth") selector.handleInput(character);
    selector.handleInput("\r");
    assert.equal(selected, "session-3");
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
      new PopupFrame(selector).render(120).map(plainText).join("\n"),
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

describe("finite viewports (issue #161)", () => {
  // The selectors must lay out their lists inside the finite body rows
  // PopupFrame allocated, so a selection the user can move to is always
  // visible — never clipped away by the frame. Budget 16 is a realistic
  // constrained popup: a 24-row terminal at the selectors' 70% height.
  function framed(component: PopupContent, budget = 16): PopupFrame {
    const frame = new PopupFrame(component);
    frame.setViewportHeight(budget);
    return frame;
  }

  function interiorRows(frame: PopupFrame): string[] {
    return frame
      .render(80)
      .map(plainText)
      .filter((line) => line.startsWith("│"));
  }

  it("keeps the selected Session visible beyond the first viewport", () => {
    const many = Array.from({ length: 10 }, (_, index) => ({
      id: `session-${index}`,
      name: `work ${index}`,
      updated_at: "2026-08-21T00:00:00Z",
      active_node: `node-${index}`,
      active: false,
    }));
    const selector = new SessionSelector({ sessions: many });
    const frame = framed(selector);
    const chosen: string[] = [];
    selector.onSelect = (session) => chosen.push(session.id);

    // Ten sessions cost twenty physical rows; the first window cannot
    // contain the tail of the list.
    assert.doesNotMatch(interiorRows(frame).join("\n"), /work 9/);

    for (let index = 0; index < 7; index += 1) selector.handleInput("\u001b[B");
    const rows = interiorRows(frame);
    const markerRow = rows.find((line) => line.includes("❯"));
    assert.ok(markerRow?.includes("work 7"), "the selected session is visible and marked");
    // Both physical rows of the selected entry render.
    assert.ok(rows.some((line) => line.includes("session-7 · node node-7")));

    selector.handleInput("\r");
    assert.deepEqual(chosen, ["session-7"], "Enter selects the visible marked item");

    for (let index = 0; index < 7; index += 1) selector.handleInput("\u001b[A");
    assert.ok(
      interiorRows(frame).find((line) => line.includes("❯"))?.includes("work 0"),
      "moving back to the start shows the first session again",
    );
  });

  it("keeps the selected boundary visible, starting from the initial end position", () => {
    const boundaries = Array.from({ length: 10 }, (_, index) => ({
      surface_revision: index + 1,
      message: {
        id: `user-${index}`,
        content: [{ type: "text" as const, text: `approach ${index}` }],
        source: "human" as const,
        kind: "message" as const,
      },
    }));
    const selector = new BoundarySelector({ boundaries, title: "Fork from user message" });
    const frame = framed(selector);
    const chosen: number[] = [];
    selector.onSelect = (value) => chosen.push(value.surface_revision);

    // The initial highlight is the last boundary; it must be visible
    // immediately even though ten boundaries cost twenty physical rows.
    const initial = interiorRows(frame);
    assert.ok(
      initial.find((line) => line.includes("❯"))?.includes("approach 9"),
      "the initially selected last boundary is visible",
    );
    assert.doesNotMatch(initial.join("\n"), /approach 0\D/);

    for (let index = 0; index < 5; index += 1) selector.handleInput("\u001b[A");
    const rows = interiorRows(frame);
    assert.ok(
      rows.find((line) => line.includes("❯"))?.includes("approach 4"),
      "the selected boundary stays visible while moving upward",
    );
    assert.ok(rows.some((line) => line.includes("revision 5")));

    selector.handleInput("\r");
    assert.deepEqual(chosen, [5], "Enter selects the visible marked boundary");
  });

  it("keeps the selected tree entry visible across nodes and branches", () => {
    const nodes = Array.from({ length: 8 }, (_, index) => node(`node-${index}`));
    const boundaries = Array.from({ length: 4 }, (_, index) => boundaryAt(`user-${index}`));
    const selector = new TreeSelector({
      session: sessionView(),
      nodes,
      boundaries,
    });
    const frame = framed(selector);
    const chosen: string[] = [];
    selector.onSelect = (selection) => {
      chosen.push(selection.kind === "node" ? selection.node.id : selection.boundary.message.id);
    };

    // The initial highlight is the last branch boundary and must be visible.
    assert.ok(
      interiorRows(frame).find((line) => line.includes("❯"))?.includes("branch at: user-3"),
      "the initially selected branch is visible",
    );

    // Move six steps up into the node range, beyond the first window.
    for (let index = 0; index < 6; index += 1) selector.handleInput("\u001b[A");
    const rows = interiorRows(frame);
    assert.ok(
      rows.find((line) => line.includes("❯"))?.includes("node-5"),
      "the selected node stays visible while moving upward",
    );
    assert.ok(rows.some((line) => line.includes("conversation-node-5")));

    selector.handleInput("\r");
    assert.deepEqual(chosen, ["node-5"], "Enter selects the visible marked node");
  });
});

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
