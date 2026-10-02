"""Bounded browser integration script. Gates control provider emission only.

No rustX operation, interaction lifetime, or Tool behavior is simulated here.
"""
import json
from fake_provider.scenario import (
    OPENAI_CHAT_COMPLETIONS, Expect, Finish, Gate, Scenario, Step, Stream, Text, ToolCall, Usage,
)


def web_console_dogfood() -> Scenario:
    def expected(prompt: str) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(prompt,))

    questions = json.dumps({"questions": [{
        "header": "Direction", "question": "Keep runtime authority in rustX?",
        "options": [{"label": "Keep native (Recommended)", "description": "Use rustX owners."},
                    {"label": "Inspect first", "description": "Read the protocol."}],
        "multi_select": False,
    }]})
    return Scenario(
        "web_console_dogfood",
        Step(expected("Long action in A"), Stream(Text("A is running."), Gate("finish-a"), Text(" A finished."), Finish())),
        Step(expected("Use B while A runs"), Stream(Text("B stayed responsive."), Finish())),
        Step(expected("Approval please"), Stream(ToolCall("console-bash", "bash", json.dumps({
            "command": "printf console-approved; printf x >> console-effect", "execution_mode": "foreground",
        })), Finish("tool_calls"))),
        Step(expected("console-approved"), Stream(Text("Approval completed."), Finish())),
        Step(expected("Questionnaire please"), Stream(ToolCall("console-question", "ask_user", questions), Finish("tool_calls"))),
        Step(expected("Keep native"), Stream(Text("Questionnaire completed."), Finish())),
        Step(expected("Publish while detached"), Stream(Text("Preparing a question."), Gate("publish-question"),
             ToolCall("console-detached-question", "ask_user", questions), Finish("tool_calls"))),
        Step(expected("Keep native"), Stream(Text("Detached question completed."), Finish())),
    )


SCENARIOS = {"web_console_dogfood": web_console_dogfood}


def web_chat_history() -> Scenario:
    """Thirty-four accepted turns cross the native 64-entry bootstrap boundary."""
    steps = [Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(f"History {i}",)),
                  Stream(Text(f"Answer {i}"), Finish())) for i in range(34)]
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Rich reply",)),
                      Stream(Text("## Rich reply\n\n| Key | Value |\n| --- | --- |\n| native | history |\n\n```rust\nfn main() {}"),
                             Gate("settle-chat"), Text("\n```\n\n- **Settled**\n\n$x^2$"), Finish())))
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Keep this draft", "user_uploaded_files", "note.txt", ".agents/uploads/")),
                      Stream(Text("Uploaded workspace file received."), Finish())))
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Image please",)),
                      Stream(ToolCall("chat-image", "render_image", "{}"), Finish("tool_calls"))))
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Image artifact", "chat-image")), Stream(Text("Image reference retained for a vision-capable model."), Finish("stop"))))
    return Scenario("web_chat_history", *steps)


SCENARIOS["web_chat_history"] = web_chat_history


def web_commands() -> Scenario:
    """Real native retry consumes the returned boundary input, never old output."""
    def expected(model: str) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model=model,
                      body_contains=("Regenerate my uploaded note", "user_uploaded_files", "note.txt", ".agents/uploads/"),
                      body_excludes=("Original native answer", "Regenerated native answer"))
    return Scenario("web_commands",
                    Step(expected("second-model"), Stream(Text("Original native answer"), Finish(), Usage(100, 20))),
                    Step(expected("second-model"), Stream(Gate("retry-request-reached"), Text("Regenerated native answer"), Finish())),
                    Step(expected("second-model"), Stream(Gate("inherited-retry-reached"), Text("Inherited replay answer"), Finish())))


SCENARIOS["web_commands"] = web_commands


def web_composer_context() -> Scenario:
    """Native Todo and Goal owners feed the composer docks; one Human input waits in the mailbox.

    The provider only emits model calls. The Todo list, Goal state, continuation
    round admission and the pending inbound row all belong to rustX.
    """
    def expected(text: str) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(text,))

    def todo(call: str, arguments: dict[str, object]) -> ToolCall:
        return ToolCall(call, "todo", json.dumps(arguments))

    return Scenario(
        "web_composer_context",
        Step(expected("Plan the composer docks"), Stream(
            todo("todo-bind", {"action": "create", "subject": "Bind native Todo"}),
            todo("todo-goal", {"action": "create", "subject": "Render the Goal dock", "blocked_by": [1]}),
            todo("todo-start", {"action": "update", "id": 1, "status": "in_progress", "active_form": "Binding native Todo"}),
            Finish("tool_calls"))),
        Step(expected("Binding native Todo"), Stream(Text("Plan recorded."), Finish())),
        Step(expected("Keep working until the docks are verified"), Stream(ToolCall("goal-create", "create_goal", json.dumps(
            {"objective": "Verify the composer docks", "autonomous_round_budget": 1})), Finish("tool_calls"))),
        Step(expected("Verify the composer docks"), Stream(Text("Goal recorded."), Finish())),
        Step(expected("Continue pursuing the current Goal"), Stream(
            Text("Continuing the Goal."), Gate("goal-round"), Text(" Round finished."), Finish())),
        Step(expected("Queued during the Goal round"), Stream(Text("Queued input handled."), Finish())),
    )


