# Architecture

`code-daily-quest` is a Rust workspace with a thin application crate layered on top of a reusable tracking core:

- `crates/app`: CLI entrypoints, TUI rendering, and updater UX.
- `crates/core`: log adapters, normalized event model, SQLite state, quest generation, daemon orchestration, notifications, and service integration.

## Domain Map

### 1. Application Surface

The binary in `crates/app` exposes these user-facing flows:

- `tui`: open the terminal UI and read cached quest state.
- `daemon`: run filesystem watching, incremental ingestion, rollover timers, and notifications.
- `doctor`: rescan source roots and print diagnostics.
- `retro --days N`: rebuild the local database from bounded historical logs.
- `clear`: wipe local tracker state only.
- `notify test`: trigger sample notifications without the daemon.
- `service install|uninstall|status`: manage the macOS launchd user agent.
- `update` / `update apply`: check GitHub Releases and self-update the binary.

### 2. Core Tracking Pipeline

The live tracking path is:

1. `Tracker::initialize_live_tracking()` discovers sources once, updates adapter status, and ingests today's activity for newly seen sources.
2. `daemon::run_daemon()` attaches recursive filesystem watchers to discovered roots.
3. Changed paths are debounced and passed to `Tracker::sync_changed_sources()`.
4. Adapters parse only the targeted source files into `NormalizedEvent` values.
5. `Store::ingest_events_incremental()` updates raw events, distinct claims, quest progress, daily records, and newly completed quests.
6. The daemon sends best-effort notifications for quest completion, all-clear, reminders, and daily reset.

The historical rebuild path is separate:

1. `retro --days N` clears derived state but preserves `profile_id`.
2. All discovered sources are parsed.
3. Events older than the requested `local_day` cutoff are discarded.
4. Raw events are reinserted and the derived state is rebuilt from the retained window.

## Subsystem Responsibilities

### Adapters

`crates/core/src/adapters/` owns tool-specific parsing:

- `codex.rs`: reads `~/.codex/sessions` and `~/.codex/archived_sessions`.
- `claude.rs`: reads `~/.claude/projects/**/*.jsonl` and ignores subagents and sidechains.
- `mod.rs`: defines the adapter trait, JSONL cursor, path normalization, checkpoint decoding, and stable event IDs.

Adapters are responsible only for translating source logs into normalized events plus a checkpoint. They do not update quests or notifications directly.

### Store

`crates/core/src/store.rs` owns the SQLite schema and derived state:

- raw normalized events
- source checkpoints
- tracked source registry and adapter discovery snapshots
- distinct ownership claims
- daily metric totals and per-tool progress
- quest assignments and completion metadata
- daily records and streak state
- lightweight app metadata such as `profile_id` and `last_sync_at`

The store is the single source of truth for local state, but not for activity truth. Activity truth still lives in Codex and Claude logs.

### Tracker

`crates/core/src/tracker.rs` is the orchestration boundary between adapters, store, and platform helpers. It exposes high-level operations that the app crate calls instead of wiring components manually.

### Platform Layer

`crates/core/src/platform.rs` is the OS boundary:

- native notifications on macOS
- launchd service install/uninstall/status on macOS
- explicit `unsupported` behavior on non-macOS targets

### TUI

The TUI in `crates/app/src/tui.rs` is read-mostly. It renders `Today`, `History`, and `Diagnostics` from tracker snapshots and only triggers an explicit rescan from the Diagnostics tab.

## Dependency Direction

The intended dependency flow is:

`app` -> `core::tracker` -> (`core::adapters`, `core::store`, `core::platform`)

Important rules:

- `crates/core` must not depend on `crates/app`.
- adapters must not reach into UI or platform code.
- the updater stays in `crates/app`; the normal tracking path should remain local-only.
- the daemon is the only intended writer during steady state; the TUI should remain read-focused.

## Cross-Cutting Invariants

- Local log files are the activity source of truth.
- Stable event IDs prefer session identity over source path, so archived or moved logs do not double-count.
- Diagnostics are cached by default; rescans must be explicit.
- Notifications are best-effort and must not crash the daemon.
- Schema mismatches rebuild local state instead of preserving compatibility branches.

For durable product and design rationale, see [docs/design-docs/index.md](docs/design-docs/index.md). For UI-specific behavior, see [docs/FRONTEND.md](docs/FRONTEND.md).
