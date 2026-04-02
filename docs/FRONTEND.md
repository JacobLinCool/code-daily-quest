# Frontend

This repository has a terminal UI, not a browser frontend. There is no Tauri surface yet; the only implemented UI is the Ratatui-based TUI in `crates/app/src/tui.rs`.

## UI Surface Map

### CLI Entry

The CLI in `crates/app/src/main.rs` is the outer shell for the product. Most user flows start with commands, then optionally enter the TUI.

### TUI Tabs

The TUI has three tabs:

- `Today`: current quests, service status, streak, reset time, and per-tool quest progress.
- `History`: recent days plus per-day quest details.
- `Diagnostics`: cached discovery and database status, with explicit manual rescan.

## State Boundaries

- The TUI reads snapshots from `Tracker`.
- It should not become a second long-running writer.
- Diagnostics refresh is explicit from the `Diagnostics` tab via `r`.
- Service status is a low-frequency refresh, not something to query on every draw.

## Interaction Rules

- `q` exits.
- `Tab` / right arrow advances tabs.
- `Shift-Tab` / left arrow moves backward.
- up/down selects the focused history day.
- `r` refreshes diagnostics only when Diagnostics is selected.

## Performance Rules

- Avoid hidden redraw loops that repaint on a fixed timer without state changes.
- Avoid rediscovering source files during normal view refresh.
- Avoid shell commands that write directly into the active TTY from inside the TUI.
- Keep idle CPU close to zero when nothing is changing.

## Validation Expectations

When changing the UI:

- manually test a normal-width terminal and a narrower terminal
- verify tab switching, history navigation, and diagnostics refresh
- confirm that launchd status checks do not corrupt the alternate screen
- watch for redraw storms or renderer CPU spikes

## Relationship To Future UI Work

If a Tauri surface is added later, document it separately instead of overloading this file with speculative rules. Until then, `docs/FRONTEND.md` is specifically about the terminal UI and CLI interaction model.
