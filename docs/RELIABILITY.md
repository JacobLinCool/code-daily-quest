# Reliability

This document records the operational expectations and failure-handling rules that should guide implementation changes.

## Operating Model

- The daemon is the steady-state writer to SQLite.
- The TUI is a read-mostly surface that should avoid hidden expensive work.
- The source-of-truth inputs are local Codex and Claude Code logs.
- Normal operation should be event-driven, not polling-heavy.

## Invariants

### Live tracking must stay cheap

- Source discovery happens on explicit rescan or daemon startup, not on every UI refresh.
- The live sync path should process changed files, not full source trees.
- Diagnostics should read cached adapter status by default.

### Quest state must be reproducible

- Daily quests are derived from `profile_id + local date`.
- Stable event IDs must survive session file moves and archival.
- Distinct metrics use first-claim ownership so per-tool totals add up cleanly.

### Historical rebuilds must stay explicit

- `retro --days N` is the only path that intentionally rebuilds history.
- Daemon startup should import today's activity but must not silently retro-import older history.
- `clear` must wipe only local tracker state, never source logs.

### The daemon must degrade gracefully

- Individual parse failures should surface in diagnostics and tracked source metadata.
- Notification failures are best-effort and must not crash the daemon.
- Unsupported platform integrations must say `unsupported` instead of attempting weak fallbacks.

## Common Debugging Commands

- `code-daily-quest doctor`: refresh discovery state and print adapter health.
- `code-daily-quest notify test --kind quest`: verify the local notification path.
- `code-daily-quest retro --days 30`: rebuild a bounded history window when the local DB drifts.
- `code-daily-quest clear`: reset the local DB completely.
- `code-daily-quest service status`: check launchd install/load state on macOS.

## Failure Modes To Watch

- Log format drift in Codex or Claude Code causing parser undercounting.
- Corrupt checkpoint state causing a source to stop ingesting until the state is reset or rebuilt.
- UI regressions that reintroduce redraw storms or hidden source rescans.
- Updater or install-script regressions that replace the binary incorrectly or skip checksum enforcement.

## Validation Expectations

Before claiming a reliability-sensitive change is complete, prefer this minimum bar:

- `cargo test -q`
- `cargo clippy -q --all-targets --all-features -- -D warnings`
- a targeted manual smoke pass when changing TUI behavior, launchd, updater, or notifications
