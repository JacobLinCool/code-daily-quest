# Core Beliefs

This document captures the durable principles and decisions that repeatedly shape `code-daily-quest`.

## Stable Principles

### Local logs are the source of truth

The tracker should infer activity from Codex and Claude Code logs that already exist. It should not require wrappers, aliases, patched clients, or git diff inspection.

### Near-zero idle overhead matters

The daemon should feel invisible when the user is not actively coding. Normal operation should scale with changed files and appended bytes, not with total history size.

### Explicit counting beats fuzzy heuristics

Quest progress should come from events the system can justify from source logs. If the tool cannot prove a write, prompt, or token count, it should not count it.

### No silent fallback paths

Corrupt checkpoint state, discovery failures, or unsupported platform features should surface honestly. Silent fallback erodes trust and can poison quest accounting.

### Determinism is part of the product

Quest generation and historical reconstruction should be reproducible for a given `profile_id` and date window.

### Local tracker state is disposable

The SQLite database is derived state. If it drifts or the schema changes, rebuilding from logs is preferred over compatibility branches.

## Durable Design Decisions

### Workspace split: `core` and `app`

- `crates/core` owns parsing, normalized events, quest logic, storage, daemon behavior, and platform abstractions.
- `crates/app` owns CLI presentation, TUI presentation, and the updater UX.

This keeps the product logic testable without UI coupling and leaves room for a future non-TUI frontend.

### Common event model across tools

All adapters normalize into the same event types:

- conversation turns
- input tokens
- output tokens
- file edits

That keeps quest logic tool-agnostic while still preserving per-tool attribution.

### Stable event identity prefers session identity

Event IDs are keyed by tool id, logical session identity, line offset, and discriminator. Source path is only a fallback when a real session key does not exist.

### Startup and live sync are different products

- live startup imports today's activity only
- steady-state sync processes changed files only
- full historical rebuild is explicit via `retro --days N`

These are intentionally different because "resume live tracking" and "reconstruct history" have different performance and user-expectation profiles.

### The daemon is the intended steady-state writer

The daemon owns checkpoints, notifications, rollover timers, and incremental ingest. The TUI should stay read-focused and should not silently turn itself into a second tracker.

### SQLite stores both raw and derived state

The database is not just a cache of raw parsed lines. It also stores:

- checkpoints
- tracked sources and discovery snapshots
- distinct claims
- quest progress and completion metadata
- daily records and streak state

This keeps the UI cheap and diagnostics honest.

### Distinct metrics use first-claim ownership

`Active Projects` and `Edited Files` are counted once per day per unit. Ownership belongs to the first tool that claims the unit that day so visible totals and per-tool totals remain coherent.

### Quest completion uses last-hit attribution

Contribution breakdown and completion attribution answer different questions, so both are stored:

- per-tool contribution totals
- the tool that pushed the quest over the threshold

### Platform support is explicit

macOS is the primary platform in v1. Unsupported integrations should report `unsupported` rather than pretending to work.
