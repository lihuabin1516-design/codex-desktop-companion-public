# Coordination Components

This page describes the implemented source components and their current limits. It is not a claim that all components are shipped, installed, or enabled by default.

```text
CodexPaneForge launcher
        |
        v
Coordinator <---- local IPC ----> MCP server
        |
        v
PaneForge HTTP MCP adapter
```

The Rust workspace contains separate crates for workflow state and protocol types, SQLite persistence, coordination runtime, host integration, and the stdio MCP boundary. The adapter treats the host as an external installation; it does not bundle or patch the host application.

## Current status

- Source includes core, protocol, store, coordinator, adapter, and MCP crates, plus synthetic tests and packaging scripts.
- The README records the implemented launcher and coordination-plane test coverage. Check the current test run before making a release claim.
- The default installation remains conservative: MCP registration and background coordinator startup are not enabled implicitly.
- External Windows-machine acceptance is pending. The contract is documented in [`../EXTERNAL_WINDOWS_ACCEPTANCE.md`](../EXTERNAL_WINDOWS_ACCEPTANCE.md).
- Real worker dispatch, session control, and background supervision are not general release capabilities in this snapshot.

See [`ADR-0001`](ADR-0001-three-component-coordination-plane.md) for the architectural decision and [`../ARCHITECTURE.md`](../ARCHITECTURE.md) for repository structure.
