# fake-provider — the external scripted provider emulator

The canonical external provider-emulation boundary for rustX Agent Loop
conformance testing (issue #47).

It is an external process that speaks the same real HTTP/SSE provider
protocols rustX uses against actual model providers. rustX itself receives no
fake model, no fake adapter, no alternate Agent Loop, and no model-layer
dependency injection.

```text
Rust / TypeScript test driver
        |  fixed user and runtime actions
        v
real rustX runtime
   Agent Loop | Context Engine | ConversationToolRuntime
   CapabilityCoordinator / Skills | session model state
        |
        v
real rustX provider adapter
        |
        |  real HTTP + SSE / provider wire protocol
        v
fake-provider  (this project)
```

## What it does and does not simulate

It owns exactly five things:

- **provider request validation** — each request is asserted against the
  scenario step it belongs to;
- **ordered scenario progression** — request *N* meets step *N*, always;
- **provider wire responses** — encoded by the protocol codec of the
  protocol the step declares;
- **provider-side synchronization** — named gates and an observation log;
- **request observation** — every request rustX sent, in arrival order.

It deliberately implements **none** of: model intelligence, prompt
understanding, tool execution, Skill discovery or semantics, context
compaction, planning, retry policy, or any other rustX runtime behaviour. It
never reads a workspace file and never decides what a tool should return.
If a scenario needs a tool result, rustX produced it.

## Setup

Python 3.12 and [uv](https://docs.astral.sh/uv/). The Python version is
declared in `pyproject.toml` (`requires-python = "==3.12.*"`); `uv` will
provision it. The lockfile is committed, so every environment resolves
identically.

```bash
cd test-support/fake-provider
uv sync --frozen
```

There are no runtime dependencies. The server speaks raw HTTP/1.1 on
purpose: the scenarios this harness exists for need byte-level control an
ASGI framework hides — a gate *between* two flushed chunks, an abrupt
disconnect at an exact position, deliberately malformed SSE framing, and
observation of the client closing a suspended connection. The only
development dependency is `pytest`.

Python is test-support only. It is never a rustX production runtime
dependency.

## Running

```bash
# the scenario registry
uv run fake-provider --list

# serve one scenario on an ephemeral loopback port
uv run fake-provider --scenario openai_chat_streamed_turn --port 0

# the emulator's own tests
uv run pytest
```

## Process readiness contract

```text
stdout   exactly two JSON records, one per line:
           {"ready": true, "host": ..., "port": ..., "scenario": ...,
            "control": "/__control", "steps": N, "gates": [...]}
           {"report": { ...the final scenario report... }}
stderr   human diagnostics only
exit     0 when the scenario is satisfied, 1 otherwise
```

"Satisfied" has two halves, and both are required:

> every required request has **matched**, in order, **and** every
> corresponding scripted response has reached its intended **terminal
> state**.

A step therefore moves `pending -> matched -> settled` (or `-> failed`), and
matching a request is progression, never completion. An unexpected extra
request, a request that does not match its step, a step that never received
its request, and a response still in flight — suspended at an unreleased
gate, for instance — each keep the report unsuccessful and the exit code
non-zero.

Terminal semantics per response kind:

| Response | Terminal when |
| --- | --- |
| `Stream` / `RawResponse` | every encoded emission has been written and flushed |
| `HttpError` | the status line, headers, and JSON body have been written and flushed |
| scripted `Disconnect` | the disconnect happens — it *is* the intended end, and the rest of the script is unreachable by construction |
| client disconnect, `allow_disconnect=True` | the client goes away — abandoning the stream is the point of that step (cancellation) |
| client disconnect, otherwise | never: it is a scenario failure |

`allow_disconnect` makes a *client* disconnect terminal. It does not excuse
a response still parked at a gate with the client connected: that is neither
settled nor disconnected, so the scenario stays unfinished.

Shutdown is deterministic and happens on any of: `POST /__control/shutdown`,
`SIGTERM`/`SIGINT`, or **EOF on stdin**. The stdin rule is what guarantees a
failed test never leaves a provider process behind — the launcher's pipe
closes even when the launcher itself is killed.

The bind address is loopback only; any other `--host` is refused.

## Control / gate contract

The control surface lives under `/__control` on the same loopback port. It is
test infrastructure, never a rustX runtime API.

| Route | Meaning |
| --- | --- |
| `GET /__control/state` | scenario progress, gate states, failures |
| `GET /__control/requests` | every recorded request, in arrival order |
| `GET /__control/observations` | the ordered observation log |
| `GET /__control/observations/await?kind=…&name=…&count=…&timeoutMs=…` | **barrier**: blocks until the matching observation exists |
| `POST /__control/gates/<name>/release` | releases a suspended response |
| `POST /__control/shutdown` | returns the final report and stops |

Observation kinds: `request_accepted`, `headers_sent`, `chunk_flushed`,
`gate_reached`, `client_disconnected`, `response_completed`,
`scenario_completed`, `assertion_failed`.

`response_completed` carries the terminal reason in its detail
(`script_complete`, `http_error`, `scripted_disconnect`, or
`client_disconnect`), and it is published only when a step actually settles.
`scenario_completed` is published only once **every** step has settled.

`observations/await` is a real barrier — an `asyncio.Condition` inside the
provider process — not a polling loop. `timeoutMs` is deadlock protection
only: it makes a stuck test fail instead of hanging, and it never establishes
ordering.

A gate suspends the response between two flushed chunks:

```text
provider                                  test driver
--------                                  ----------------------------------
write chunk 1, flush   -> chunk_flushed
reach Gate("g")        -> gate_reached ->  await gate_reached("g")   [barrier]
                                           …perform the runtime action…
                       <- release        <- POST /gates/g/release
write chunk 2, flush   -> chunk_flushed
```

Once `gate_reached` is observed, everything before the gate is provably on
the wire and nothing after it can be. That is the whole reason race tests
here never sleep.

## How the tests launch it

**Rust** — `tests/common/provider_emulator.rs` (`ProviderEmulator`) spawns
`uv run --project test-support/fake-provider --frozen fake-provider
--scenario <name> --port 0`, parses the readiness record, exposes the base
URL and the control API, and asserts both the scenario report and the child
exit status on `finish()`.
`tests/issue47_conformance.rs` composes the real `LocalConversationRuntime`
with a catalog pointing at that URL.

**TypeScript** — `tui/test/support/provider-emulator.ts` is the same
launcher for the TUI real-child integration suite, with the same `finish()`
contract (report **and** exit status). It owns process mechanics only; the
TUI has no provider protocol of its own.

Both launchers skip with an explicit reason when `uv` is unavailable. In CI,
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1` turns that skip into a hard failure, so a
broken toolchain can never be reported as a green conformance run.

## Adding a scenario

Scenarios are Python definitions in `src/fake_provider/scenarios/`. Python is
already an adequate DSL for "assert this, then emit that"; a YAML/JSON
mini-language would need its own parser, validation, and failure reporting
for no gain.

```python
from fake_provider.scenario import (
    OPENAI_CHAT_COMPLETIONS, Expect, Finish, Gate, Scenario, Step, Stream, Text, ToolCall,
)

def my_scenario() -> Scenario:
    return Scenario(
        "my_scenario",
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model="chat-model",
                body_contains=("the fixed user input",),
                tools_include=("read",),
            ),
            Stream(ToolCall("call-1", "read", '{"path":"note.txt"}'), Finish("tool_calls")),
        ),
        Step(
            Expect(protocol=OPENAI_CHAT_COMPLETIONS, body_contains=("the real tool result",)),
            Stream(Text("done"), Finish("stop")),
        ),
    )

