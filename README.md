# Code Daily Quest

**Turn your agentic coding sessions into a daily RPG quest log.**

Every day at midnight, 3 new quests appear — randomly drawn from your real coding activity with Codex and Claude Code. Complete all 3 to extend your streak. Miss a day, and the streak resets.

No behavior changes needed. No config. It reads your existing session logs and does the rest.

```
╭─ Quests ── ✓ ALL CLEAR ───────────────────────────────────────────╮
│                                                                   │
│ ✓  HARD   Input Tokens                                            │
│   77,528,927 / 65,536 tokens  ████████████████████ 100%           │
│   claude-code 33  ·  codex 77,528,894                             │
│                                                                   │
│ ✓  NORMAL  Conversation Turns                                     │
│   40 / 4 turns  ████████████████████ 100%                         │
│   claude-code 2  ·  codex 38                                      │
│                                                                   │
│ ✓  EASY   Output Tokens                                           │
│   497,459 / 1,024 tokens  ████████████████████ 100%               │
│   claude-code 13,278  ·  codex 484,181                            │
│                                                                   │
╰───────────────────────────────────────────────────────────────────╯
```

## Quick Start

```bash
# Install
curl -fsSL https://raw.githubusercontent.com/JacobLinCool/code-daily-quest/main/install.sh | bash

# Start background tracking + notifications
code-daily-quest service install

# Open the quest board
code-daily-quest tui
```

That's it. Quests are already being tracked from your Codex and Claude Code sessions.

## The Quest System

### 5 Quest Types

Each day picks **3 out of 5** — you never know which combination you'll get.

| Quest | What It Tracks | Unit |
|---|---|---|
| **Active Projects** | Distinct project directories you worked in | projects |
| **Conversation Turns** | Prompts you sent to AI tools | turns |
| **Input Tokens** | Tokens sent to the model | tokens |
| **Output Tokens** | Tokens received from the model | tokens |
| **Edited Files** | Distinct files written by AI tools | files |

### 3 Difficulty Tiers

Easy, Normal, and Hard quests have different thresholds. The TUI shows your current progress and how far you have to go to complete each quest.

### Streaks

Complete **all 3 quests** in a day to increment your streak by 1. Miss even one, and it resets to 0 the next day. The counter is displayed prominently in the TUI so it can haunt you.

### Last-Hit Attribution

When your progress crosses the threshold, the tool that delivered the final hit gets credit. The TUI shows per-tool contribution breakdowns so you can see whether Codex or Claude Code carried the day.

### Deterministic Randomness

Quests are seeded by your profile ID + date. Same person, same date, same quests — useful when debugging, but it still *feels* random day to day.

## Commands

| Command | What It Does |
|---|---|
| `tui` | Open the quest board (Today / History / Diagnostics) |
| `daemon` | Run the background tracker in the foreground |
| `doctor` | Check log discovery, event counts, adapter health |
| `retro` | Import historical logs into the database (default: last 90 days) |
| `clear` | Wipe tracker state (your original logs are untouched) |
| `notify test` | Trigger a local test notification |
| `service install` | Install as a background service (macOS launchd) |
| `service uninstall` | Remove the background service |
| `service status` | Check if the daemon is running |
| `update` | Check for a newer release |
| `update apply` | Download and install the latest release |

### Typical Flow

```bash
# First time — check that your logs are found
code-daily-quest doctor

# Import past sessions from the last 90 days
code-daily-quest retro

# Or choose a shorter / longer window explicitly
code-daily-quest retro --days 30

# Send a test notification
code-daily-quest notify test

# Or test a specific notification template
code-daily-quest notify test --kind reminder

# Install the daemon so it runs automatically
code-daily-quest service install

# Check your quests whenever you want
code-daily-quest tui
```

## How It Works Under the Hood

1. **Log Discovery** — scans `~/.codex/sessions`, `~/.codex/archived_sessions`, and `~/.claude/projects/**/*.jsonl`
2. **Incremental Parsing** — tracks byte offsets per file, only reads newly appended content
3. **Event Normalization** — extracts conversation turns, token counts, file edits, and project paths into a unified schema
4. **Quest Generation** — at midnight local time, picks 3 quest kinds and difficulties using a deterministic PRNG (BLAKE3 + ChaCha8)
5. **Live Progress** — the daemon watches source directories with filesystem watchers and debounces updates
6. **Notifications** — on macOS, sends native notifications for quest completion, full `3/3` all-clear, noon and 20:00 reminders if you are not done yet, and daily quest reset at local midnight

All state lives in a single SQLite database (WAL mode). The `clear` command wipes it; your original tool logs are never modified.

`retro` rebuilds local state from a bounded history window. By default it imports the last `90` days including today; use `--days N` to change that window.

`notify test` does not need the daemon. It sends a local sample notification immediately so you can verify macOS notification delivery and the current notification templates.

## Data Sources

### Codex

```
~/.codex/sessions
~/.codex/archived_sessions
```

Extracts: session metadata, conversation turns, token deltas, file patches.

### Claude Code

```
~/.claude/projects/**/*.jsonl
```

Extracts: user prompts, assistant responses, tool-use file edits, token usage.

Both parsers filter out subagents, sidechains, tool-result noise, and bootstrap entries to keep quest progress clean.

## Installation

### One-Line Install (GitHub Releases)

```bash
curl -fsSL https://raw.githubusercontent.com/JacobLinCool/code-daily-quest/main/install.sh | bash
```

Installs to `~/.local/bin/code-daily-quest` by default. Override with environment variables:

```bash
CODE_DAILY_QUEST_INSTALL_DIR="$HOME/bin" \
CODE_DAILY_QUEST_VERSION="v0.1.0" \
curl -fsSL https://raw.githubusercontent.com/JacobLinCool/code-daily-quest/main/install.sh | bash
```

### From Source

```bash
cargo install --path crates/app --locked
```

Or build manually:

```bash
cargo build --release -p code-daily-quest
install -m 0755 target/release/code-daily-quest ~/.local/bin/code-daily-quest
```

## Platform Support

| Feature | macOS | Linux |
|---|---|---|
| Core tracking + TUI | Yes | Yes |
| Background daemon | Yes | Yes |
| Native notifications | Yes | No |
| `service install` (launchd) | Yes | No |
| Self-updater | Yes | Yes |

macOS is the primary platform in v1. Linux runs the core, TUI, and updater, but notifications and service installation are intentionally marked as unsupported.

## Documentation

- [Architecture](ARCHITECTURE.md)
- [Agent entrypoint](AGENTS.md)
- [Design docs index](docs/design-docs/index.md)

## Release Targets

- `x86_64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

Each release includes `.tar.gz` and `.sha256` checksum files. The `update apply` command verifies checksums before replacing the binary.