SCENARIOS["web_composer_context"] = web_composer_context


def web_upload_conformance() -> Scenario:
    """The model requests ordinary workspace Tool IO, before/after source deletion.

    The glob only discovers the server-chosen batch name. The browser separately
    asserts the model request's exact absolute path against the upload receipt.
    No ArtifactStore or emulator filesystem reads participate.
    """
    command = "for f in .agents/uploads/*/*/acceptance.txt; do printf '%s\\n' \"$PWD/$f\"; cat \"$f\"; done"
    steps = []
    for phase in ("Source", "Destination"):
        steps.extend([
            Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                        body_contains=("Use my uploaded files", "<user_uploaded_files>", "acceptance.txt")),
                 Stream(ToolCall(f"upload-{phase.lower()}", "bash", json.dumps({
                     "command": command, "execution_mode": "foreground",
                 })), Finish("tool_calls"))),
            Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                        body_contains=("UPLOAD_NATIVE_SENTINEL", ".agents/uploads/")),
                 Stream(Text(f"{phase} upload read through native Tool."), Finish())),
        ])
    return Scenario("web_upload_conformance", *steps)


SCENARIOS["web_upload_conformance"] = web_upload_conformance


def web_workflow_conformance() -> Scenario:
    # Reuse the native conformance script, including its child-admission gate.
    from dataclasses import replace
    from .conformance import workflow_output
    return Scenario("web_workflow_conformance", *(
        replace(step, expect=replace(step.expect, model="console-model"))
        for step in workflow_output().steps
    ))


SCENARIOS["web_workflow_conformance"] = web_workflow_conformance


def web_session_archive() -> Scenario:
    """One settled native history shared by browser and remote TUI exports."""
    return Scenario("web_session_archive", Step(
        Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Archive this Session",)),
        Stream(Text("Archive fixture settled."), Finish()),
    ))


SCENARIOS["web_session_archive"] = web_session_archive


def web_trace_convergence() -> Scenario:
    """Request input is frozen before the save/adoption gates in the browser suite."""
    return Scenario(
        "web_trace_convergence",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Before adoption",)),
             Stream(Text("Frozen before configuration."), Gate("trace-before"), Finish())),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("After adoption", "TRACE_NEW_INSTRUCTIONS")),
             Stream(Text("Frozen after explicit adoption."), Finish())),
    )


SCENARIOS["web_trace_convergence"] = web_trace_convergence


def web_harness_convergence() -> Scenario:
    """Real native Tool and status projection for the Issue #402 browser flow."""
    from fake_provider.scenario import Reasoning
    expected = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="second-model",
                      body_contains=("Converge the conversation",))
    return Scenario(
        "web_harness_convergence",
        Step(expected, Stream(Reasoning("## Plan\n\n- Inspect the workspace.\n- Keep **native authority**.\n\n`exact identity`"),
                              Text("I will prepare a small workspace change."),
                              ToolCall("write-402", "write", json.dumps({"path": "hello.txt", "content": "hello native world\n"})), Finish("tool_calls"))),
        Step(expected, Stream(Reasoning("Verify the committed file before answering."),
                              ToolCall("bash-402", "bash", json.dumps({"command": "cat hello.txt", "execution_mode": "foreground"})), Finish("tool_calls"))),
        Step(expected, Stream(ToolCall("read-402", "read", json.dumps({"path": "hello.txt"})), Finish("tool_calls"))),
        Step(expected, Stream(Text("## Workspace ready\n\nThe file was written and verified through native Tools."), Finish())),
    )


SCENARIOS["web_harness_convergence"] = web_harness_convergence


