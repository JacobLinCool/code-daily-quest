# Quality Score

This is the quality ledger for the current repository state. Grades are implementation-facing, not marketing claims.

## Current Grades

| Area | Grade | Evidence | Next Upgrade |
|---|---|---|---|
| Core tracking pipeline | A- | Log-driven adapters, stable event IDs, incremental ingest, and focused integration tests cover the main quest logic. | Add sanitized real-log fixture corpora for both Codex and Claude Code so parser behavior is checked against real-world drift. |
| Terminal UI and CLI ergonomics | B | The CLI surface is coherent and the TUI now uses dirty redraws, explicit diagnostics refresh, and clearer quest rendering. | Add repeatable TUI smoke coverage across narrow and wide terminal sizes. |
| Platform integration | B- | macOS notifications, launchd support, and test-notification flows exist behind explicit platform boundaries. | Add real macOS smoke validation for notifications, launchd install/uninstall, and daemon restart behavior. |
| Release and update path | B | GitHub Release packaging, checksum verification, install script, and self-updater are implemented. | Add an end-to-end release validation pass that exercises published artifacts, not just local packaging. |
| Documentation harness | B | The repo now has an agent-readable knowledge store with architecture, reliability, security, and debt tracking. | Keep docs synchronized with TUI, notification, and retro semantics as behavior changes. |

## Known Gaps

- No sanitized, versioned corpus of real Codex / Claude session logs exists in the repository yet.
- No automated UI snapshot or screenshot verification exists for the TUI.
- No product-spec or product-sense doc exists yet; if quest balancing or behavior becomes more complex, that surface should be documented explicitly.
