# Plans

Use `docs/exec-plans/` for multi-step work that should survive across sessions.

## Layout

- `docs/exec-plans/active/`: plans that are still in progress.
- `docs/exec-plans/completed/`: plans that were executed and are now historical reference.
- `docs/exec-plans/tech-debt-tracker.md`: durable backlog for cleanup that is known but not actively staffed.

## Naming Convention

Prefer plan filenames in this form:

- `YYYY-MM-DD-short-topic.md`

That keeps plans sortable and easy to archive without needing extra metadata.

## Lifecycle

1. Create a file in `docs/exec-plans/active/` when work needs durable sequencing, checkpoints, or cross-session context.
2. Update the file as decisions change instead of creating parallel plan copies.
3. Move the file to `docs/exec-plans/completed/` once the work is done or explicitly retired.
4. If a gap remains but is not being executed now, add it to `docs/exec-plans/tech-debt-tracker.md`.

## What Belongs In A Plan

Good plan contents:

- current goal
- constraints or invariants that must hold
- ordered execution steps
- validation steps
- deferred follow-ups that should survive completion

Do not use plan files for general architecture reference. Stable truth belongs in `ARCHITECTURE.md` or the docs under `docs/`.
