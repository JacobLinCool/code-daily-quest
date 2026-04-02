# Tech Debt Tracker

This file records known cleanup work that matters, even when it is not part of the current task.

## Active Debt

### 1. Build a sanitized real-log fixture corpus

- Problem: parser tests mostly use synthetic log lines.
- Why it matters: Codex and Claude log formats can drift in the wild, and synthetic fixtures miss edge cases.
- Desired future state: versioned, sanitized fixtures that reflect real session logs for both tools.

### 2. Add end-to-end macOS smoke coverage

- Problem: launchd install/uninstall, native notifications, and daemon lifecycle are only partially covered by local unit/integration tests.
- Why it matters: the most user-visible failures are platform integration failures.
- Desired future state: a repeatable smoke checklist or automated harness that verifies notifications, service install, and recovery behavior on macOS.

### 3. Add repeatable TUI regression coverage

- Problem: the TUI has already seen regressions around redraw frequency, TTY pollution, and layout clarity.
- Why it matters: the UI is the main way users inspect progress.
- Desired future state: terminal-size smoke tests or snapshot-style checks that catch rendering regressions early.

### 4. Decide whether product-spec docs are now warranted

- Problem: the project has grown enough user-facing behavior that quest balancing and notification policy may deserve explicit specs.
- Why it matters: future changes to quest generation, reminder behavior, or TUI semantics could drift without a durable spec surface.
- Desired future state: either a clear decision to keep behavior lightweight and code-led, or a `docs/product-specs/` area with real scope.
