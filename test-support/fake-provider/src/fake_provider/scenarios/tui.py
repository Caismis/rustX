"""The scenario driven by the TUI real-child integration suite.

The TUI owns process mechanics and client assertions. It owns no provider
protocol: the wire bytes below are the same codec every Rust conformance
scenario uses, so there is exactly one provider-emulation implementation in
the repository.
"""

from __future__ import annotations

from fake_provider.scenario import (
    OPENAI_CHAT_COMPLETIONS,
    Expect,
    Finish,
    Scenario,
    Step,
    Stream,
    Text,
    Usage,
)
from fake_provider.scenarios.conformance import SUMMARY_INSTRUCTION

INTEGRATION_MODEL = "integration-model"
PROMPT = "hello from the tui"
REPLY = "Hello world"

# -- repeated compaction over stdio ----------------------------------------

TURN_ONE = "tui compaction: turn one"
TURN_TWO = "tui compaction: turn two"
TURN_THREE = "tui compaction: turn three"
FILLER_ONE_MARKER = "tui-compaction-filler-one-marker-39c1"
FILLER_TWO_MARKER = "tui-compaction-filler-two-marker-84e2"
SUMMARY_ONE_TEXT = "tui summary one: the assistant produced filler report one."
SUMMARY_TWO_TEXT = "tui summary two: the assistant produced filler report two."


def _filler(marker: str) -> str:
    """~200 KB of deterministic filler, worth roughly 53k estimated tokens.

    The compaction describe block publishes a 56k-token window with an 8k
    reserve and a 1k output budget, so the next turn crosses the soft input
    limit while the complete-message compaction span still fits the summary
    model's own request budget. The block text is byte-identical to the Rust
    conformance scenarios' so the same proven margins hold.
    """
    return (marker + " ") + " ".join(
        f"compaction filler block {index:05d}." for index in range(6800)
    )


FILLER_ONE = _filler(FILLER_ONE_MARKER)
FILLER_TWO = _filler(FILLER_TWO_MARKER)


def tui_integration() -> Scenario:
    return Scenario(
        "tui_integration",
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=INTEGRATION_MODEL,
                # The catalog's own request parameters were assembled by the
                # runtime, not by the client.
                json_subset={"stream": True, "temperature": 0.25},
                body_contains=(PROMPT,),
                headers_present=("content-type",),
            ),
            Stream(Text(REPLY, pieces=2), Finish("stop"), Usage(12, 3)),
        ),
    )


def tui_compaction() -> Scenario:
    """Two committed compactions, observed by the real TUI child over stdio.

    The construction mirrors the Rust conformance scenario: turn one fills
    the window, turn two compacts, turn two's answer refills the window, and
    turn three's compaction retires the still-active first summary. Every
    `body_excludes` is the wire proof that retired bytes never reach the
    provider again.
    """
    return Scenario(
        "tui_compaction",
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=INTEGRATION_MODEL,
                body_contains=(TURN_ONE,),
            ),
            Stream(Text(FILLER_ONE), Finish("stop")),
        ),
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=INTEGRATION_MODEL,
                no_tools=True,
                body_contains=(SUMMARY_INSTRUCTION, FILLER_ONE_MARKER),
                body_excludes=(FILLER_TWO_MARKER,),
            ),
            Stream(Text(SUMMARY_ONE_TEXT), Finish("stop")),
        ),
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=INTEGRATION_MODEL,
                body_contains=(SUMMARY_ONE_TEXT, TURN_TWO),
                body_excludes=(FILLER_ONE_MARKER,),
            ),
            Stream(Text(FILLER_TWO), Finish("stop")),
        ),
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=INTEGRATION_MODEL,
                no_tools=True,
                body_contains=(SUMMARY_INSTRUCTION, SUMMARY_ONE_TEXT, FILLER_TWO_MARKER),
                body_excludes=(FILLER_ONE_MARKER,),
            ),
            Stream(Text(SUMMARY_TWO_TEXT), Finish("stop")),
        ),
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model=INTEGRATION_MODEL,
                body_contains=(SUMMARY_TWO_TEXT, TURN_THREE),
                body_excludes=(FILLER_ONE_MARKER, FILLER_TWO_MARKER, SUMMARY_ONE_TEXT),
            ),
            Stream(Text("tui: continuing from the second summary"), Finish("stop")),
        ),
    )


SCENARIOS = {"tui_integration": tui_integration, "tui_compaction": tui_compaction}
