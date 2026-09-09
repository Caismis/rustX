---
description: Review one bounded request and return a concise result.
model: example/demo-model
tools:
  builtin: ["read"]
skills: ["review-guidance"]
agentsMd:
  inherit: false
  files: [".agents/subagents/reviewer/AGENTS.md"]
---
Review only the explicit proposal or assessment supplied to this child. Return the declared structured summary and acceptable boolean using workflow_output. Do not delegate or ask questions.
