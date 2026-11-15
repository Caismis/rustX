"""Concurrent primary and detached Summary conformance over real HTTP."""
from __future__ import annotations

import json

from fake_provider.scenario import (
    AdaptiveScenario, Expect, Finish, Gate, HttpError, Reasoning, Step, Stream, Text, ToolCall, Usage,
    OPENAI_CHAT_COMPLETIONS,
)
from fake_provider.scenarios.conformance import (
    CHAT_MODEL, SECOND_MODEL, SUMMARY_MODEL, CHUNKED_PROMPT, CHUNKED_SUMMARY,
    PRIMARY_REASONING, SUMMARY_REASONING, SUMMARY_INSTRUCTION, _chunked_write_content, summary_json,
)

NEXT_TURN = "async: cancelled later user tail"
FINAL_TURN = "async: fresh final user tail"


def asynchronous_compaction() -> AdaptiveScenario:
    """Bounded, independently selected lanes; arrival order across lanes is unconstrained."""
    def select(requests):
        if len(requests) > 320:
            raise ValueError("asynchronous compaction exceeded its request bound")
        current, prior = requests[-1], requests[:-1]
        primary_count = sum(request.model == CHAT_MODEL for request in prior)
        summary_count = sum(request.model == SUMMARY_MODEL for request in prior)
        if current.model == SUMMARY_MODEL:
            if primary_count < 14:
                raise ValueError("summary began before sufficient tool history")
            native = summary_count == 0
            expected = Expect(
                protocol=OPENAI_CHAT_COMPLETIONS, model=SUMMARY_MODEL, no_direct_tools=True,
                json_subset={"reasoning_effort": "medium", **({"response_format": {"type": "json_schema"}} if native else {})},
                body_contains=(SUMMARY_INSTRUCTION,),
                body_excludes=(PRIMARY_REASONING, SUMMARY_REASONING) if native else
                              (PRIMARY_REASONING, SUMMARY_REASONING, '"response_format"'),
            )
            if native:
                return Step(expected, HttpError(400, {"error": {
                    "type": "invalid_request_error", "code": "unsupported_parameter",
                    "param": "response_format", "message": "response_format is unsupported",
                }})), False
            gate = (Gate("async-summary"),) if summary_count == 1 else ()
            return Step(expected, Stream(*gate, Reasoning(SUMMARY_REASONING),
                                         Text(summary_json(CHUNKED_SUMMARY)), Finish("stop"))), False
        if current.model == CHAT_MODEL:
            if summary_count == 0:
                if primary_count >= 63:
                    raise ValueError("history did not trigger proactive compaction")
                index = primary_count + 1
                return Step(
                    Expect(protocol=OPENAI_CHAT_COMPLETIONS, model=CHAT_MODEL,
                           tools_include=("write",), body_contains=(CHUNKED_PROMPT,)),
                    Stream(Reasoning(PRIMARY_REASONING), ToolCall(
                        f"chunked-call-{index:03d}", "write",
                        json.dumps({"path": f"chunked-{index:03d}.txt", "content": _chunked_write_content(index)})),
                        Finish("tool_calls")),
                ), False
            first_summary = next(i for i, request in enumerate(prior) if request.model == SUMMARY_MODEL)
            if any(request.model == CHAT_MODEL for request in prior[first_summary + 1:]):
                raise ValueError("unexpected additional original-model primary")
            return Step(
                Expect(protocol=OPENAI_CHAT_COMPLETIONS, model=CHAT_MODEL, tools_include=("write",),
                       body_contains=(CHUNKED_PROMPT, "chunked-call-001"), body_excludes=(CHUNKED_SUMMARY,)),
                Stream(Gate("async-primary"), Text("first attempt completed while Summary waits"), Finish("stop")),
            ), False
        if current.model == SECOND_MODEL:
            body = current.body_text
            if FINAL_TURN in body:
                if NEXT_TURN not in body or summary_count <= 1:
                    raise ValueError("final primary lacks the retained tail or Summary")
                return Step(
                    Expect(protocol=OPENAI_CHAT_COMPLETIONS, model=SECOND_MODEL,
                           body_contains=(CHUNKED_SUMMARY, NEXT_TURN, FINAL_TURN),
                           body_excludes=("chunked-call-001", PRIMARY_REASONING, SUMMARY_REASONING)),
                    Stream(Text("asynchronous compaction complete"), Finish("stop")),
                ), True
            return Step(
                Expect(protocol=OPENAI_CHAT_COMPLETIONS, model=SECOND_MODEL,
                       body_contains=(NEXT_TURN, "chunked-call-001"), body_excludes=(CHUNKED_SUMMARY,)),
                Stream(Gate("async-next-primary"), Text("cancelled second primary"), Finish("stop")),
                allow_disconnect=True,
            ), False
        raise ValueError(f"unexpected model {current.model!r}")

    return AdaptiveScenario("asynchronous_compaction", select,
                            ("async-summary", "async-primary", "async-next-primary"))


