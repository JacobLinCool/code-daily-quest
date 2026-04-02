# AGENTS

This is the agent entrypoint for `code-daily-quest`. Use it as a map to the living sources of truth for architecture, UI behavior, operational constraints, and documentation debt. Do not treat this file as a standards manual.

Read in this order:

1. [README.md](README.md) for the user-facing product and CLI overview.
2. [ARCHITECTURE.md](ARCHITECTURE.md) for subsystem boundaries, dependency direction, and runtime flows.
3. [docs/FRONTEND.md](docs/FRONTEND.md) for the terminal UI surface and interaction constraints.
4. [docs/design-docs/index.md](docs/design-docs/index.md) for durable design rationale and core beliefs.
5. [docs/PLANS.md](docs/PLANS.md) for plan file lifecycle and where to record active or completed execution plans.
6. [docs/RELIABILITY.md](docs/RELIABILITY.md) and [docs/SECURITY.md](docs/SECURITY.md) before changing daemon behavior, updater behavior, local storage, notifications, or platform integration.
7. [docs/QUALITY_SCORE.md](docs/QUALITY_SCORE.md) and [docs/exec-plans/tech-debt-tracker.md](docs/exec-plans/tech-debt-tracker.md) for known gaps and cleanup priorities.
8. [docs/references/index.md](docs/references/index.md) for any future repo-local reference material.
