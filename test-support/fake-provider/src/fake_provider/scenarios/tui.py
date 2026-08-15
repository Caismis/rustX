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

INTEGRATION_MODEL = "integration-model"
PROMPT = "hello from the tui"
REPLY = "Hello world"


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


SCENARIOS = {"tui_integration": tui_integration}
