# Explicit source selection

The canonical `.agents/agents/analyst.toml` profile demonstrates the shared MCP
and Managed Python selection document. Supply an enabled/trusted MCP definition
named `github` and a canonical `.agents/tools/data-analysis` package, then admit
`analyst` through the existing Agent admission configuration. Discovery alone
prepares neither source. Source selection and materialization remain separate;
see [the full contract](../../../docs/tool-source-selection.md).
