# WinDevCleaner

## What it is

A Windows developer machine fills up with things nobody remembers putting there: editor extension
leftovers, GPU shader caches, a VM bundle from a desktop app, updater downloads. WinDevCleaner finds
where the space went, says why each folder is there and what deleting it would really free, and
cleans the known, safe cases. It has three faces on one engine: `wdc.exe` (the command line),
`WinDevCleaner.exe` (the desktop app) and `wdc mcp` (an MCP server an AI agent drives while you
watch every call in the app). Scanning, explaining and planning only read; nothing else on the disk
changes unless you approve it.

## Install

Download `WinDevCleaner-<version>-x64.msi` from the
[latest release](https://github.com/paulo-raoni/WinDevCleaner/releases/latest) (pre-releases are on
the [releases page](https://github.com/paulo-raoni/WinDevCleaner/releases)) and run it. To run
without installing, download `WinDevCleaner-<version>-x64.zip` instead; it holds `wdc.exe` and
`WinDevCleaner.exe`. The MSI installs per user, without admin rights, to
`%LOCALAPPDATA%\Programs\WinDevCleaner`, adds that folder to your `PATH` (open a new terminal to use
`wdc`) and adds a Start menu shortcut. Uninstall it from Settings > Apps.

To check a download, compare its hash with the matching line of `SHA256SUMS.txt` from the same release:

```
Get-FileHash .\WinDevCleaner-<version>-x64.msi -Algorithm SHA256
```

The MSI is not code-signed, so SmartScreen says "unknown publisher": choose "More info", then
"Run anyway". What changed in each version is in [CHANGELOG.md](CHANGELOG.md).

## Use

Every read command takes `--json` for stable English JSON. `--lang en|pt-BR` picks the language
(otherwise the Windows display language). `wdc --help` lists everything.

### Where is my space: `scan`

```
wdc scan                         # the read scope: your profile without the personal folders, plus ProgramData
wdc scan C:\Users\me\.vscode     # specific folders
wdc scan --depth 3 --top 20 --throttle 5
```

A scan runs in Windows background mode (low CPU, I/O and memory priority), on one thread unless
you ask for more, and prints the largest folders and files. Sizes are **sizes on disk**. A file
with several names (hard links) is counted once, also when its other name is outside the scanned
folders. Junctions and symbolic links are recorded and never followed. Sparse and compressed files
count what is allocated. Cloud files not stored locally (OneDrive) count 0 and are never downloaded. Folders of
1 GiB or more that no rule knows are flagged. A scan of the default scope also lists the **system
items** Windows manages, read through Windows' own APIs and always report-only: Recycle Bin, paging,
swap and hibernation files, error reports, crash and memory dumps, live kernel reports and System
Restore (only an administrator can read the last one). Each names the Windows setting or tool that
manages it.

`wdc scan --jobs N` (1 to 4, default 1) walks with N threads, each still in background mode, with
the same report; use it when you are waiting for the result. With more than one job the scan holds
each folder's record until it merges them, so it uses more memory than with one job. In the app,
the "Scan faster" switch does the same with 4 threads; it is off at every start and not saved.

Each scan saves a compressed snapshot in `%LOCALAPPDATA%\WinDevCleaner\snapshots` (the last 10 are
kept, and their total size is shown). The app shows the same data as a tree of sizes, with a "?" to
explain any folder; an agent drills into the latest scan with the MCP tool `tree`, and reads the
system items with `system_items`.

### What grew: `diff`

```
wdc diff                         # since the previous scan of the same folders
wdc diff --min-mb 100 --limit 50 --json
wdc diff --from OLDER.json.zst --to NEWER.json.zst   # two files from the snapshots folder (scan-*.json.zst)
```

New and grown folders first, each at the place that changed (not every parent of it), then removed
and shrunk ones, and the large files that are new or grew. It needs two scans of the same folders.

### Why is this here: `explain`

```
wdc explain "%USERPROFILE%\.vscode\extensions"
```

Says why a folder or file is here: the rule that knows it (class, evidence, source, cost, how to
manage it), every condition and whether it passed, whether the item is inside the allowed roots or
forbidden (checked on its real final path), its size on disk, every name of each hard-linked file,
and what deleting it would free. A file with a name outside the item frees nothing, and `explain`
says so.

### Clean: `plan`, `apply`, `quarantine`, `restore`, `purge`

```
wdc plan -o plan.json            # the items the built-in rules may move now, each explained
wdc plan --rule vscode-caches    # only one rule; --class takes regenerable-cache, install-leftover, app-managed or never-touch
wdc plan show plan.json
wdc apply --plan plan.json       # proof of presence, then moves the items to quarantine
wdc quarantine                   # what is held, and since when
wdc restore ID                   # put an item back
wdc purge                        # delete items held for more than 7 days
wdc purge --now                  # earlier, only caches and install leftovers
```

- `plan` writes a list of choices with each item's size on disk, what deleting it would free, the
  rule and its evidence. It changes nothing. A plan older than 24 hours is refused.
- `apply`, `restore` and `purge` need **proof that a person is present**: Windows Hello when it is
  set up; otherwise a one-time code shown only in a desktop dialog, never in the console, which you
  type into the console. An interactive console alone is not proof. In the app you approve on the
  approval card (Windows Hello, or the click itself where Hello is not set up).
- `apply` does not delete. It moves each item into **quarantine**, a rename on the same volume
  (`%LOCALAPPDATA%\WinDevCleaner\quarantine`, or `<drive>:\.wdc-quarantine` on other drives): instant
  and reversible. Quarantine frees nothing by itself; every command shows what it holds.
- `restore ID` puts an item back only if its place is free. If the app recreated the folder, both
  are kept and you choose.
- `purge` deletes quarantined items older than 7 days. `purge --now` (or `apply --purge-now`)
  deletes earlier, and only items that regenerate on their own (caches) or are install leftovers.
  Everything else waits the 7 days.
- Free space on each affected volume is measured before and after `apply` and `purge`, and shown
  against what was expected.

### What is installed: `programs`

```
wdc programs
wdc programs --json
```

Lists the installed programs found in the uninstall registry keys (per user, and per machine in the
64-bit and 32-bit views) with name, version, publisher, install date and the program's own uninstall
command, for you to run yourself.

- The size is the figure the installer reports (`EstimatedSize`), not a measured size: some
  installers report nothing. Programs are ordered by that figure, largest first.
- The tool never uninstalls. It runs no command, writes nothing to the registry and moves no file;
  the registry is opened for reading only. Updates, hotfixes, system components and Store (MSIX)
  apps are not listed.
- Leftovers: each run saves the list in `programs.json` in the data folder. On the next run, for
  each program that is gone, it reports a folder that is a direct child of `%APPDATA%`,
  `%LOCALAPPDATA%` or `%LOCALAPPDATA%\Programs` and is named exactly like the program or its
  publisher (ignoring case, nothing else: `Foo` never matches `Foo Bar`, `FooBar` or `Foo 2`). A
  publisher folder is reported only when no installed program has that publisher. These are
  `install-leftover` candidates: report-only, never in a plan, and a person must confirm each one
  (`wdc explain FOLDER` says what it is).
- No winget id is shown: a guessed id would break the rule that nothing matches by substring.

## What it can clean, and what it never touches

**Cleaned by `apply` (17 rules).** Nothing else is cleaned. Where a rule names a program (the
editors, Steam, Edge, pip, uv), the item is moved only while that program is closed.

- Obsolete extensions that VS Code, VS Code Insiders and Cursor listed for removal and no longer
  reference (3 rules).
- The folders of interrupted extension installs (GUID-named, older than a day) of the same three
  editors (3 rules).
- The caches of the same three editors: `Cache`, `CachedData`, `CachedExtensionVSIXs`,
  `Code Cache`, `GPUCache` (3 rules).
- The C/C++ extension's IntelliSense cache (`ipch`).
- NVIDIA shader caches (`DXCache`, `GLCache`, `ComputeCache`). While a game or GPU app holds a file
  in one, that folder is left in place: close the program and run again.
- Steam's web cache (`htmlcache`).
- Microsoft Edge's page, script and GPU caches (`Cache`, `Code Cache`, `GPUCache`) in each profile.
  The rest of a profile (history, passwords, cookies) is never touched.
- pip's download and wheel cache, including the one of a Microsoft Store Python.
- uv's package cache.
- The browsers Playwright downloaded (one item per build, plus Playwright's own bookkeeping, which
  it re-creates; `npx playwright install` downloads the browsers again).
- Temporary files and folders directly under `%TEMP%` whose own date is more than 7 days old (a
  folder whose contents changed recently can still qualify). A file in use keeps its item in
  place.

The old tool's crash dumps (`CrashDumps`, Camtasia's `CrashDumps`) are not carried over: they are
diagnostic data, not caches that regenerate, so they stay unknown or report-only.

**NVIDIA app update artifacts** (`ota-artifacts`, under `ProgramData`) are never deleted by the
tool, because that needs administrator rights. `wdc explain` on one gives the exact manual steps
(close the NVIDIA app, a PowerShell command to run as administrator for that folder, then explain
again), or says not to delete it when it is newer than the installed driver or the versions cannot
be compared. `explain --json` and the MCP `explain` tool carry the steps as `manual_steps`.

**Report-only** (explained, never in a plan): installed VS Code, Insiders and Cursor extensions
(manage them in the editor), the editors' workspace storage, Claude desktop data and the Cowork VM
bundle, the system items above, and program leftovers. Rules you add in
`%LOCALAPPDATA%\WinDevCleaner\rules` can explain more, but are always report-only.

**Never touched** (rules for Windows itself): the component store (WinSxS), `Windows.old`, the
installer package cache and `%WINDIR%`.

**Forbidden places, whatever a rule says**, checked on the real final path of an open handle:

- `%WINDIR%` and everything under it, `Program Files` and `Program Files (x86)` (programs are
  removed only by their own uninstaller), and the volume root;
- `%USERPROFILE%` itself and its personal folders (Documents, Desktop, Pictures, Videos, Music,
  Downloads) and OneDrive;
- credentials and keys: `Microsoft\Protect`, `Microsoft\Credentials`, `Microsoft\Vault`, `.ssh`,
  `.gnupg`;
- virtual disks (`*.vhd`, `*.vhdx`), Outlook data (`*.pst`, `*.ost`) and messenger data folders;
- browser profile data other than named cache subfolders;
- any folder that contains a `.git` directory;
- the quarantine itself.

Changes are allowed only inside `%LOCALAPPDATA%`, `%APPDATA%`, `%TEMP%`, `%USERPROFILE%\.vscode`
(also `.vscode-insiders` and `.cursor`), `%USERPROFILE%\.cache`, `%USERPROFILE%\.nuget\packages` and
`%USERPROFILE%\.gradle\caches`. An allowed root is never an item itself, only things inside it.
`ProgramData` is read but not changed.

## Safety model

- A plan is a list of choices, not an authorisation. `apply` trusts nothing in it.
- For each item, `apply` opens it once without following links, checks on that open handle its real
  path (short names, `..` and trailing dots resolved by Windows) against the allowed and forbidden
  places, and its file ID against the plan. It runs the rules and their conditions again (the editor
  is closed, the extension is still listed as obsolete) and only then renames the item into
  quarantine through the same handle. Anything that changed is skipped and reported.
- Nothing is deleted at once: quarantine first, `restore` any time, `purge` after 7 days.
- A file in use or access denied is reported and skipped. There are no forced deletes and no
  commands built from paths.
- Free space is measured before and after, and the tool reports expected against actual.
- Hard-linked files are counted once, and a name outside the item means deleting it frees nothing.
- An agent can never apply, restore or purge.

## Connect an AI agent

Add `wdc mcp` as an MCP server (stdio) in your agent. For Claude Code:

```
claude mcp add windevcleaner -- wdc mcp
```

Codex and other MCP clients take the same command. Tools: `scan`, `tree` (drill into a folder of
the latest scan), `diff` (what grew), `system_items`, `programs`, `explain`, `plan`, `quarantine`
and `request_apply`. The app's view follows the agent: it shows the scan, the folder, the
comparison, the system items, the programs or the plan the agent is reading. With the app open,
every call runs in the app while you watch, with "Agent connected: <name>", every call live in the
activity panel and "Pause agent" / "Disconnect agent" buttons. With it closed, `wdc mcp` runs
headless (`wdc mcp --show` opens the app first). The `scan` tool takes an optional `jobs` (1 to 4,
default 1), the same as `wdc scan --jobs`, for when the person is waiting for the result.

An agent can never apply, restore or purge. `request_apply` shows the plan on an approval card in
the app, with each item's evidence, and the agent gets back approved, rejected or timed out; with
the app closed it answers `needs-human` and nothing changes. To keep agents from even trying the
command line, [docs/agent-permissions.md](docs/agent-permissions.md) has a Claude Code snippet that
denies `wdc apply`, `restore` and `purge` (a convenience only: such rules match command text).

## What happened to the old tool

The first WinDevCleaner was a Python menu program with two entries, "Limpeza padrão (Cleaner)" and
"Obliterate Program", plus a separate script, "Insanity mode" (`programs_parser.py`). It was
removed at the end of the rebuild and is still in the git history; the last commit that has it is
`5ff166f` (for example `git show 5ff166f:legacy/cleaner.py.txt`). The audit that led to
the rewrite is section 2 of [the rebuild plan](docs/rebuild-plan.md).

| Old mode | What replaced it |
|---|---|
| Cleaner, **SAFE** (the standard clean) | The rules with `wdc plan` / `wdc apply`: evidence per item, quarantine, restore, free space measured. The old list was fixed paths of one machine from 2025 (Edge, pip, uv and Playwright caches, crash dumps); the rules in this version cover the editors, NVIDIA shader caches, Steam, Edge, pip, uv, Playwright and old temporary files, and crash dumps are not cleaned (a report-only system item). |
| Insanity mode (program folders ranked by size) | `wdc programs`, read-only. |
| Obliterate, the uninstall step | `wdc programs` shows each program's own uninstall command for you to run. |

**Dropped, and why:**

- **Cleaner, DEEP.** It removed whole installed programs (the VS Code install folder, Discord,
  Opera) and the `Temp` folder itself, not its contents. In a real run it removed the entire Discord
  install, reported 10.5 GB freed while about 1.4 GB was removed, and ran a forced PowerShell delete
  on the VS Code install folder after "access denied".
- **Cleaner, NUCLEAR.** It added all of `AppData\Local\Programs` (every per-user app) and the Edge
  `User Data` folder (profiles, history, saved passwords, extensions). Deleting programs and user
  data is not a cleaning operation, and programs are removed only by their own uninstaller.
- **Obliterate's folder deletion.** It joined the typed text into paths such as
  `C:\Program Files\<text>` and deleted them with no dry run and no confirmation: `.` reaches
  `C:\Program Files` and `AppData\Local` themselves, `..\Windows` reaches `C:\Windows`, and a vendor
  name deletes a folder shared by many products. Its uninstall matched by substring through a
  shell (`wmic`, already absent on current Windows). Nothing in this tool matches by substring, and
  it deletes no program folder.

In the old tool, deletion also stopped halfway at the first locked file, counted a file with two
names twice, and never checked that space was freed. The new one does not delete anything that has
not passed the checks above.

## Data folder

`%LOCALAPPDATA%\WinDevCleaner`: `snapshots\`, `activity.jsonl` (what agents did), `plans\`,
`quarantine\` (with `index.json`, what is held), `programs.json` (the last program list) and
`changes.jsonl` (every apply, restore and purge, with the free space measured). Outside it, only
`apply`, `restore` and `purge` change anything, and only the items you approved.

## Development

All building and testing runs in the cloud and on GitHub's `windows-latest` runner, never on the
owner's laptop. The workflow `.github/workflows/windows.yml` builds both executables and the MSI,
runs the Windows fixtures, the budget check of plan section 6.4 (`tools/wdc-bench`), and the guard
check (`ci/guard-check.sh`), which removes each guarding line of the product code and requires the
tests that depend on it to fail. The plan is [docs/rebuild-plan.md](docs/rebuild-plan.md); the
working rules are in [AGENTS.md](AGENTS.md).

Workspace:

| Crate | What |
|---|---|
| `crates/wdc-core` | the engine: scanner, snapshots, rules, plans, apply, quarantine, presence proof, programs, messages, activity log, app link, agent tools |
| `crates/wdc-cli` | `wdc.exe`: the commands and `mcp` |
| `crates/wdc-app` | `WinDevCleaner.exe` (egui) |
| `crates/wdc-testkit` | Windows fixtures and measurement helpers for tests (not shipped) |
| `tools/wdc-bench` | the budget check (not shipped) |
| `rules` | the rule base (TOML), compiled into the engine |
| `installer` | the WiX v5 MSI definition |

## Languages

The project (code, documentation, commits) is in English. The app and the command line speak
English and Brazilian Portuguese (`--lang en|pt-BR`); the pt-BR texts live only in the message
catalogue (`crates/wdc-core/src/i18n.rs`) and in the rules.

## License

MIT, see [LICENSE](LICENSE).
