# WinDevCleaner

Finds where the disk space of a Windows developer machine went, so that an AI agent can
investigate and propose, and a human approves before anything is touched.

The tool is being rebuilt in Rust, one vertical slice at a time, following
[the rebuild plan](docs/rebuild-plan.md). **This version is slice 1, "Where is my space?": it
is read-only.** It never changes anything on disk outside its own data folder.

## What it does now

- `wdc scan` measures the read scope (your user profile without the personal folders, plus
  `ProgramData`) or the folders you name, gently: Windows background mode (low CPU, I/O and
  memory priority), one thread, optional `--throttle`.
- Sizes are **sizes on disk**: a file with several names (hard links) is counted once, also
  when its other name is outside the scanned folders; junctions and symbolic links are recorded
  and never followed; sparse and compressed files count what is allocated; cloud files that are
  not stored locally (OneDrive) count 0 and are never downloaded.
- Each scan saves a compressed snapshot in `%LOCALAPPDATA%\WinDevCleaner\snapshots` (the last
  10 are kept), for the "what grew" view of slice 2.
- `WinDevCleaner.exe` shows the tree of sizes. When an AI agent connects through `wdc mcp`, the
  window shows "Agent connected: <name>", every call live in the activity panel, and "Pause
  agent" / "Disconnect agent" buttons.
- English and Brazilian Portuguese (`--lang en|pt-BR`, or the Windows display language).

The old Python tool is kept in `legacy/` as `.txt` files, for reference only, and will be
deleted at the end of the rebuild.

## Install

Download `WinDevCleaner.msi` from the latest successful
[windows workflow run](https://github.com/paulo-raoni/WinDevCleaner/actions/workflows/windows.yml)
(artifact `windevcleaner-slice-1`) and run it. It installs per user, without admin rights, to
`%LOCALAPPDATA%\Programs\WinDevCleaner`, adds that folder to your `PATH` and adds a Start menu
shortcut. Uninstall it from Settings > Apps.

## Use

```
wdc scan                         # the default read scope
wdc scan C:\Users\me\.vscode     # specific folders
wdc scan --json                  # stable English JSON, for scripts and agents
wdc --lang pt-BR scan
```

### Connect an AI agent

Add `wdc mcp` as an MCP server (stdio) in your agent. For Claude Code:

```
claude mcp add windevcleaner -- wdc mcp
```

Codex and other MCP clients take the same command. Tools: `scan` and `tree` (drill into a
folder of the latest scan). With the app open, every call runs in the app while you watch; with
it closed, `wdc mcp` runs headless (`wdc mcp --show` opens the app first).

## Data folder

`%LOCALAPPDATA%\WinDevCleaner`: `snapshots\` and `activity.jsonl` (what agents did). Nothing
else is written.

## Development

All building and testing runs in the cloud and on GitHub's `windows-latest` runner, never on
the owner's laptop. The workflow `.github/workflows/windows.yml` builds both executables and the
MSI, runs the Windows fixtures, the budget check of plan section 6.4 (`tools/wdc-bench`), and
the guard check (`ci/guard-check.sh`), which removes each guarding line of the product code and
requires the tests that depend on it to fail.

Workspace:

| Crate | What |
|---|---|
| `crates/wdc-core` | the engine: scanner, snapshots, messages, activity log, app link, agent tools |
| `crates/wdc-cli` | `wdc.exe`: `scan` and `mcp` |
| `crates/wdc-app` | `WinDevCleaner.exe` (egui) |
| `crates/wdc-testkit` | Windows fixtures and measurement helpers for tests (not shipped) |
| `tools/wdc-bench` | the budget check (not shipped) |
| `installer` | the WiX v5 MSI definition |

## Languages

The project (code, documentation, commits) is in English. The app and the command line speak
English and Brazilian Portuguese; the pt-BR texts live only in the message catalogue
(`crates/wdc-core/src/i18n.rs`).

## License

MIT, see [LICENSE](LICENSE).
