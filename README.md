# WinDevCleaner

Finds where the disk space of a Windows developer machine went, so that an AI agent can
investigate and propose, and a human approves before anything is touched.

The tool is being rebuilt in Rust, one vertical slice at a time, following
[the rebuild plan](docs/rebuild-plan.md). **This version has slices 1 to 4: "Where is my space?",
"What grew since last time?", "Why is this here, and what would deleting it free?" and "Clean the
VS Code leftovers safely".** Scanning, explaining and planning never change anything outside the
tool's own data folder. Only `apply`, `restore` and `purge` do, and each needs proof that a person
is present.

## What it does now

- `wdc scan` measures the read scope (your user profile without the personal folders, plus
  `ProgramData`) or the folders you name, gently: Windows background mode (low CPU, I/O and
  memory priority), one thread, optional `--throttle`.
- Sizes are **sizes on disk**: a file with several names (hard links) is counted once, also
  when its other name is outside the scanned folders; junctions and symbolic links are recorded
  and never followed; sparse and compressed files count what is allocated; cloud files that are
  not stored locally (OneDrive) count 0 and are never downloaded.
- Each scan saves a compressed snapshot in `%LOCALAPPDATA%\WinDevCleaner\snapshots` (the last
  10 are kept, and their total size is shown).
- `wdc diff` shows what grew since the previous scan of the same folders: new and grown folders
  first, each at the place that changed (not every parent of it), then removed and shrunk ones,
  and the large files that are new or grew.
- A scan of the default read scope also lists the system items Windows manages, read through
  Windows' own APIs and always report-only: Recycle Bin, paging, swap and hibernation files,
  error reports, crash and memory dumps, and System Restore (which only an administrator can
  read). Each one names the Windows setting or tool that manages it.
- `wdc explain PATH` says why a folder or file is here: the rule that knows it (class,
  evidence, source, cost, how to manage it), every condition and its result, whether it is
  inside the allowed roots or forbidden (checked on its real final path), its size on disk, every
  name of each hard-linked file, and what deleting it would free. A file with a name outside the
  item frees nothing. The rules live in `rules/*.toml`, are compiled into `wdc.exe`, and cover the
  VS Code family, NVIDIA update artifacts and shader caches, Steam, Claude desktop data
  (report-only) and Windows itself (never touched). Rules in
  `%LOCALAPPDATA%\WinDevCleaner\rules` can explain more, but are always report-only.
- Large folders and growth that no rule knows are flagged after a scan and in `wdc diff`.
- `WinDevCleaner.exe` shows the tree of sizes (with a "?" to explain any folder), a "What
  grew?" view, the system items, the latest explanation, the plan with its approval card, and
  the quarantine with restore and purge. When an AI agent connects through `wdc mcp`, the
  window shows "Agent connected: <name>", every call live in the activity panel, and "Pause
  agent" / "Disconnect agent" buttons.
- `wdc plan` drafts a plan: the items built-in rules may move now, each explained, with its
  size on disk and what deleting it would free. A plan is a list of choices, not an
  authorisation; it changes nothing. What `wdc apply` can clean in version 1, and nothing else:
  - obsolete extensions that VS Code, VS Code Insiders and Cursor listed for removal and no
    longer reference;
  - the folders of interrupted extension installs (GUID-named, older than a day) of the same
    three editors;
  - the caches of the same three editors (`Cache`, `CachedData`, `CachedExtensionVSIXs`,
    `Code Cache`, `GPUCache`);
  - the C/C++ extension's IntelliSense cache (`ipch`);
  - NVIDIA shader caches (`DXCache`, `GLCache`, `ComputeCache`): while a game or GPU app holds
    a file in one, that folder is left in place; close the program and run again;
  - Steam's web cache (`htmlcache`).

  Where a rule names a program (the editors, Steam), the item is moved only while that program
  is closed. NVIDIA app
  update artifacts (`ota-artifacts`) are never deleted by the tool in version 1, because that
  needs administrator rights: `wdc explain` on one gives the exact manual steps (close the NVIDIA
  app, a PowerShell command to run as administrator for that folder, then explain again), or says
  not to delete it when it is newer than the installed driver or the versions cannot be compared.
  `explain --json` and the MCP `explain` tool carry the steps as `manual_steps`.
