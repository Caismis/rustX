"""The Agent Loop conformance scenarios driven from `tests/issue47_conformance.rs`.

Each scenario fixes the provider half of one interaction. The rustX half —
the user input, the workspace fixture, the runtime action, and every
assertion about runtime state — belongs to the Rust driver. The two halves
meet only through real HTTP.

The literals below are the contract between the two sides. The Rust driver
mirrors them as constants, and a mismatch fails the scenario rather than
passing quietly, because every expectation here is asserted by default.
"""

from __future__ import annotations

from fake_provider.scenario import (
    ANTHROPIC_MESSAGES,
    OPENAI_CHAT_COMPLETIONS,
    OPENAI_RESPONSES,
    Expect,
    Finish,
    Gate,
    HttpError,
    Reasoning,
    Scenario,
    Step,
    Stream,
    Text,
    ToolCall,
    Usage,
)

# -- the catalog the Rust driver publishes ---------------------------------

CHAT_MODEL = "chat-model"
RESPONSES_MODEL = "responses-model"
ANTHROPIC_MODEL = "anthropic-model"
SUMMARY_MODEL = "summary-model"
SECOND_MODEL = "second-model"

# -- fixed user inputs -----------------------------------------------------

TURN_ONE = "conformance: turn one"
READ_PROMPT = "conformance: read the note"
SKILL_PROMPT = "conformance: follow the conformance skill"
TURN_TWO = "conformance: turn two"
FIRST_ATTEMPT = "conformance: first attempt"
SECOND_ATTEMPT = "conformance: second attempt"

# -- workspace fixtures the Rust driver writes -----------------------------

NOTE_PATH = "note.txt"
NOTE_MARKER = "deterministic-note-payload-6d41"
SKILL_NAME = "conformance-skill"
SKILL_DESCRIPTION = "The deterministic workspace Skill of the issue 47 conformance harness."
SKILL_PATH = f".agents/skills/{SKILL_NAME}/SKILL.md"
SKILL_BODY_MARKER = "skill-body-marker-a17c"

# -- compaction ------------------------------------------------------------

COMPACTION_MARKER = "compaction-filler-marker-93be"
#: ~200 KB of deterministic filler. The Rust driver configures a 40k-token
#: window with a 1k reserve and a 1k output budget, so the soft input limit
#: is ~152 KB of serialized projection: turn one sits far below it and turn
#: two sits far above. The threshold is crossed by construction, never by
#: hoping a random amount of text happens to be enough.
FILLER = (COMPACTION_MARKER + " ") + " ".join(
    f"compaction filler block {index:05d}." for index in range(6800)
)
SUMMARY_TEXT = "conformance summary: the assistant produced one long report."
#: The exact deterministic prefix `ModelBackedSummarizer` sends. A summary
#: request is identified structurally, never by guessing at its content.
SUMMARY_INSTRUCTION = "Summarize the following conversation history for continuation."


def _text_turn(name: str, protocol: str, model: str, prompt: str) -> Scenario:
    """One streamed assistant turn over the given protocol.

    The script leads with a reasoning block so the protocol's full normal
    lifecycle — not just its text subset — is exercised against the real
    rustX stream parser.
    """
    return Scenario(
        name,
        Step(
            Expect(
                protocol=protocol,
                model=model,
                json_subset={"stream": True},
                body_contains=(prompt,),
                tools_include=("read", "bash"),
            ),
            Stream(
                Reasoning("the conformance plan"),
                Text("Hello world", pieces=2),
                Finish("stop"),
                Usage(41, 3),
            ),
        ),
    )


def openai_chat_streamed_turn() -> Scenario:
    return _text_turn(
        "openai_chat_streamed_turn", OPENAI_CHAT_COMPLETIONS, CHAT_MODEL, TURN_ONE
    )


def openai_responses_streamed_turn() -> Scenario:
    return _text_turn(
        "openai_responses_streamed_turn", OPENAI_RESPONSES, RESPONSES_MODEL, TURN_ONE
    )


def anthropic_streamed_turn() -> Scenario:
    return _text_turn(
        "anthropic_streamed_turn", ANTHROPIC_MESSAGES, ANTHROPIC_MODEL, TURN_ONE
    )


def tool_call_continuation() -> Scenario:
    """Provider tool call -> real rustX Read -> provider continuation.

    The emulator requests the call and inspects the continuation. It never
    reads the file, never fabricates the result, and never knows what rustX's
    Read tool does.
    """
    return Scenario(
        "tool_call_continuation",
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=CHAT_MODEL,
                body_contains=(READ_PROMPT,),
                tools_include=("read",),
            ),
            Stream(
                ToolCall("call-read-1", "read", f'{{"path":"{NOTE_PATH}"}}', pieces=2),
                Finish("tool_calls"),
            ),
        ),
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=CHAT_MODEL,
                # The continuation carries the *real* tool result, produced
                # by rustX's own Read through ConversationToolRuntime.
                body_contains=(READ_PROMPT, "call-read-1", NOTE_MARKER),
            ),
            Stream(Text(f"the note says {NOTE_MARKER}"), Finish("stop")),
        ),
    )


