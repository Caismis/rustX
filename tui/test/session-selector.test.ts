/** Deterministic keyboard/rendering tests for native Session selectors. */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  BoundarySelector,
  SessionSelector,
} from "../src/ui/components/session-selector.ts";
import { TreeSelector } from "../src/ui/components/tree-selector.ts";
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
    const session = sessionView({
      nodes: [
        ...sessionView().nodes,
        {
          id: "node-2",
          parent: "node-1",
          conversation_id: "conv-2",
          origin: { type: "fork", source_session: "session-1", source_node: "node-1", source_surface_revision: 4, source_user_message: "user-c" },
        },
      ],
    });
    const selector = new TreeSelector({ session, boundaries: [boundary] });
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
    const selector = new TreeSelector({ session: sessionView(), boundaries: [] });
    const rendered = selector.render(120).map(plainText).join("\n");

    assert.match(rendered, /├─ node-1/);
    assert.doesNotMatch(rendered, /└─ node-1/);
  });
});