def web_agent_continuation() -> Scenario:
    """Hold real native parent/child requests, then interrupt and resume the child.

    Parent continuation and first child request can arrive in either order.
    The browser observes recorded input to release the correct provider gate;
    all Agent identity, cancellation, settlement, and continuation is native.
    """
    return Scenario(
        "web_agent_continuation",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("WEB_AGENT_PARENT",), tools_include=("subagent",)),
             Stream(ToolCall("web-agent-create", "subagent", json.dumps({
                 "agent": "reviewer", "task": "WEB_AGENT_CHILD: review the workspace",
             })), Finish("tool_calls"))),
        *(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model"),
               Stream(Gate(f"agent-initial-{index}"), Text("Initial request completed."), Finish()),
               allow_disconnect=True) for index in range(2)),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("WEB_AGENT_RESUME",), body_excludes=("WEB_AGENT_PARENT",)),
             Stream(Gate("agent-resumed"), Text("Resumed canonical child report."), Finish())),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("WEB_AGENT_PARENT", "Resumed canonical child report.")),
             Stream(Text("Parent received the resumed child report."), Finish())),
    )


SCENARIOS["web_agent_continuation"] = web_agent_continuation


def web_reading_surface() -> Scenario:
    """300 real native Attempts exceed 512 transcript entries; gates prove follow."""
    steps = [Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                         body_contains=(f"Reading {i}",)),
                  Stream(Text(f"Native reading answer {i}\n\n" + "Readable history paragraph."), Finish()))
             for i in range(300)]
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                             body_contains=("Stream reading",)),
                      Stream(Text("Reading stream begins.\n\n"), Gate("reading-detached"),
                             Text("Detached output.\n\n" + "\n\n".join(
                                 f"Measured paragraph {i} retains position at checkpoint {i * i + 17}."
                                 for i in range(60))), Gate("reading-latest"),
                             Text("Following output.\n\n" + "\n\n".join(
                                 f"Live paragraph {i} advances the reading tail through checkpoint {i * i + 29}."
                                 for i in range(60))), Finish())))
    return Scenario("web_reading_surface", *steps)


SCENARIOS["web_reading_surface"] = web_reading_surface


def web_file_delivery() -> Scenario:
    """Ordinary Write/Bash/present Tool settlement; the browser owns no file facts."""
    markdown = '# Delivered report\r\n\r\n**Original** Unicode bytes.\r\n\r\n<script>globalThis.PWNED=1</script>\r\n\r\n![blocked](https://example.org/tracker)\r\n'
    script = """import base64,json,pathlib
p=pathlib.Path('.')
(p/'plain 空格.txt').write_bytes(b'plain\\r\\noriginal\\r\\n')
(p/'source.rs').write_bytes(b'fn main() { println!(\"native\"); }\\n')
(p/'pixel.png').write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII='))
(p/'data.bin').write_bytes(bytes([0,1,255,128,10]))
(p/'large.txt').write_bytes(b'x'*(300*1024))
for name in ('a','b'):
 (p/name).mkdir()
 (p/name/'report.md').write_text('Ambiguous basename')
print(json.dumps({'deliveries':[{'path':'a/report.md'}]}))
"""
    files = [{'path': name, 'description': description} for name, description in [
        ('报告 file.md', 'Explicit Markdown report'), ('plain 空格.txt', 'Created by Bash'),
        ('source.rs', 'Source code'), ('pixel.png', 'Raster image'),
        ('data.bin', 'Unsupported inline format'), ('large.txt', 'Above the managed Artifact threshold'),
    ]]
    expected = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model='console-model', body_contains=('Deliver files explicitly',))
    def tool(call: str, name: str, arguments: dict[str, object]) -> Stream:
        return Stream(ToolCall(call, name, json.dumps(arguments, ensure_ascii=False)), Finish('tool_calls'))
    return Scenario('web_file_delivery',
        Step(expected, tool('delivery-write', 'write', {'path': '报告 file.md', 'content': markdown})),
        Step(expected, tool('delivery-bash', 'bash', {'command': "python3 - <<'PY'\n" + script + 'PY\n', 'execution_mode': 'foreground'})),
        Step(expected, Stream(Text('I created report.md and 报告 file.md.'), Gate('delivery-before-declaration'),
                              ToolCall('delivery-malformed', 'present', json.dumps({'files': [{'path': '../escape'}]})), Finish('tool_calls'))),
        Step(expected, tool('delivery-present', 'present', {'files': files + [{'path': './报告 file.md', 'description': 'Duplicate discarded'}]})),
        Step(expected, tool('delivery-repeat', 'present', {'files': [{'path': '报告 file.md', 'description': 'Repeated explicit declaration'}]})),
        Step(expected, Stream(Text('Delivery finished.'), Finish())),
    )


SCENARIOS['web_file_delivery'] = web_file_delivery
