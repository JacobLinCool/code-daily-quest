# Security

`code-daily-quest` is a local-first developer tool, but it still crosses important trust boundaries. Treat security-sensitive changes accordingly.

## Trust Boundaries

### Local log inputs

Codex and Claude Code logs are parsed as untrusted structured input:

- they may contain arbitrary prompt text, code, paths, or malformed JSONL lines
- parser logic must never shell out based on log contents
- file paths extracted from logs should be normalized before use

### Local state and service installation

The app writes to:

- the app data directory resolved by `directories::ProjectDirs`
- the SQLite database and WAL files
- the macOS launchd plist path under `~/Library/LaunchAgents`

Changes that expand write scope or modify service-install behavior deserve extra scrutiny.

### Network boundary

Normal tracking is local-only. Network access is limited to:

- the updater in `crates/app/src/update.rs`
- the one-line install script that downloads release artifacts

These paths must keep checksum verification intact and should stay scoped to GitHub Releases unless requirements change explicitly.

## Sensitive Data Rules

- Local logs may contain prompts, code snippets, file paths, and other user work product.
- The SQLite database should be treated as having similar sensitivity because it stores normalized derivatives of those logs.
- Do not add debug logging that prints raw prompt or code content unless the user explicitly asked for it and understands the exposure.
- Sanitized fixtures should be used for tests and docs; never commit real private session logs.

## Secret Handling

- `GITHUB_TOKEN` is optional and only used to authenticate updater API requests.
- Tokens must not be written into the SQLite database, launchd plist, or persistent logs.
- Error messages should avoid echoing credential values.

## Review Triggers

Treat these changes as security-sensitive and review them with extra care:

- updater download, checksum, extraction, or binary replacement logic
- install script behavior
- launchd installer behavior
- parser changes that affect path normalization or file discovery
- any new network dependency or telemetry

## Safe Defaults

- Prefer explicit `unsupported` over silent fallback behavior.
- Prefer clearing or rebuilding local state over carrying compatibility code that could silently corrupt quest accounting.
- Keep the tracking daemon local-only unless the project adds a deliberate remote surface in the future.