def asynchronous_compaction_crash() -> AdaptiveScenario:
    """A strict finite prefix whose held clients are expected to die.

    Summary fragments are bounded by the runtime's source-range limit even
    with a large model window. The held primary and first Summary request
    explicitly allow the client disconnect caused by process death.
    """
    def select(requests):
        if len(requests) > 80:
            raise ValueError("crash scenario exceeded its request bound")
        current, prior = requests[-1], requests[:-1]
        primary_count = sum(request.model == CHAT_MODEL for request in prior)
        summary_count = sum(request.model == SUMMARY_MODEL for request in prior)
        if current.model == SUMMARY_MODEL:
            if primary_count < 14 or summary_count >= 256:
                raise ValueError("unexpected crash-scenario Summary request")
            gate = (Gate("crash-summary"),) if summary_count == 0 else ()
            return Step(
                Expect(protocol=OPENAI_CHAT_COMPLETIONS, model=SUMMARY_MODEL,
                       no_direct_tools=True,
                       json_subset={"reasoning_effort": "medium", "response_format": {"type": "json_schema"}},
                       body_contains=(SUMMARY_INSTRUCTION,),
                       body_excludes=(PRIMARY_REASONING, SUMMARY_REASONING)),
                Stream(*gate, Text(summary_json(CHUNKED_SUMMARY)), Finish("stop"), Usage(123, 45)),
                allow_disconnect=True,
            ), False
        if current.model == CHAT_MODEL and summary_count == 0:
            if primary_count >= 63:
                raise ValueError("history did not trigger proactive compaction")
            index = primary_count + 1
            return Step(
                Expect(protocol=OPENAI_CHAT_COMPLETIONS, model=CHAT_MODEL,
                       tools_include=("write",), body_contains=(CHUNKED_PROMPT,)),
                Stream(Reasoning(PRIMARY_REASONING), ToolCall(
                    f"chunked-call-{index:03d}", "write",
                    json.dumps({"path": f"chunked-{index:03d}.txt", "content": _chunked_write_content(index)})),
                    Finish("tool_calls")),
            ), False
        if current.model == CHAT_MODEL and summary_count >= 1:
            first_summary = next(i for i, request in enumerate(prior) if request.model == SUMMARY_MODEL)
            if any(request.model == CHAT_MODEL for request in prior[first_summary + 1:]):
                raise ValueError("unexpected extra primary after Summary admission")
            return Step(
                Expect(protocol=OPENAI_CHAT_COMPLETIONS, model=CHAT_MODEL,
                       tools_include=("write",), body_contains=(CHUNKED_PROMPT, "chunked-call-001"),
                       body_excludes=(CHUNKED_SUMMARY,)),
                Stream(Gate("crash-primary"), Text("held primary"), Finish("stop")),
                allow_disconnect=True,
            ), False
        raise ValueError(f"unexpected request in crash scenario: {current.model!r}")

    return AdaptiveScenario("asynchronous_compaction_crash", select,
                            ("crash-summary", "crash-primary"))


SCENARIOS = {
    "asynchronous_compaction": asynchronous_compaction,
    "asynchronous_compaction_crash": asynchronous_compaction_crash,
}
