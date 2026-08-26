/**
 * The semantic transcript contract.
 *
 * These assertions are about *meaning*, not about spacing: that an answer is
 * an answer, that reasoning stays reasoning whether or not it is shown, that
 * a refusal is never dressed up as a reply, and that canonical block order
 * survives the trip to the screen.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { Markdown } from "@earendil-works/pi-tui";

import { reduce } from "../src/presentation/projection.ts";
import { correlateTools } from "../src/presentation/tools.ts";
import { renderTranscript } from "../src/ui/components/transcript.ts";
import { markdownTheme, role } from "../src/ui/theme.ts";
import {
  assistantBlocks,
  assistantMessage,
  attemptModel,
  attemptView,
  runtimeInbound,
  runtimeCursor,
  transcriptCursor,
  toolCallBlock,
  userMessage,
} from "./support/fixtures.ts";
import {
  blockText,
  plain,
  prefs,
  stateOf,
  transcriptString,
  transcriptText,
} from "./support/render.ts";

describe("assistant text", () => {
  it("renders as ordinary Markdown with no repeated banner", () => {
    const state = stateOf({
      messages: [assistantMessage("m1", "# Heading\n\nthe answer")],
    });
    const blocks = renderTranscript(state, prefs());

    assert.equal(blocks.length, 1);
    assert.equal(blocks[0]?.kind, "markdown");
    // The old debug grammar prefixed every text block with `▌ answer`.
    assert.equal(blockText(blocks[0]!), "# Heading\n\nthe answer");
    assert.ok(!transcriptString(state).includes("answer\n"));
    assert.ok(!/▌\s*answer/.test(transcriptString(state)));
  });

  it("renders streaming and committed assistant text identically", () => {
    let streaming = stateOf();
    for (const event of [
      {
        type: "attempt_started" as const,
        attempt_id: "a1",
        model: attemptModel("alpha/model-a"),
      },
      {
        type: "assistant_message_started" as const,
        attempt_id: "a1",
        message_id: "m1",
      },
      {
        type: "assistant_text_delta" as const,
        attempt_id: "a1",
        message_id: "m1",
        block_index: 0,
        delta: "final answer",
      },
    ]) {
      streaming = reduce(streaming, {
        cursor: runtimeCursor(streaming.cursor + 1),
        event,
      });
    }
    const committed = stateOf({
      messages: [assistantMessage("m1", "final answer")],
    });

    // Nothing reflows when the message commits: the same text, the same kind.
    assert.deepEqual(transcriptText(streaming), ["final answer"]);
    assert.deepEqual(transcriptText(committed), ["final answer"]);
  });

  it("keeps a provider failure beside the interrupted answer", () => {
    const state = stateOf({
      messages: [assistantMessage("m1", "partial answer")],
      attempt: {
        ...attemptView(),
        phase: {
          type: "settled",
          outcome: {
            type: "failed",
            error: {
              type: "model",
              kind: "provider_error",
              message: "provider returned a structured failure",
            },
          },
        },
      },
    });
    const rendered = transcriptString(state);

    assert.match(rendered, /partial answer/);
    assert.match(rendered, /request failed · provider_error/);
    assert.match(rendered, /provider returned a structured failure/);
  });

  it("renders runtime failure, cancellation, timeout, and limit outcomes inline", () => {
    const outcomes = [
      {
        outcome: {
          type: "failed" as const,
          error: {
            type: "runtime" as const,
            error: { type: "runtime_failure", message: "runtime detail" },
          },
        },
        expected: /runtime failed · runtime_failure.*runtime detail/s,
      },
      {
        outcome: { type: "cancelled" as const, reason: "user_requested" as const },
        expected: /cancelled · user_requested/,
      },
      {
        outcome: { type: "timed_out" as const },
        expected: /timed out/,
      },
      {
        outcome: { type: "limit_exceeded" as const, limit: "max_turns" as const },
        expected: /limit exceeded · max_turns/,
      },
    ];

    for (const [index, item] of outcomes.entries()) {
      const rendered = transcriptString(
        stateOf({
          messages: [assistantMessage("m1", "interrupted")],
          attempt: {
            ...attemptView({ attempt_id: `attempt-${index}` }),
            phase: { type: "settled", outcome: item.outcome },
          },
        }),
      );
      assert.match(rendered, item.expected);
    }
  });

  it("does not add an error band for a completed attempt", () => {
    const state = stateOf({
      messages: [assistantMessage("m1", "complete")],
      attempt: {
        ...attemptView(),
        phase: {
          type: "settled",
          outcome: { type: "completed", finish_reason: { type: "stop" } },
        },
      },
    });
    assert.deepEqual(transcriptText(state), ["complete"]);
  });

  it("does not infer cancellation from an observable unsettled attempt", () => {
    const rendered = transcriptString(
      stateOf({
        messages: [assistantMessage("m1", "still streaming")],
        attempt: attemptView({ phase: { type: "running" } }),
      }),
    );
    assert.match(rendered, /still streaming/);
    assert.doesNotMatch(rendered, /cancelled|timed out|failed/);
  });

  it("bounds terminal outcome diagnostics", () => {
    const detail = "x".repeat(2_000);
    const rendered = transcriptString(
      stateOf({
        attempt: {
          ...attemptView(),
          phase: {
            type: "settled",
            outcome: {
              type: "failed",
              error: { type: "model", kind: "provider_error", message: detail },
            },
          },
        },
      }),
    );
    assert.ok(rendered.length < 1_000, "the inline diagnostic stays bounded");
    assert.ok(rendered.includes("x".repeat(100)));
    assert.ok(!rendered.includes(detail));
  });
});

describe("reasoning", () => {
  const message = assistantBlocks("m1", [
    { type: "reasoning", text: "step one" },
    { type: "reasoning", text: "step two" },
    { type: "text", text: "the answer" },
  ]);

  it("stays a distinct presentation type, dimmed below the answer", () => {
    const state = stateOf({ messages: [message] });
    const blocks = renderTranscript(state, prefs());

    assert.equal(blocks.length, 2, "the reasoning run groups into one block");
    const [reasoning, answer] = blocks;
    assert.equal(blockText(reasoning!), "step one\n\nstep two");
    assert.equal(blockText(answer!), "the answer");
    assert.equal(
      reasoning?.kind === "markdown"
        ? reasoning.defaultTextStyle?.color?.("x")
        : undefined,
      role.reasoning("x"),
      "reasoning carries the muted role",
    );
    assert.equal(
      answer?.kind === "markdown" ? answer.defaultTextStyle : "not markdown",
      undefined,
      "the answer is primary content and carries no muted style",
    );
  });

  it("reapplies the reasoning style after nested Markdown resets ANSI", () => {
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          { type: "reasoning", text: "before **bold** after `code` tail" },
        ]),
      ],
    });
    const block = renderTranscript(state, prefs())[0];
    assert.equal(block?.kind, "markdown");

    const rendered = new Markdown(
      block.markdown,
      0,
      0,
      markdownTheme,
      block.defaultTextStyle,
    )
      .render(100)
      .join("\n");

    // The reasoning colour is reopened after every nested span that reset
    // it, so the whole run reads as reasoning rather than only its first
    // fragment.
    const reopen = role.reasoning("").split("\u001b[39m")[0]!;
    assert.ok(rendered.includes(`${reopen} after`));
    assert.ok(rendered.includes(`${reopen} tail`));
  });

  it("collapses to a marker when hidden, and never becomes answer text", () => {
    const state = stateOf({ messages: [message] });
    const hidden = renderTranscript(state, prefs({ reasoningVisible: false }));

    assert.equal(hidden.length, 2);
    assert.equal(blockText(hidden[0]!), "Thinking...");
    assert.equal(blockText(hidden[1]!), "the answer");
    // The reasoning body is gone from the screen but was never promoted.
    assert.ok(!transcriptString(state, prefs({ reasoningVisible: false })).includes("step one"));
  });

  it("is a display preference that leaves the projection untouched", () => {
    const state = stateOf({ messages: [message] });
    const before = structuredClone(state);

    renderTranscript(state, prefs({ reasoningVisible: false }));
    renderTranscript(state, prefs({ reasoningVisible: true }));

    // Rendering with either preference mutates nothing the runtime owns.
    assert.deepEqual(state, before);
  });

  it("never invents reasoning the provider did not expose", () => {
    const state = stateOf({
      messages: [assistantBlocks("m1", [{ type: "reasoning" }])],
    });
    assert.match(
      transcriptString(state),
      /the provider exposed no reasoning text/,
    );
  });
});

describe("block ordering", () => {
  it("preserves the canonical sequence of a mixed assistant message", () => {
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          { type: "reasoning", text: "thinking" },
          { type: "text", text: "first" },
          toolCallBlock("call-1", "tool-bash", "bash", { command: "ls" }),
          { type: "text", text: "second" },
        ]),
      ],
    });
    const rendered = transcriptText(state);

    assert.equal(rendered.length, 4);
    assert.equal(rendered[0], "thinking");
    assert.equal(rendered[1], "first");
    assert.match(rendered[2] ?? "", /Bash/);
    assert.equal(rendered[3], "second");
  });

  it("does not merge reasoning across an intervening block", () => {
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          { type: "reasoning", text: "a" },
          { type: "text", text: "middle" },
          { type: "reasoning", text: "b" },
        ]),
      ],
    });
    assert.deepEqual(transcriptText(state), ["a", "middle", "b"]);
  });
});

describe("refusal", () => {
  it("stays distinct from an answer", () => {
    const state = stateOf({
      messages: [
        assistantBlocks("m1", [
          { type: "refusal", text: "I cannot help with that" },
        ]),
      ],
    });
    const rendered = transcriptString(state);

    assert.match(rendered, /refusal/);
    assert.match(rendered, /I cannot help with that/);
    const block = renderTranscript(state, prefs())[0];
    assert.equal(block?.kind, "text", "a refusal is not laid out as an answer");
  });
});

describe("inbound provenance", () => {
  it("labels a runtime-originated turn and leaves a human turn unlabelled", () => {
    const state = stateOf({
      messages: [
        userMessage("m1", "a human turn"),
        runtimeInbound("m2", "a runtime turn"),
      ],
    });
    const blocks = renderTranscript(state, prefs());
    const [human, provenance, runtime] = blocks;

    // A human turn is its own band and carries no label.
    assert.equal(blockText(human!), "a human turn");
    assert.equal(human?.background, "user");
    // A runtime-originated turn is labelled above the band, never disguised
    // as a human one — and it still gets the same band, because it is still
    // an inbound turn.
    assert.equal(blockText(provenance!), "runtime");
    assert.equal(provenance?.background, undefined);
    assert.equal(blockText(runtime!), "a runtime turn");
    assert.equal(runtime?.background, "user");
  });

  it("marks a compaction summary as one", () => {
    const state = stateOf({
      messages: [
        {
          role: "user",
          id: "m1",
          content: [{ type: "text", text: "summary body" }],
          source: "runtime",
          kind: "compaction_summary",
        },
      ],
    });
    assert.match(transcriptString(state), /compaction summary/);
  });

  it("does not render a semantic user echo before durable acceptance", () => {
    assert.doesNotMatch(transcriptString(stateOf()), /just typed/);
  });
});

describe("durable transcript audits", () => {
  const publicationAudit = {
    stream_id: "stream-audit",
    attempt_id: "attempt-1",
    turn_id: "turn-1",
    request_id: "request-1",
    message_id: "provisional-1",
    kind: "incomplete" as const,
    content: [
      {
        kind: "proposed_tool_call" as const,
        block_index: 0,
        call_id: "call-proposed",
        tool_id: "tool-read",
        name: "Read",
        arguments: '{"file_path":".agents/skills/read/SKILL.md"}',
        complete: true,
      },
    ],
    settled_at: "2026-08-24T12:00:00Z",
  };

  it("renders incomplete and unaccepted publication output as noncanonical audit", () => {
    const state = stateOf({
      transcript: {
        entries: [
          {
            cursor: transcriptCursor(1),
            item: { type: "publication_audit", audit: publicationAudit },
          },
          {
            cursor: transcriptCursor(2),
            item: {
              type: "publication_audit",
              audit: { ...publicationAudit, stream_id: "stream-unaccepted", kind: "unaccepted" },
            },
          },
        ],
      },
    });
    const blocks = renderTranscript(state, prefs());
    const rendered = blocks.map(blockText).join("\n");

    assert.match(rendered, /assistant output · incomplete · noncanonical/);
    assert.match(rendered, /assistant output · unaccepted · noncanonical/);
    assert.match(rendered, /proposed tool call · Read · not accepted · not executed/);
    assert.match(rendered, /file_path/);
    assert.equal(
      blocks.every((block) => block.kind === "text"),
      true,
      "a publication audit never becomes a ToolCard or foreground Tool Plane row",
    );
    assert.equal(correlateTools(state).byCallId.size, 0);
  });

  it("renders historical interaction audits without restoring an actionable prompt", () => {
    const state = stateOf({
      transcript: {
        entries: [
          {
            cursor: transcriptCursor(3),
            item: {
              type: "interaction_requested",
              event_id: "interaction-requested-event",
              timestamp: "2026-08-24T12:00:00Z",
              attempt_id: "attempt-1",
              turn_id: "turn-1",
              interaction_id: "interaction-1",
              subject: {
                type: "questionnaire",
                questionnaire: {
                  questions: [
                    {
                      question: "Which environment?",
                      header: "Environment",
                      options: [
                        { label: "staging", description: "A safe test environment." },
                        { label: "production", description: "The live environment." },
                      ],
                      multi_select: false,
                    },
                  ],
                },
              },
            },
          },
          {
            cursor: transcriptCursor(4),
            item: {
              type: "interaction_settled",
              event_id: "interaction-settled-event",
              timestamp: "2026-08-24T12:00:01Z",
              attempt_id: "attempt-1",
              turn_id: "turn-1",
              interaction_id: "interaction-1",
              settlement: {
                type: "questionnaire_submitted",
                submission: {
                  answers: [
                    {
                      question_index: 0,
                      answer: { type: "single_option", value: { label: "staging" } },
                    },
                  ],
                },
              },
            },
          },
        ],
      },
    });
    const rendered = transcriptString(state);

    assert.match(rendered, /historical interaction · requested · not actionable/);
    assert.match(rendered, /Which environment/);
    assert.match(rendered, /historical interaction · settled · questionnaire submitted/);
    assert.match(rendered, /single_option/);
    assert.doesNotMatch(rendered, /respond|pending prompt|approve action/i);
  });
});
