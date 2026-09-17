# External source demand

MCP definitions in `.agents/mcp.toml` and Python packages in `.agents/tools` are
inert. Discovery captures identities, definitions, provenance, revisions and bounded
diagnostics. It neither connects MCP nor prepares a Python environment.

The native lifecycle is:

```text
definition -> selection -> finite admitted demand -> credential/source resolution
           -> connection or preparation -> capability validation -> exact exposure
```

Root selects its own source Tools in `agent.tools.sources`. A server key addresses
MCP; `python:<package>` addresses Managed Python. Each value is `"all"`, an exact
Tool array, or `[]`. Defining a source or allowlisting a named Agent does not admit
that Agent's source demand. The child uses its captured complete profile when it is
actually admitted. Workflow demand follows its native admission semantics.

The existing source lifecycle owner performs materialization and settlement. Failed
required preparation rejects admission or the entire reload candidate. No alternate
connection manager exists. Tool visibility remains profile-owned; execution,
concurrency, approval and deadlines remain global invocation-policy facts.

Offline config inspection performs no external effects. Explicit diagnostic probes
also respect demand, and Python preparation requires `--prepare`. See the
[full configuration reference](configuration.md#resources-and-admission).
