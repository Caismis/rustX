// Generated from serialized Rust DTOs.
import type {ProtocolMessage} from './v25.js';
export const fixtures = [
  {
    "jsonrpc": "2.0",
    "id": "exact-session-summary",
    "method": "session/summary",
    "params": {
      "session_id": "ses_00000000-0000-7000-8000-000000000001"
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-session-summary",
    "result": {
      "type": "session_summary",
      "summary": {
        "cwd": "/workspace",
        "id": "ses_00000000-0000-7000-8000-000000000001",
        "name": null,
        "preview": "Native first user message",
        "updated_at": "1970-01-01T00:00:00Z",
        "active_node": "node_00000000-0000-7000-8000-000000000001"
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 291,
    "result": {
      "type": "diagnostics",
      "snapshot": {
        "lifecycle": "Accepting",
        "policy": {
          "max_resident_runtimes": 8,
          "max_connections": 32,
          "max_external_attachments": 64,
          "idle_grace_ms": 300000,
          "shutdown_deadline_ms": 30000
        },
        "loaded": 0,
        "loading": 0,
        "unloading": 0,
        "active_roots": 0,
        "external_attachments": 0,
        "sessions": [],
        "admission_refusals": {},
        "shutdown_failures": "0",
        "shutdown_timeouts": "0",
        "unload_failures": "0",
        "transport": {
          "websocket_connections": 0,
          "stdio_connections": 0,
          "connection_refusals": "0",
          "delivery_failures": "0",
          "max_message_bytes": 1048576,
          "outbound_queue_messages": 32,
          "outbound_queue_bytes": 33554432,
          "in_flight_requests": 16,
          "write_deadline_ms": "10000"
        }
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 291,
    "method": "server/diagnostics",
    "params": {}
  },
  {
    "jsonrpc": "2.0",
    "id": "initialize-fixture",
    "method": "initialize",
    "params": {
      "protocol_version": 25,
      "client": {
        "name": "fixture-client",
        "version": "1"
      },
      "presentation": {
        "images": false,
        "questionnaires": false,
        "reviews": false
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 7,
    "result": {
      "type": "initialized",
      "protocol_version": 25,
      "capabilities": {
        "multi_session": true,
        "single_writable_controller": true,
        "headless_interactions": true,
        "experimental_methods": []
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": null,
    "error": {
      "code": -32700,
      "message": "Parse error"
    }
  },
  {
    "jsonrpc": "2.0",
    "id": -3,
    "method": "interaction/respond",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      },
      "interaction": {
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "interaction_id": "interaction-fixture"
      },
      "response": {
        "type": "questionnaire",
        "response": {
          "type": "submitted",
          "value": {
            "answers": [
              {
                "question_index": 0,
                "answer": {
                  "type": "integer",
                  "value": {
                    "value": "9007199254740993"
                  }
                }
              },
              {
                "question_index": 1,
                "answer": {
                  "type": "number",
                  "value": {
                    "value": "3ff4000000000000"
                  }
                }
              }
            ]
          }
        }
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "method": "session/event",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      },
      "cursor": "9007199254740993",
      "event": {
        "type": "attempt_started",
        "attempt_id": "attempt-fixture",
        "model": null,
        "execution_settings": null
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "method": "session/closed",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "read",
    "result": {
      "type": "session",
      "session": {
        "id": "ses_00000000-0000-7000-8000-000000000001",
        "name": null,
        "created_at": "1970-01-01T00:00:00Z",
        "updated_at": "1970-01-01T00:00:01Z",
        "active_node": "node_00000000-0000-7000-8000-000000000001",
        "active_conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "node_count": 1
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "method": "artifact/read",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      },
      "artifact_id": "artifact_1"
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "method": "session/upload",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      },
      "files": [
        {
          "name": "hello.txt",
          "data": "aGk="
        }
      ]
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "method": "session/subscribe",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      },
      "after_cursor": "9007199254740993"
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "method": "session/trace",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      },
      "before": "trace:9007199254740993",
      "limit": 32
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "method": "session/transcript",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      },
      "before": "9007199254740993",
      "limit": 32
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "method": "session/fork",
    "params": {
      "session_id": "ses_00000000-0000-7000-8000-000000000001",
      "node_id": null,
      "surface_revision": "9007199254740993",
      "boundary": null,
      "side": "before"
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "method": "inbound/edit",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      },
      "expected": {
        "sequence": "9007199254740993",
        "message_id": "message-fixture",
        "revision": "9007199254740993"
      },
      "text": "edited pending input"
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "method": "inbound/remove",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      },
      "expected": {
        "sequence": "9007199254740993",
        "message_id": "message-fixture",
        "revision": "9007199254740993"
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "method": "agent/transcript",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      },
      "agent_id": "agent-fixture",
      "before": "9007199254740993",
      "limit": 32
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "method": "goal/control",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      },
      "control": {
        "action": "mutate",
        "expected": {
          "id": "goal-fixture",
          "revision": "9007199254740993"
        },
        "mutation": {
          "action": "pause"
        }
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "result": {
      "type": "inbound_mutation",
      "outcome": {
        "status": "conflict"
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "result": {
      "type": "configuration_application",
      "application": {
        "scope": "session-fixture",
        "sources": [
          {
            "kind": "user"
          },
          {
            "kind": "workspace",
            "directory": "/workspace/fixture"
          }
        ],
        "version": "9007199254740993",
        "desired": {
          "input_revision": "input",
          "attempt": "9007199254740993"
        },
        "units": {},
        "candidate": null,
        "eligibility": {
          "status": "unavailable"
        }
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "exact-u64",
    "result": {
      "type": "inbound_accepted",
      "message_id": "message-fixture",
      "inbound_sequence": "9007199254740993"
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "stale-settings",
    "error": {
      "code": -32000,
      "message": "Stale settings",
      "data": {
        "kind": "stale_settings",
        "expected": "9007199254740993",
        "actual": "9007199254740994"
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "method": "session/event",
    "params": {
      "target": {
        "session_id": "ses_00000000-0000-7000-8000-000000000001",
        "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
        "runtime_incarnation": "9007199254740993",
        "attachment_id": "attachment-fixture"
      },
      "cursor": "9007199254740993",
      "event": {
        "type": "workflows_updated",
        "workflows": {
          "revision": "9007199254740993",
          "runs": [
            {
              "id": {
                "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
                "attempt_id": "attempt-fixture",
                "invocation": "9007199254740993"
              },
              "workflow_id": "workflow-fixture",
              "program_digest": "digest-fixture",
              "resource_revision": "9007199254740993",
              "tool_call_id": "call-fixture",
              "state": {
                "type": "running"
              },
              "instances": [],
              "omitted_instances": 0,
              "steps_consumed": 1,
              "steps_max": 10,
              "agents_consumed": 0,
              "candidate": {
                "run": {
                  "conversation_id": "conv_00000000-0000-7000-8000-000000000001",
                  "attempt_id": "attempt-fixture",
                  "invocation": "9007199254740993"
                },
                "version": "9007199254740993",
                "content": "content-fixture"
              },
              "candidate_users": 0,
              "handoff": null
            }
          ],
          "omitted_runs": 0
        }
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 291,
    "error": {
      "code": -32000,
      "message": "Operation rejected",
      "data": {
        "kind": "unknown_agent",
        "agent_id": "agent-fixture"
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 291,
    "error": {
      "code": -32000,
      "message": "Operation rejected",
      "data": {
        "kind": "agent_history_unavailable",
        "agent_id": "agent-fixture"
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 291,
    "error": {
      "code": -32000,
      "message": "Operation rejected",
      "data": {
        "kind": "residency_capacity"
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 291,
    "error": {
      "code": -32000,
      "message": "Operation rejected",
      "data": {
        "kind": "attachment_capacity"
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 291,
    "error": {
      "code": -32000,
      "message": "Operation rejected",
      "data": {
        "kind": "request_capacity"
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 291,
    "error": {
      "code": -32000,
      "message": "Operation rejected",
      "data": {
        "kind": "server_draining"
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "archive-fixture",
    "method": "session/exportPrepare",
    "params": {
      "session_id": "ses_00000000-0000-7000-8000-000000000001"
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "archive-fixture",
    "result": {
      "type": "session_archive",
      "download": {
        "path": "/session-archive/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "filename": "rustx-session-fixture.zip",
        "expires_in_seconds": 60,
        "loopback_port": null
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "archive-failure-fixture",
    "error": {
      "code": -32000,
      "message": "Cannot export complete Session: a required descendant is missing or unreadable",
      "data": {
        "kind": "archive_preparation_failed",
        "reason": "descendant_unavailable"
      }
    }
  },
  {
    "jsonrpc": "2.0",
    "id": "archive-failure-fixture",
    "error": {
      "code": -32000,
      "message": "Cannot export complete Session: required artifact content is missing, unreadable, changed, or still being written; wait for active tools to finish and retry",
      "data": {
        "kind": "archive_preparation_failed",
        "reason": "artifact_unavailable"
      }
    }
  }
] satisfies ProtocolMessage[];
