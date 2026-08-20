"""The provider-emulator process entry point.

Startup contract, so Cargo and pnpm tests can launch it without scraping
prose:

```text
stdout   exactly two JSON records, one per line:
           {"ready": true, "host": ..., "port": ..., "scenario": ..., "control": ...}
           {"report": {...}}
stderr   human diagnostics only
exit     0 when the scenario is satisfied, 1 otherwise
```

"Satisfied" means every declared step's request matched **and** every
corresponding scripted response reached its intended terminal state, with no
assertion failure. An unexpected request, an unmatched request, a step that
never received its request, and a response still in flight — suspended at an
unreleased gate, for instance — each fail the process.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import signal
import stat
import sys

from fake_provider.control import ScenarioRun
from fake_provider.scenarios import SCENARIOS, build
from fake_provider.server import CONTROL_PREFIX, ProviderServer


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="fake-provider",
        description="The deterministic external scripted provider emulator for rustX.",
    )
    parser.add_argument(
        "--scenario",
        help=f"the scenario to serve; one of: {', '.join(sorted(SCENARIOS))}",
    )
    parser.add_argument(
        "--port",
        type=int,
        default=0,
        help="the loopback port; 0 selects an ephemeral port (the default)",
    )
    parser.add_argument(
        "--host",
        default="127.0.0.1",
        help="the bind address; loopback only",
    )
    parser.add_argument(
        "--workspace",
        default=None,
        help="the absolute workspace root of the runtime under test; "
        "substitutes the {workspace} placeholder of scripted tool-call arguments",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="print the scenario registry as JSON and exit",
    )
    return parser.parse_args(argv)


async def run(scenario_name: str, host: str, port: int, workspace: str | None) -> int:
    scenario = build(scenario_name, workspace)
    state = ScenarioRun(scenario)
    server = ProviderServer(state)
    bound_host, bound_port = await server.start(host, port)

    loop = asyncio.get_running_loop()
    for name in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(name, state.shutdown_requested.set)
    # Closing stdin shuts the process down. A launcher may be killed while a
    # test is failing, and `uv run` is itself a parent process; an EOF on the
    # inherited pipe is the one signal that always arrives, so a failed test
    # can never leave a provider process behind.
    await _shutdown_on_stdin_eof(state)

    print(
        json.dumps(
            {
                "ready": True,
                "host": bound_host,
                "port": bound_port,
                "scenario": scenario.name,
                "control": CONTROL_PREFIX,
                "steps": len(scenario.steps),
                "gates": sorted(state.gates),
            }
        ),
        flush=True,
    )
    print(f"fake-provider: serving {scenario.name} on {bound_host}:{bound_port}", file=sys.stderr)

    await server.serve_until_shutdown()

    report = state.report()
    print(json.dumps({"report": report}), flush=True)
    if not report["ok"]:
        print(
            "fake-provider: scenario FAILED\n"
            + json.dumps(
                {
                    "failures": report["failures"],
                    "unsettledSteps": report["unsettledSteps"],
                },
                indent=2,
            ),
            file=sys.stderr,
        )
    return 0 if report["ok"] else 1


async def _shutdown_on_stdin_eof(state: ScenarioRun) -> None:
    """Requests shutdown when stdin reaches EOF.

    Only a pipe or socket is watched. A terminal, a regular file, or
    `/dev/null` is not a launcher handing over a lifetime, and a reader
    cannot be attached to every one of them portably.
    """
    if not _stdin_is_a_pipe():
        return
    reader = asyncio.StreamReader()
    try:
        await asyncio.get_running_loop().connect_read_pipe(
            lambda: asyncio.StreamReaderProtocol(reader), sys.stdin
        )
    except (ValueError, OSError):  # pragma: no cover - no usable stdin
        return

    async def watch() -> None:
        await reader.read()
        state.shutdown_requested.set()

    asyncio.ensure_future(watch())


def _stdin_is_a_pipe() -> bool:
    if sys.stdin is None or sys.stdin.closed:
        return False
    try:
        mode = os.fstat(sys.stdin.fileno()).st_mode
    except (OSError, ValueError):  # pragma: no cover - no usable stdin
        return False
    return stat.S_ISFIFO(mode) or stat.S_ISSOCK(mode)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.list:
        print(json.dumps({"scenarios": sorted(SCENARIOS)}), flush=True)
        return 0
    if args.scenario is None:
        print("fake-provider: --scenario is required", file=sys.stderr)
        return 2
    if args.host not in ("127.0.0.1", "::1", "localhost"):
        print(
            f"fake-provider: refusing to bind {args.host}; this harness is loopback only",
            file=sys.stderr,
        )
        return 2
    return asyncio.run(run(args.scenario, args.host, args.port, args.workspace))


if __name__ == "__main__":
    raise SystemExit(main())
