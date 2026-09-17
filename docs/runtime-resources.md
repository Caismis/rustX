# Runtime resource generations

A loaded runtime publishes one immutable `RuntimeResourceSnapshot` through the
existing capability coordinator. Its `RuntimeConfiguration` carries the resolved
configuration, independent Provider/Model catalog, origins and source revisions.
The same snapshot owns Skills, named Agents, Workflows, MCP/Python source facts,
prepared external capabilities and Root exposure. There is no parallel resource-only
configuration generation.

See the [configuration reference](configuration.md#save-reload-and-restart) for the
candidate and publication contract. A candidate rereads both configuration sources
and both `.agents` roots, captures whole-resource shadow winners, resolves typed
policies and profiles, and prepares only admitted demand. Revision fences reject
source changes during capture. It is built off-side.

The runtime's private publication authority replaces the complete coordinator
snapshot under its state lock. Active Attempts, background work, children,
Workflows, maintenance or competing reload ownership can refuse publication as
busy. Failed candidates never replace any part of the current snapshot. Existing
admitted work holds its frozen snapshot; subsequent eligible work sees the new one.

Unused malformed resources produce bounded ordered diagnostics. A selected invalid
resource fails resolution or admission. Workspace invalid duplicates shadow User
resources without fallback. Discovery itself has no connection/preparation effects.