SCENARIOS = {"my_scenario": my_scenario}
```

Register the module's `SCENARIOS` in `scenarios/__init__.py`, then drive it
from a Rust or TypeScript test. Expectation primitives: `model`, `path`,
`json_subset`, `json_exact`, `body_contains`, `body_excludes`,
`tools_include`, `no_tools`, `headers_present`. Response script items:
`Text`, `Reasoning`, `ToolCall`, `Usage`, `Finish`, `Gate`, `Raw`,
`Disconnect`; responses are `Stream`, `HttpError`, or `RawResponse`.

Keep runtime semantics on the driver side. If a scenario starts wanting to
know *why* rustX sent something, the assertion belongs in the driver.

## Protocol codecs

One codec per protocol boundary rustX supports, never one server per provider
brand. A brand that reuses a protocol reuses its codec.

| Protocol | Path | Codec |
| --- | --- | --- |
| OpenAI Chat Completions | `/v1/chat/completions` | `protocols/openai_chat.py` |
| OpenAI Responses | `/v1/responses` | `protocols/openai_responses.py` |
| Anthropic Messages | `/v1/messages` | `protocols/anthropic_messages.py` |

Scenario semantics stay protocol-neutral (`Text`, `ToolCall`, `Finish`,
`Usage`); the codec owns the wire representation. Each codec emits the
**normal documented lifecycle** of its protocol, not the subset any
particular parser happens to accept — a codec trimmed to what rustX
currently consumes would model the parser rather than the provider, and a
gap in the parser would then be invisible. `Raw` (inside a `Stream`) and
`RawResponse` are the escape hatch for deliberately malformed, truncated, or
compatibility-shaped sequences, and they bypass the codec entirely.
`tests/test_protocols.py` asserts each codec's exact event ordering, so
protocol coverage is a fact about the wire rather than about an enum.

## Relationship to the other test fixtures

```text
tests/scripted/support/model.rs     scripted injected ModelAdapter.
                                    Internal state machines and units that
                                    need no network or provider boundary.

tests/common/mod.rs FixtureServer   raw Rust HTTP fixture. One adapter in
                                    isolation: request serialization, stream
                                    parsing, error normalization,
                                    one-attempt/no-retry. No Agent Loop.

test-support/fake-provider          this project. Composed Agent Loop
                                    conformance across the real runtime and
                                    a real external provider boundary.
```

A test that exercises the Agent Loop, the context engine, the tool runtime,
or the capability plane belongs here. A test that exercises one adapter's
translation belongs in the Rust fixture.

## Out of scope

The M9 cancellation redesign (#12), M10 durability/recovery (#13), and the
live real-provider multi-compaction validation (#27) are owned by their own
issues. This harness gives them deterministic provider gates to build on; it
does not pre-empt their semantics.
