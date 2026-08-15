"""The scenario registry.

Scenarios are Python definitions, not a YAML/JSON mini-language: Python is
already an adequate DSL for "assert this, then emit that", and a bespoke
document format would need its own parser, its own validation, and its own
failure reporting for no gain.

Every scenario is registered by name. `--scenario <name>` selects one, and
`--list` prints the registry.
"""

from __future__ import annotations

from typing import Callable

from fake_provider.scenario import Scenario
from fake_provider.scenarios import conformance, tui

#: name -> factory. A factory builds a fresh scenario per process, so a
#: scenario definition can never accumulate state across runs.
SCENARIOS: dict[str, Callable[[], Scenario]] = {
    **conformance.SCENARIOS,
    **tui.SCENARIOS,
}


def build(name: str) -> Scenario:
    """Builds one registered scenario.

    Raises:
        SystemExit: when the name is not registered, listing what is.
    """
    factory = SCENARIOS.get(name)
    if factory is None:
        raise SystemExit(
            f"unknown scenario {name!r}; registered: {', '.join(sorted(SCENARIOS))}"
        )
    return factory()


__all__ = ["SCENARIOS", "build"]