def skill_read_turn() -> Scenario:
    """A real workspace Skill, discovered and projected by rustX alone."""
    return Scenario(
        "skill_read_turn",
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=CHAT_MODEL,
                # The Skill catalog reached the provider as system context.
                body_contains=(SKILL_PROMPT, SKILL_NAME, SKILL_DESCRIPTION),
                tools_include=("read",),
            ),
            Stream(
                ToolCall("call-skill-1", "read", f'{{"path":"{SKILL_PATH}"}}'),
                Finish("tool_calls"),
            ),
        ),
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=CHAT_MODEL,
                body_contains=("call-skill-1", SKILL_BODY_MARKER),
            ),
            Stream(Text("the skill instructions were read"), Finish("stop")),
        ),
    )


def provider_http_error() -> Scenario:
    """A deterministic provider failure with no retry and no extra turn."""
    return Scenario(
        "provider_http_error",
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=CHAT_MODEL,
                body_contains=(TURN_ONE,),
            ),
            HttpError(
                500,
                {
                    "error": {
                        "message": "the scripted provider is unavailable",
                        "type": "server_error",
                        "code": "internal_error",
                    }
                },
            ),
        ),
    )


def gated_stream_cancellation() -> Scenario:
    """A stream suspended at an explicit gate for a cancellation test.

    The driver waits for the gate, cancels, and waits for the provider to
    observe the disconnect. Nothing sleeps, and the cancellation provably
    lands after the first delta and before anything the gate holds back.
    """
    return Scenario(
        "gated_stream_cancellation",
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=CHAT_MODEL,
                body_contains=(TURN_ONE,),
            ),
            Stream(
                Text("partial"),
                Gate("before-remaining-text"),
                Text(" remainder that cancellation must prevent"),
                Finish("stop"),
            ),
            allow_disconnect=True,
        ),
    )


def _compaction(name: str, summary_model: str) -> Scenario:
    """Turn one fills the window, turn two must compact through the provider."""
    return Scenario(
        name,
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=CHAT_MODEL,
                body_contains=(TURN_ONE,),
            ),
            Stream(Text(FILLER), Finish("stop")),
        ),
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=summary_model,
                # A summary invocation is structurally distinct: no tools, no
                # Agent Status, no Skill catalog, and the runtime's own
                # deterministic instruction.
                no_tools=True,
                body_contains=(SUMMARY_INSTRUCTION, COMPACTION_MARKER),
            ),
            Stream(Text(SUMMARY_TEXT), Finish("stop")),
        ),
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=CHAT_MODEL,
                tools_include=("read",),
                # The compacted projection reached the primary model: the
                # summary is present and the retired filler is gone.
                body_contains=(SUMMARY_TEXT, TURN_TWO),
                body_excludes=(COMPACTION_MARKER,),
            ),
            Stream(Text("continuing from the summary"), Finish("stop")),
        ),
    )


def compaction_session_summary() -> Scenario:
    return _compaction("compaction_session_summary", CHAT_MODEL)


def compaction_explicit_summary() -> Scenario:
    return _compaction("compaction_explicit_summary", SUMMARY_MODEL)


def frozen_attempt_model() -> Scenario:
    """The immutable attempt model snapshot, observed from outside rustX.

    The provider suspends the first attempt at a gate; the driver switches
    the session model while that attempt is in flight. The frozen model is
    externally observable because the emulator asserts the `model` field of
    each request.
    """
    return Scenario(
        "frozen_attempt_model",
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=CHAT_MODEL,
                body_contains=(FIRST_ATTEMPT,),
            ),
            Stream(
                Text("frozen"),
                Gate("session-model-updated"),
                Text(" attempt"),
                Finish("stop"),
            ),
        ),
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=SECOND_MODEL,
                body_contains=(SECOND_ATTEMPT,),
            ),
            Stream(Text("second attempt"), Finish("stop")),
        ),
    )


SCENARIOS = {
    "openai_chat_streamed_turn": openai_chat_streamed_turn,
    "openai_responses_streamed_turn": openai_responses_streamed_turn,
    "anthropic_streamed_turn": anthropic_streamed_turn,
    "tool_call_continuation": tool_call_continuation,
    "skill_read_turn": skill_read_turn,
    "provider_http_error": provider_http_error,
    "gated_stream_cancellation": gated_stream_cancellation,
    "compaction_session_summary": compaction_session_summary,
    "compaction_explicit_summary": compaction_explicit_summary,
    "frozen_attempt_model": frozen_attempt_model,
}