- `wdc apply --plan FILE` trusts nothing in the plan: for each item it opens the item once without
  following links, checks on that open handle its real path (short names, `..` and trailing dots
  resolved by Windows) against the allowed and forbidden places and its file ID against the plan,
  runs the rules and their conditions again (VS Code closed, still listed as obsolete), and only
  then renames it into quarantine through the same handle. Anything that changed is skipped and
  reported. Plans older than 24 hours are refused. Free space is measured before and after.
- Quarantine is a rename on the same volume (`%LOCALAPPDATA%\WinDevCleaner\quarantine`, or
  `<drive>:\.wdc-quarantine` on other drives): instant, and `wdc restore ID` puts an item back
  (only if its place is free; if the app recreated the folder, both are kept and you choose).
  Quarantine frees nothing by itself: `wdc purge` deletes items after 7 days, and `--now` (or
  `apply --purge-now`) earlier only for regenerable caches and install leftovers. Every command
  shows what the quarantine holds.
- Proof of presence: Windows Hello when it is set up; otherwise a one-time code shown only in a
  desktop dialog, never in the console, which you type into the console. An interactive console
  alone is not proof. In the app, you approve on the approval card (Windows Hello, or the click
  itself where Hello is not set up).
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
wdc diff                         # what grew since the previous scan of the same folders
wdc diff --min-mb 100 --json
wdc explain "%USERPROFILE%\.vscode\extensions"
wdc plan                         # what may move to quarantine now, explained
wdc plan show PLAN.json
wdc apply --plan PLAN.json       # asks for Windows Hello or the desktop code
wdc quarantine
wdc restore ID
wdc purge                        # items older than 7 days; --now for caches and leftovers
wdc --lang pt-BR scan
```

### Connect an AI agent

Add `wdc mcp` as an MCP server (stdio) in your agent. For Claude Code:

```
claude mcp add windevcleaner -- wdc mcp
```

Codex and other MCP clients take the same command. Tools: `scan`, `tree` (drill into a
folder of the latest scan), `diff` (what grew), `explain`, `system_items`, `plan`, `quarantine`
and `request_apply`. The app's view follows the agent: it shows the scan, the folder, the
comparison, the system items or the plan the agent is reading. With the app open, every call runs
in the app while you watch; with it closed, `wdc mcp` runs headless (`wdc mcp --show` opens the
app first).

An agent can never apply, restore or purge. `request_apply` shows the plan on an approval card in
the app, with each item's evidence, and the agent gets back approved, rejected or timed out; with
the app closed it answers `needs-human` and nothing changes. To keep agents from even trying the
command line, [docs/agent-permissions.md](docs/agent-permissions.md) has a Claude Code snippet that
denies `wdc apply`, `restore` and `purge` (a convenience only: such rules match command text).

## Data folder

`%LOCALAPPDATA%\WinDevCleaner`: `snapshots\`, `activity.jsonl` (what agents did), `plans\`,
`quarantine\` (with `index.json`, what is held) and `changes.jsonl` (every apply, restore and
purge, with the free space measured). Outside it, only `apply`, `restore` and `purge` change
anything, and only the items you approved.

## Development

All building and testing runs in the cloud and on GitHub's `windows-latest` runner, never on
the owner's laptop. The workflow `.github/workflows/windows.yml` builds both executables and the
MSI, runs the Windows fixtures, the budget check of plan section 6.4 (`tools/wdc-bench`), and
the guard check (`ci/guard-check.sh`), which removes each guarding line of the product code and
requires the tests that depend on it to fail.

Workspace:

| Crate | What |
|---|---|
| `crates/wdc-core` | the engine: scanner, snapshots, rules, plans, apply, quarantine, presence proof, messages, activity log, app link, agent tools |
| `crates/wdc-cli` | `wdc.exe`: the commands and `mcp` |
| `crates/wdc-app` | `WinDevCleaner.exe` (egui) |
| `crates/wdc-testkit` | Windows fixtures and measurement helpers for tests (not shipped) |
| `tools/wdc-bench` | the budget check (not shipped) |
| `rules` | the rule base (TOML), compiled into the engine |
| `installer` | the WiX v5 MSI definition |

## Languages

The project (code, documentation, commits) is in English. The app and the command line speak
English and Brazilian Portuguese; the pt-BR texts live only in the message catalogue
(`crates/wdc-core/src/i18n.rs`).

## License

MIT, see [LICENSE](LICENSE).
