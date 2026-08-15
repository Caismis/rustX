"""The deterministic external scripted provider emulator.

This package is *test infrastructure*. It is never imported by rustX
production code, and it deliberately implements no model intelligence: it
validates the provider requests rustX actually sends, and it replays a fixed
ordered script of provider wire responses.
"""

from fake_provider.scenario import (
    Expect,
    Response,
    Scenario,
    Step,
    Chunk,
    Disconnect,
    Finish,
    Gate,
    Raw,
    Reasoning,
    Text,
    ToolCall,
    Usage,
)

__all__ = [
    "Chunk",
    "Disconnect",
    "Expect",
    "Finish",
    "Gate",
    "Raw",
    "Reasoning",
    "Response",
    "Scenario",
    "Step",
    "Text",
    "ToolCall",
    "Usage",
]
