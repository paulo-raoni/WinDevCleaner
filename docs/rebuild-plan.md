# WinDevCleaner rebuild plan

Status: **draft, waiting for the owner's approval.** No code is written until this plan is approved
or changed. Date: 2026-11-07.

## 1. Why rebuild

On 2026-11-07 the C: drive (117 GB SSD) was down to 6 to 12 GB free. The space was found and freed
by hand, with an assistant's help:

| What | Size | Where it came from |
|---|---|---|
| Claude desktop Cowork VM bundle | 8.6 GB | arrived 2026-11-04; two paths that were hard links to one file |
| NVIDIA app update artifacts | 3.7 GB | `C:\ProgramData\NVIDIA Corporation\NVIDIA app\UpdateFramework\ota-artifacts` |
| VS Code extension leftovers and caches | about 6 GB | `.obsolete` entries and hidden GUID staging folders in `%USERPROFILE%\.vscode\extensions` |

The current WinDevCleaner (code last changed July 2025, last run 2025-10-27) could not have found any
of these: it only knows a fixed list of 2025 paths. It also measures sizes wrongly and deletes in ways that are
unsafe. The goal of the rebuild is a tool that an AI agent can use to **investigate and propose**,
and that a human **approves** before anything is touched.

## 2. Audit of the current code

All files were read: `main.py`, `cleaner.py`, `programs_parser.py`, `obliterate.py`, `utils.py`,
`config_paths.json`, `limpa_custom.ps1`, `README.md`, and the old run logs in `logs/` (not tracked).

### 2.0 What it actually did

`logs/limpeza_20251027-104151.txt` records a real Deep run on 2025-10-27. It removed the Edge
`Profile 3` cache (218 MB), `CrashDumps` (287 MB) and **the whole Discord install (894 MB)**, then
hit "access denied" on `Temp` and on the **VS Code install folder** and ran the forced PowerShell
delete on both. The log reports **10.5 GB** of space while about **1.4 GB** was actually removed
(the forced deletes report 0 B and their outcome is unknown).

### 2.1 Unsafe (can destroy data or the system)

1. **Obliterate deletes whole system folders on trivial input.** `obliterate.py` joins the typed
   text straight into paths such as `C:\Program Files\{prog}` and calls `shutil.rmtree`. Typing
   `.` resolves to `C:\Program Files`, `C:\ProgramData`, `AppData\Local` and `AppData\Roaming`
   themselves; typing `..\Windows` reaches `C:\Windows`. Ordinary input is just as bad: `Microsoft`
   or `NVIDIA Corporation` deletes the whole `ProgramData\<vendor>` tree, shared by many products.
   There is no dry run and no confirmation in this mode.
2. **Obliterate's uninstall matches by substring and runs through a shell.** It runs
   `wmic product where "Name like '%{prog}%'" call uninstall` with `shell=True`. Typing `a`
   uninstalls every MSI product whose name contains an "a". The typed text is placed inside the
   command string, so quotes or `&` in it inject commands. Querying `Win32_Product` also makes
   Windows run a consistency check (and possible repair) on every installed MSI package.
   `wmic` is deprecated, and on this laptop (build 26200) `WMIC.exe` is already absent (checked
   2026-11-07), so today the uninstall step silently fails and the folder deletion runs anyway.
   (Whether `wmic ... call uninstall` asked Y/N per product was not verified.)
   (The brief said Obliterate "matches folders by name substring". The code is slightly different:
   the *uninstall* matches by substring; the *folder* step joins raw input into paths, which is the
   worse problem described in item 1.)
3. **Deep and Nuclear modes delete installed programs and user data.** Deep removes
   `AppData\Local\Programs\Microsoft VS Code` (the VS Code install), `AppData\Local\Discord` and
   `Programs\Opera`. Nuclear removes all of `AppData\Local\Programs` (every per-user app) and
   `Microsoft\Edge\User Data` (profiles, history, saved passwords, extensions).
4. **`force_delete` builds a PowerShell command by string interpolation.**
   `subprocess.run(["powershell", "-Command", f"Remove-Item -Path '{path}' ..."], shell=True)`.
   A path containing `'` breaks out of the quoted string; `shell=True` adds a second layer
   (cmd.exe) of parsing on top. It is offered on "file in use" and "access denied" errors, which
   are exactly the cases where the tool should stop, not escalate.
5. **Deep removes the `Temp` folder itself**, not its contents (`shutil.rmtree` on
   `AppData\Local\Temp`). Applications expect that folder to exist.
6. **Caches are deleted while their owner may be running** (Edge cache, Discord, VS Code). Nothing
   checks for running processes.
7. **No reparse-point handling.** The size walker and the PowerShell `Remove-Item -Recurse` path
   do not check for junctions or symbolic links, so they can measure or act on a target outside
   the intended folder. (`shutil.rmtree` on Python 3.8+ removes a junction without entering it,
   but the PowerShell fallback and the measuring code have no such guard.)
8. **Partial deletes.** `shutil.rmtree` stops at the first locked file (`cleaner.py:104`), leaving a
   program half-deleted and unusable, with no record of what was removed.
9. **`programs_parser.py` ("Insanity mode") offers deletion of any program folder** under
   `AppData\Local\Programs` and Python `local-packages`, ranked only by size and last access time.
   (Access times are often not updated on NTFS, so "last used" is unreliable.) It is not wired into
   `main.py`, but runs on its own.

### 2.2 Wrong results

10. **Hard links are counted once per path.** `os.path.getsize` per path means a file with two
   names is counted twice, and deleting one name frees nothing. This is exactly what happened with
   the Cowork VM bundle (8.6 GB counted twice; deleting one path freed 0 bytes).
11. **Overlapping targets are counted twice.** Nuclear lists `ms-playwright` twice and contains both
    `Programs` and folders inside it (Opera, VS Code), and both `Edge\User Data` and its cache
    folders.
12. **Logical size, not size on disk.** `getsize` ignores NTFS compression, sparse files (common
    for VM disk images) and cloud placeholders (OneDrive files that are not stored locally).
13. **Nothing checks that space was really freed.** The report shows the sum of the sizes it
    intended to delete. Free space on the volume is never read before or after (see 2.0: 10.5 GB
    reported, about 1.4 GB removed).
14. **Totals disagree between reports.** The size is appended to `sizes` before the deletion is
    attempted, so failed deletions are included in the TXT/CSV/HTML totals but not in the console
    total.
15. **Errors are detected by matching message text** (`"WinError 32"`, `"em uso"`) instead of the
    numeric `winerror` attribute. The `[WinError N]` part happens to be language-independent, so it
    works today, but it is fragile.

### 2.3 Obsolete or dead

16. Every target path is hardcoded to `C:\Users\raoni\...` and to 2025 apps (Python 3.10 from the
    Store, Camtasia, TikTok Live Studio, Streamlabs, Opera).
17. `config_paths.json` is never read. `limpa_custom.ps1` is never called. `utils.py` is never
    imported. `programs_parser.py` is not reachable from `main.py`.
18. `delete_path` uses mutable default arguments (`logs=[]`, `sizes=[]`), shared between calls.
19. Logs and reports are written relative to the current directory; colour escape codes from
    `colorama` end up inside the TXT and CSV files; HTML reports do not escape paths.
20. No tests, no dependency file, no packaging. The README links images (`docs/imagens/...`) that
    do not exist and calls Obliterate "coming soon" although it is wired in.

**Conclusion:** nothing of the deletion logic is worth keeping. The rebuild starts from an empty
tree; the old files move to `legacy/` for reference in the first PR and are deleted as the last
step of the rebuild (section 10).

## 3. Design principles

1. **Read-only by default.** `scan`, `explain` and `plan` never modify anything. Only `apply`,
   `purge` and `restore` write, and only from a plan file.
2. **The agent proposes, the human approves.** An agent can scan, explain and write a plan. The
   approval step is built to stop accidents and casual misuse by an agent, not a determined attacker
   with the user's own rights (see section 7).
3. **Every byte has evidence.** Each candidate carries the rule that matched it, the class, the
   reason, and the checks that were run.
4. **Measure what is real.** Size on disk, each file counted once by its file ID, and free space
   read from the volume before and after every change.
5. **Gentle on the machine.** Low CPU and IO priority, bounded memory, scoped roots. No admin
   rights in version 1: anything that needs elevation is report-only, with the exact steps for the
   human.
7. **Read scope is wider than act scope.** The tool may look at more places than it may change, so
   it can find growth anywhere it can read cheaply, while acting only inside the allowed roots.
6. **Never escalate on failure.** A file in use or access denied is reported and skipped. No
   forced deletes, no "delete on reboot", no shell commands built from paths.

## 4. Components

### 4.1 Scanner (read-only)

- **Read scope** (walked, never changed unless also in the act scope of section 5.3): the allowed
  roots, all of `%ProgramData%` (where the NVIDIA artifacts and `Package Cache` live), and
  `%USERPROFILE%` excluding the user folders of section 5.3. Never the whole disk by default.
- **System items read through their own APIs, not by walking:** Recycle Bin size
  (`SHQueryRecycleBin`), `hiberfil.sys` and `pagefile.sys` sizes, Windows error reports and
  `LiveKernelReports`, memory dumps, and System Restore shadow storage where readable without
  admin. These are always report-only, with the Windows setting or tool that manages each.
- **Optional whole-C: overview** (`wdc scan --overview`), read-only and still gentle, only when the
  human asks for it. Not part of the default scan.
- **One count per file.** Each file is identified by `(volume serial, file ID)`. Files are listed
  per directory with `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)` on the directory
  handle, which returns file ID, allocation size, logical size and attributes for every entry in
  one call, without opening each file.
- **Hard links, done correctly.** Directory listings do not report link counts, and the size stored
  in a directory entry can be stale for a file with several names. So:
  - files of 1 MB or more are remembered by file ID; a second name of the same file counts 0 bytes
    (small files are not tracked, which bounds memory; their error is negligible);
  - files of 64 MB or more are checked individually: link count read by name
    (`GetFileInformationByName` where the build has it, otherwise a handle opened with no data
    access) and size re-read from the file itself. These are few, so the cost is small;
  - `explain` lists every name of a file (`FindFirstFileNameW`), including names outside the
    scanned roots. This answers "why did deleting this free 0 bytes" directly.
- **Size on disk.** Uses allocation size, not logical size, and reads compressed size for
  compressed files. Cloud placeholders (`FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`, `OFFLINE`) count 0
  and are never read, so the scan never triggers a OneDrive download.
- **Reparse points.** Only *links* (junctions, symbolic links, mount points) are recorded and never
  followed. Other reparse points (Windows compressed files, OneDrive files, deduplicated files, app
  execution aliases) are measured normally.
- **Gentle.** The process enters background mode (`PROCESS_MODE_BACKGROUND_BEGIN`, which lowers
  CPU, IO and memory priority), runs single-threaded by default, and can pause between directories
  (`--throttle`). Memory is bounded by streaming: only per-directory totals, the top-N largest
  files and the large-file ID set are kept. Section 6 decides whether an optional bounded parallel
  walk is worth offering.
- **Snapshots.** Each scan stores per-directory totals (to a configurable depth) and the top-N
  files as one compressed JSON file under `%LOCALAPPDATA%\WinDevCleaner\snapshots\`. Only the last
  N snapshots are kept, and their total size is reported. SQLite is not used in version 1; it is
  reconsidered only if history queries need it. `wdc diff` answers "what grew since the last
  cleanup" by comparing two snapshots; it shows new and grown directories first, even when no rule
  knows them.
- **Discovery.** Large or fast-growing folders anywhere in the read scope that no rule matches are
  reported as `unknown`. Unknown items are never actionable; they are what an agent investigates
  next. With `%ProgramData%` in the read scope, this would have caught both the Cowork bundle and
  the NVIDIA artifacts.

### 4.2 Rule base (evidence-based)

Rules are data files in the repo (`rules/*.toml`), reviewed like code, and **compiled into the
executable** at build time. Only built-in rules can make an item actionable. A local rules folder
may add rules for discovery and explanation, but those rules are always report-only, so an agent
that writes a rule file cannot make something deletable. Each rule has:

- `id`, `title`, `paths` (environment-variable based patterns, never a user name);
- `class`, one of:
  - `regenerable-cache`: the owner rebuilds it automatically; deleting costs only time or a
    download;
  - `install-leftover`: left behind by an install or update that has finished;
  - `app-managed`: the app manages it; the right action is the app's own setting or tool, and the
    rule only explains how;
  - `never-touch`: user data, system files, or anything whose loss is not recoverable;
- `evidence`: how we know, as text plus a source (vendor documentation, an observed case with its
  date, or a check the tool runs);
- `conditions`: checks the tool runs before an item is eligible, chosen from a fixed set
  implemented in code (no scripts in rules), for example `process-not-running`, `older-than`,
  `listed-in-file`, `not-referenced-by`, `version-matches-installed`, `name-matches-regex`;
- `action`: `quarantine`, `report-only`, or `open-app-setting` (with the instructions);
- `cost`: what happens after removal (for example "re-downloads 8.6 GB", "shaders recompile, first
  game launch stutters");
- `last_verified`: the date the rule was last confirmed on a real machine.

Starting rules, from what was real on 2026-11-07:

| Rule | Class | Paths | Conditions and evidence |
|---|---|---|---|
| VS Code obsolete extensions | install-leftover | `%USERPROFILE%\.vscode\extensions\<folder>` | folder is listed in `extensions\.obsolete` **and** not referenced by `extensions.json`; `Code.exe` not running. VS Code writes `.obsolete` to mark folders it will remove; it often does not finish. |
| VS Code GUID staging folders | install-leftover | hidden folders in `.vscode\extensions` named like a GUID | name matches the GUID pattern; older than 1 day; not referenced by `extensions.json`; `Code.exe` not running. These are extraction folders from interrupted installs. |
| VS Code caches | regenerable-cache | `%APPDATA%\Code\Cache`, `CachedData`, `CachedExtensionVSIXs`, `Code Cache`, `GPUCache` | `Code.exe` not running. Rebuilt on next start. |
| VS Code family | same rules | `.vscode-insiders`, `.cursor`, and the C/C++ extension's `ipch` cache | same conditions per product. `workspaceStorage` is report-only (holds per-workspace state). |
| NVIDIA app OTA artifacts | install-leftover, **report-only in version 1** | `C:\ProgramData\NVIDIA Corporation\NVIDIA app\UpdateFramework\ota-artifacts` | leftover when the artifact's driver version is **equal to or older than** the installed driver. NVIDIA and Windows number driver versions differently (Windows `32.0.15.6094` is NVIDIA `560.94`), so the rule converts before comparing. A newer artifact may be a pending update. Removing it needs admin, so version 1 only explains the exact steps. |
| NVIDIA shader caches | regenerable-cache | `%LOCALAPPDATA%\NVIDIA\DXCache`, `GLCache`, `%APPDATA%\NVIDIA\ComputeCache` | no game or GPU app holding files open (files in use are skipped). Cost: shaders recompile. The lasting fix is the "Shader Cache Size" setting in the NVIDIA app, which the rule explains. |
| Steam web cache | regenerable-cache | `%LOCALAPPDATA%\Steam\htmlcache` | `steam.exe` not running. |
| Claude desktop data and Cowork VM bundle | app-managed | `%LOCALAPPDATA%\Claude-Data\**`, `%APPDATA%\Claude\**` | `report-only`. Observed 2026-11-07: the bundle existed under two paths that were hard links to one file; deleting one freed 0 bytes, and the app re-downloads it. Claude's own `Claude-Data\README.txt` says never to delete that folder and to use Settings > This computer > System > Storage, or Help > Troubleshooting > Clear Cache and Restart. The rule shows those steps and the link count. |
| Windows component store and system | never-touch / app-managed | `%WINDIR%`, `WinSxS`, `pagefile.sys`, `hiberfil.sys`, `Windows.old`, reserved storage, Recycle Bin, error reports, memory dumps, `ProgramData\Package Cache` | never touched. The rule shows the size and points to Windows' own tools (Storage Sense, Disk Cleanup, `DISM /Online /Cleanup-Image /StartComponentCleanup`, emptying the Recycle Bin) as advice only. `Package Cache` holds installers that uninstall and repair need. |

Exact path patterns are confirmed against the machine before each rule ships; only top-level folder
names were checked for this plan.

Adding a rule is a reviewed change with its evidence. When an agent finds an `unknown` item, its
output is a proposed rule plus the evidence, not a deletion.

### 4.3 Three faces, one engine

All three call the same engine (a Rust library). None has cleaning logic of its own.

| Face | For | What it is |
|---|---|---|
| `wdc.exe` | humans in a terminal, scripts | headless command line, JSON output on every read command |
| `WinDevCleaner.exe` | humans | installable desktop app with a window |
| `wdc mcp` | any AI agent with MCP (Claude Code, Codex, others) | MCP server that can drive the **whole** app, while the human watches it in the window |

The command line and the app are two executables because a Windows program is either a console
program or a window program; one file doing both flashes a console window. The installer puts both
in place and adds `wdc` to `PATH`.

### 4.4 Command line

`wdc.exe` commands:

| Command | Writes? | What it does |
|---|---|---|
| `wdc scan [--overview] [--json]` | snapshot only | gentle scan of the read scope and system items; saves a snapshot |
| `wdc diff [--since last-apply]` | no | what grew or appeared between two snapshots |
| `wdc explain <path> [--json]` | no | rule match, class, evidence, conditions and their results, hard-link names, size on disk, owning process if running |
| `wdc plan [--class ...] [--rule ...] -o plan.json` | writes the plan file | the eligible items with their evidence and expected bytes freed (a file whose other names stay outside the plan counts zero) |
| `wdc apply --plan plan.json` | **yes** | needs human presence proof; see section 7 |
| `wdc quarantine list`, `wdc restore <id>`, `wdc purge` | yes | manage quarantine; same proof |

### 4.5 Desktop app

A window with: where the space is (tree with sizes and growth since the last scan), candidates
grouped by class with their evidence, the plan with a checkbox per item, apply with a live
free-space readout, and the quarantine with restore. It redraws only when something changes, so it
uses no CPU while idle, and it runs only while open: no service, no tray process.

### 4.6 Agent mode: the agent drives the app, the human watches

The model is the browser MCP: the agent operates the real application, and the human sees every
step in it.

- **Connection.** The agent's configuration starts `wdc mcp` (stdio, the transport every MCP client
  supports). If the app is open, `wdc mcp` connects to it through a local named pipe that only the
  same Windows user can open, and every call goes through the app. If the app is closed, `wdc mcp`
  runs the engine headless; with `--show` it opens the app first.
- **What the app shows.** A banner "Agent connected: <client name>" (the name the MCP client
  reports, such as Claude Code or Codex), and a live activity panel: each call as it happens
  ("scanning `%LOCALAPPDATA%`", "explaining `.vscode\extensions\...`", "plan drafted: 12 items,
  6.1 GB"), with its status and result. The views follow along, so the human sees the same folder or
  plan the agent is looking at.
- **Human controls.** "Pause agent" and "Disconnect agent" buttons are always visible. A paused
  agent's calls wait; a disconnected one gets an error.
- **What the agent can do.** Everything a human can do in the app: scan, diff, explain, open views,
  draft and edit plans. To apply, purge or restore it calls `request_apply`, which opens an approval
  card in the app with the plan and its evidence; the human approves with Windows Hello (section 7).
  The agent gets back approved, rejected or timed out. With the app closed, `request_apply` returns
  "needs the human: open the app" and nothing is changed.
- **Cost.** The extra pieces are the named-pipe link and the activity log, both small. The window
  itself is the larger piece and is shared with plain human use. Nothing runs when neither the app
  nor an agent is using it.

## 5. Safety model

### 5.1 Plan files

A plan is **a list of choices, not a list of authorisations.** Each item has its path, the file ID
of the item itself (folder or file), the file IDs of the files of 1 MB or more inside it, rule id,
class, size on disk, expected bytes freed, and the time of the scan. Listing every small file of a
100,000-file cache would make plans huge and every item "changed"; it is not done.

`apply` trusts nothing in the plan file. For every item it:

1. re-runs the matching against the built-in rules and re-checks every condition (process not
   running, driver version, `.obsolete` listing) at that moment;
2. opens the item **once**, without following links (`FILE_FLAG_OPEN_REPARSE_POINT`), and checks on
   that open handle its file ID and its real final path (`GetFinalPathNameByHandleW`) against the
   allowed and forbidden roots. Short names (`PROGRA~1`), `..` and trailing dots are resolved by
   Windows itself this way, not by string comparison;
3. renames it into quarantine **through the same handle** (`SetFileInformationByHandle` with
   `FileRenameInfoEx`), so nothing can be swapped in between the check and the move.

Items that changed, no longer match, or fail a condition are skipped and reported. A plan older than
24 hours is refused as a whole.

### 5.2 Quarantine before delete

- `apply` **renames** items into a quarantine folder on the same volume
  (`%LOCALAPPDATA%\WinDevCleaner\quarantine\<plan-hash>\` on C:, `<drive>:\.wdc-quarantine\` on other
  volumes). A rename is instant and fully reversible with `wdc restore`.
- The quarantine folder itself is created by the tool, owned by the user, checked on an open handle
  like any item, and refused if it is or contains a link.
- Quarantine frees **no** space by itself. Space is freed by `wdc purge`, which deletes quarantined
  items older than 7 days (owner decision, section 9).
- Early purge (before 7 days) is allowed only for items of class `regenerable-cache` or
  `install-leftover` whose `explain` output was shown in the approval step (owner decision, section
  9), by the same rule on every path: `apply --purge-now` and `purge --now` both enforce it.
- Every command prints the bytes currently held in quarantine, so the space it occupies on a tight
  disk is never hidden.
- `restore` puts an item back only if its original path is free. If the app has recreated the
  folder meanwhile, restore stops and reports both, and the human chooses.
- Quarantine paths are kept short (`q\<n>\`) and long-path aware, so moving an item never pushes it
  past path-length limits.
- An item that cannot be renamed (file in use, access denied) is skipped and reported. There is no
  forced deletion.

### 5.3 Allowed and forbidden roots

- **Act scope (allowed roots):** `%LOCALAPPDATA%`, `%APPDATA%`, `%TEMP%`, `%USERPROFILE%\.vscode`
  (and `.vscode-insiders`, `.cursor`), `%USERPROFILE%\.cache`, and package-manager caches (`npm`,
  `pip`, `uv`, `nuget`, `gradle`). `%ProgramData%` is read but not acted on in version 1.
- **An allowed root itself is never an item.** Only things inside it are, so `%TEMP%` the folder
  can never be moved, only its contents.
- **Forbidden, always**, whatever a rule says:
  - `%WINDIR%` and everything under it (including WinSxS), `Program Files` and
    `Program Files (x86)` (programs are removed only by their own uninstaller), the volume root;
  - `%USERPROFILE%` itself and its user folders (Documents, Desktop, Pictures, Videos, Music,
    Downloads, OneDrive);
  - Windows secrets: `%APPDATA%\Microsoft\Protect` (encryption keys), `Microsoft\Credentials`,
    `Microsoft\Vault`, `.ssh`, `.gnupg`, and any other credential store;
  - virtual disks (`*.vhd`, `*.vhdx`: WSL, Docker, VMs), Outlook data (`*.pst`, `*.ost`), and
    messenger data folders (WhatsApp, Telegram, Signal);
  - browser profile data other than named cache subfolders;
  - any folder that contains a `.git` directory;
  - the quarantine folder.
- Every check is made on the real final path of an open handle (section 5.1), never on the text of
  a path.

### 5.4 Free space measured every time

`apply` and `purge` read free space on each affected volume (`GetDiskFreeSpaceExW`) before and
after, and report expected against actual. A gap is explained where possible and recorded with the
snapshot: other links of the same file, files in use or still pending deletion, other processes
writing meanwhile, and System Restore, which can keep the old blocks of deleted files in shadow
storage until its own limit is reached.

### 5.5 Programs

The old Obliterate mode is dropped. Its replacement is read-only: list installed programs from the
uninstall registry keys (per-user and per-machine) with their size and install date, and show the
program's own uninstall command or the `winget uninstall --id <exact id>` command for the human to
run. After an uninstall, folders left in the allowed roots whose name equals the program's
publisher or product name are reported as `install-leftover` candidates, which still need a human to
confirm. Nothing matches by substring.

## 6. Stack

Decided by a cold analysis on 2026-11-07, at the owner's request (section 9), with a local
micro-benchmark run on this laptop at low priority and published evidence. "Measured" means
measured on this laptop; "estimate" means not measured.

### 6.1 The bottleneck is the Windows call, not the language

Local benchmark, warm cache, `%USERPROFILE%\.vscode\extensions` (29,377 files, 464 directories,
1.98 GB), each method single-threaded in background mode, median of 3 runs, through Python `ctypes`:

| Method | Wall time | Hard-link dedup | Size on disk |
|---|---|---|---|
| `FindFirstFileExW` (basic, large fetch) | 0.085 s | no | no |
| `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)` | **0.094 s** | yes (128-bit IDs) | yes |
| `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)` | 0.108 s | yes | yes |
| `NtQueryDirectoryFile` | 0.108 s | yes | yes |
| `os.scandir` + `os.stat` on every file | 2.33 s | yes | no |
| `GetFileInformationByName` on every file | 2.43 s | yes | yes |
| open every file + `GetFileInformationByHandle` | 2.81 s | yes | yes |

- Reading a whole directory per call is about **25 times faster** than asking about each file, and
  about 90% of the slow methods' time is inside Windows. A compiled language gains at most about 2x
  over Python on the fast path (estimate).
- **Chosen enumeration:** `FileIdExtdDirectoryInfo` (falls back to `FileIdBothDirectoryInfo` on file
  systems without it). It returns 128-bit file IDs, sizes on disk, attributes and the reparse tag,
  so links and cloud placeholders are recognised without opening anything. Per-file checks
  (`GetFileInformationByName`, about 80 µs each, measured) are kept for large files and plan
  candidates only, as section 4.1 describes.
- **No shortcut without admin (measured):** the USN change journal is readable without admin but
  hides file names, and on this C: it holds only about 17 hours of activity (32 MiB). Reading the
  MFT directly needs admin. Snapshots stay the way to answer "what grew".
- **Hard-link sizes can be stale** in directory entries of the other names (published:
  [Raymond Chen, The Old New Thing](https://devblogs.microsoft.com/oldnewthing/20111226-00/?p=8813)).
  This is why large files are re-read individually.

### 6.2 Decision: Rust

| | Rust | Go (runner-up) | Python |
|---|---|---|---|
| Memory during a 200k-file scan | 6 to 15 MB (estimate), no garbage collector | 15 to 30 MB (estimate), garbage collected | 35 to 50 MB for the scanner alone (extrapolated from measurement), 60 to 100 MB with a UI and MCP |
| Start-up | under 10 ms (estimate) | under 10 ms (estimate) | 107 ms with the needed modules (measured) |
| Executable | one `.exe`, 4 to 8 MB (estimate) | one `.exe`, 12 to 20 MB (estimate) | interpreter and packages on C:, or a 15 to 40 MB bundle |
| Windows API bindings | `windows-sys`, generated from Microsoft's metadata, every structure typed | `x/sys/windows`; the directory-record structures and `FindFirstFileNameW` must be declared by hand | `ctypes` by hand |
| Robustness | memory- and data-race safety checked at compile time; an ignored error is a compiler warning; exhaustive matching over rule classes and actions | memory-safe; errors are easy to ignore silently; races found only at run time | memory-safe; exceptions |
| MCP SDK | official, Tier 1 (`rmcp`; moved from 0.x to 3.x in 20 months, so the version is pinned) | official, Tier 1, stable v1 API | official, Tier 1 |
| Desktop UI | egui (no web engine) | Fyne or Wails | Qt (PySide), heavy |
| Build cost | heaviest (1 to 3 min, 1 to 2 GB build folder), paid only in the cloud and on CI | light | none |

Zig and C++ were dropped (no official MCP SDK; Zig is pre-1.0; C++ is not memory-safe for a tool
that decides what to delete). C# Native AOT was dropped because it cannot build Windows binaries from
the Linux cloud session at all. Sources for every row are in the analysis notes.

**Why Rust:** for a kernel-bound workload all compiled languages tie on speed, so the deciding
criteria are the other two the owner set. Rust is the most robust (the compiler rejects whole
classes of mistakes, which matters most in code that deletes files) and the lightest to run (no
garbage collector, lowest memory floor, smallest executable). Its real cost, build weight, never
touches the laptop: builds run in the cloud session and on the `windows-latest` runner.

**The product is Rust only. Nothing is hybrid.** Go was the runner-up of the comparison, nothing
more: no Go code is written. The first slice measures the Rust scanner against fixed budgets
(section 6.4). Go is reconsidered only if Rust misses those budgets, or if the cloud session cannot
build Windows binaries in Rust, and then as a replacement decided by the owner, never alongside.

### 6.3 Parallel walk

- **Default: one thread in background mode,** with optional `--throttle`. Warm, a compiled scanner
  finishes this scope well under a second (estimate); extra threads would compete with what the
  owner is doing.
- Cold cache on an NVMe SSD, parallel walks are 5 to 7 times faster in published Linux benchmarks
  ([gdu](https://github.com/dundee/gdu), [diskus](https://github.com/sharkdp/diskus)); a single thread
  here is estimated at 2 to 10 s cold, acceptable for a background tool.
- **Optional `--jobs N`, capped at 4, still in background mode,** for the desktop app when the
  owner is waiting at the screen. The design keeps per-worker totals that merge cleanly, but the option
  ships only if the benchmark shows a cold-cache gain above 2x.

### 6.4 Budget check in the first slice

Runs on the `windows-latest` runner, never on the laptop, against the real Rust scanner of slice 1
(section 6.1's enumeration with background mode, dedup, per-directory totals and one snapshot
write). It becomes a permanent CI check, so the scanner can never quietly get heavier.

- **Fixture,** generated on a fresh 4 GiB VHDX (detached and re-attached for cold runs): 200,000
  files in about 25,000 directories shaped like `node_modules` and VS Code extensions; 1,000
  hard-link pairs and one file with a name outside the scanned root; a stale-size link; junctions
  (one looping, one to `C:\Windows`) and symbolic links; a sparse file; a compressed file; real
  cloud placeholders from a test sync root whose data-fetch callback counts every call; an offline
  file; a path over 260 characters; Unicode names; a directory the user cannot read; a file held
  open exclusively.
- **Metrics:** wall time, CPU time, peak working set and peak private bytes, IO counters, start-up
  time, executable size, build time; warm (5 runs) and cold (3 runs); `--jobs` 1, 2 and 4.
- **Pass criteria:** results equal to the generator's expected values (every hard-link group
  counted once, stale size corrected, no link followed, zero data-fetch callbacks, denied and locked
  items reported without failing). Budgets: warm scan of 200k files at most 2 s, cold at most 15 s,
  peak private memory at most 40 MB, start-up at most 50 ms, `wdc.exe` at most 15 MB. The desktop app
  gets its own budget in slice 1: idle CPU about 0%, memory while open at most 80 MB (to be
  confirmed by measurement).
- The runner is elevated, so behaviour without admin cannot be reproduced there; that part was
  already measured on the laptop.

### 6.5 Supporting choices

- `windows-sys` for Windows calls; Windows Hello through the `windows` crate's WinRT bindings.
- Snapshots as compressed JSON (`serde_json` + `zstd`) in version 1. If SQLite is ever needed,
  `rusqlite` with its bundled SQLite builds on the Windows runner with no setup.
- TOML rules through `serde` and `toml`, embedded in the executable at build time.
- Messages in English and pt-BR through a message catalogue (for example `fluent` or a small
  built-in table).
- Desktop app: **`egui`** (pure Rust, MIT/Apache licence, redraws only on events, no web engine).
  Considered and not chosen: Tauri (nicer web-based UI, but the WebView2 engine adds an estimated
  80 to 150 MB while open) and Slint (light, but its licences need care in an MIT project). The
  slice-1 budget check confirms the choice.
- MCP: `rmcp`, version pinned; the app link is a named pipe whose access list allows only the
  current user.
- Releases: a tag builds both executables and an installer (MSI) on the Windows runner; a `winget`
  manifest can follow.

## 7. How the human approves

- `scan`, `diff`, `explain` and `plan` work freely for any agent, through MCP or the shell.
- **The main path is the app:** the agent drafts the plan, the app shows it as an approval card with
  each item's evidence, and the human approves there with Windows Hello (section 4.6).
- `apply`, `purge` and `restore` show the plan summary (items, classes, expected bytes freed) and
  then require **proof that a person is present**, which an agent driving a terminal cannot produce:
  - a **Windows Hello** prompt (PIN, face or fingerprint) through `UserConsentVerifier`, when Hello is
    set up;
  - otherwise a random one-time code shown only in a desktop dialog, never printed to the console,
    which the human types into the console.
- An interactive console alone is not accepted as proof: agents can open real terminals, and the
  plan hash is known to whoever wrote the plan.
- Limits, stated plainly: this stops accidents and casual misuse. An agent that also controls the
  screen (computer use) could read the dialog; Windows Hello is the barrier for that case.
- The repo ships a Claude Code permissions snippet that denies `wdc apply`, `purge` and `restore`.
  It is a convenience only: such rules match command text and are bypassed by `wdc.exe`, a full
  path or `cmd /c`.
- Without the app: the agent writes `plan.json` and explains it; Paulo reviews it with
  `wdc plan show plan.json`, then runs `wdc apply --plan plan.json` in his own terminal.

## 8. Delivery: vertical slices

Work is delivered in **vertical slices, never in layers**. Each slice is one pull request that gives
the owner one capability he can use, cut through every part it needs: engine, `wdc.exe`, MCP tool,
the desktop app view, the agent's activity in the app, messages in both languages, and its tests.
No slice builds "the engine" or "the UI" on its own.

Code, builds and tests run in a **cloud session** on `paulo-raoni/WinDevCleaner`; Windows-specific
tests run on a GitHub Actions `windows-latest` runner, free because the repository is public
(checked 2026-11-07). Nothing heavy runs on the laptop: Paulo only installs and runs the release.

| Slice | What the owner can do after it | Proven by |
|---|---|---|
| 1. **Where is my space?** (walking skeleton) | Run `wdc scan`, open the app and see the tree of the read scope with sizes on disk; connect Claude Code or Codex to `wdc mcp`, watch "Agent connected" and each scan appear live in the app; pause and disconnect the agent. Installer, both executables, English and pt-BR. Old code moved to `legacy/` as `.txt` files. | Windows CI fixtures: a 64 MB file with two names counts once, also when the second name is outside the scanned root; a junction is not followed; sparse and compressed files count their allocation; the budget check of section 6.4 (scanner and app); a 1,000,000-file tree under the memory and time limits, with background mode checked; an MCP client test that drives a scan and sees it mirrored in the app's activity log; a pipe test that another user cannot open. Every test is first shown failing with its guarding line removed. Placeholder handling is tested only if a Cloud Files sync root can be registered on the runner; otherwise it is listed as untested. |
| 2. **What grew since last time?** | `wdc diff` and the app's "growth" view; an agent can ask "what grew" and the app shows the answer it is reading; system items (Recycle Bin, hibernation, error reports, dumps) appear with the Windows tool that manages each. | `diff` on two snapshots of a fixture that grew; snapshot retention keeps only N and reports their size; system-item sizes read from their APIs. |
| 3. **Why is this here, and what would deleting it free?** | `explain` everywhere: rule match, class, evidence, every name of a hard-linked file and its true size on disk. The first rules: the VS Code family, NVIDIA artifacts and shader caches, Steam, Claude desktop data (report-only), Windows items (never-touch). Unknown large or growing folders are flagged. | fixture trees per rule (an `.obsolete` list, GUID folders, driver versions older, equal and newer than installed, the Windows-to-NVIDIA version conversion); the Cowork case reproduced: two names, one outside the plan, "expected freed: 0"; a local rule file cannot make an item actionable. |
| 4. **Clean the VS Code leftovers safely** (first slice that changes the disk) | An agent drafts a plan, the app shows the approval card, the owner approves with Windows Hello; items move to quarantine, free space is measured before and after, early purge is allowed for caches and install leftovers, `restore` works. The same from `wdc apply` with presence proof. | on a small VHDX created by the test: apply then restore gives an identical tree; restore refuses when the folder was recreated; an item swapped for a junction after planning moves nothing; a condition that became false (fake process started) skips the item; a forbidden-root suite (`..`, short names, trailing dots, a junction into `C:\Windows`, user folders, an allowed root itself); purge before 7 days refuses items that are neither cache nor install leftover, on both paths; the hard-link fixture reports expected 0 and measures 0; apply without presence proof refuses, also from a pseudo-console (ConPTY) and from MCP without the app. |
| 5. **Clean the other known cases** | The cleaning path of slice 4 opened to the shader caches, Steam and the other cache rules; NVIDIA stays report-only in version 1 with exact steps. | the slice-4 suite run for each rule's fixture. |
| 6. **Faster scan when I am waiting** (only if measured worth it) | Optional `--jobs` (at most 4) and the app's "scan faster" switch; optional whole-C: overview. | cold-cache benchmark shows more than 2x gain for `--jobs 4`; otherwise this slice is dropped and the reason recorded. **Built on the owner's decision of 2026-11-11** at 1.66x: see below. |
| 7. **Which programs take the space, and how do I remove one?** (section 5.5) | `wdc programs`, the app's programs view and an MCP tool: installed programs from the per-user and per-machine uninstall keys with size and install date, and the program's own uninstall command or `winget uninstall --id <exact id>` for the human to run; after an uninstall, leftover folders in the allowed roots named exactly as the publisher or product are reported as `install-leftover` candidates. Read-only: the replacement of the old Obliterate mode. | fixture uninstall keys (per-user and per-machine, 32- and 64-bit views) read back exactly; a leftover named by exact match is reported and a substring is not; nothing on disk or in the registry changes. |

**Slice 6 measured, then decided by the owner (2026-11-11).** The budget check measured, on the
`windows-latest` runner, a cold walk of the 202,108-file, 25,015-folder fixture on a VHDX detached and
re-attached before each run, with the scanner's own Windows calls: 1 worker 4,831 ms, 4 workers 2,914 ms
(medians of 3, interleaved), a ratio of 1.66. The 2x threshold above was written by the session that
drafted this plan, not decided by the owner; a session first applied it and dropped the slice. The
owner's words: "why would I leave out an improvement that gives me a 40% gain?" So the slice is built:
`--jobs` up to 4, opt-in, the default staying one thread in background mode. The optional whole-C:
overview is not part of it. The measurement stays in the budget report
(`tools/wdc-bench/src/parallel.rs`).

Slice 7 was missing from this table although section 5.5 asks for it: without it, the old Obliterate
mode would have no replacement (section 10, point 2). It was added on 2026-11-11.

Slice 1 is the largest because the skeleton touches every face at once; each later slice is
smaller. Slices 1 to 3 never change the disk and already would have found and explained all three
cases of 2026-11-07.

The first real run on this laptop, after slice 1, is a scan in background mode with the app open,
watched in Task Manager for CPU and memory.

## 9. Owner decisions (2026-11-07)

1. **Language:** not tied to Python or to any stack. The choice goes to whatever is fastest, most
   robust and lightest to run. The cold analysis chose **Rust** (section 6); the product is Rust
   only, with no hybrid.
2. **Interface language:** both English and Brazilian Portuguese. Every user-facing message goes
   through a message catalogue from slice 1; English is the default, `pt-BR` follows the Windows
   display language or `--lang`. JSON output for agents stays English and stable.
3. **Quarantine time:** the prudent default, 7 days before purge.
4. **Legacy code:** moved to `legacy/` in slice 1 and kept as a reference during development. It is
   deleted only after the full migration (see the definition of done, section 10).
5. **Early purge on a full disk:** `regenerable-cache` and `install-leftover` items may be purged
   before the 7 days, once the human has seen their `explain` output in the approval step. Every
   other class waits the full 7 days.
6. **Legacy kept unrunnable:** the old files go to `legacy/` with a `.txt` suffix added (for example
   `legacy/cleaner.py.txt`), readable as reference but impossible to run by accident.
7. **Three faces:** a headless command line, an installable desktop app, and an MCP server through
   which any agent can drive the whole app while the human watches every step in it (section 4.3).
   The terminal UI of the first draft is dropped.
8. **Vertical slices,** never layers (section 8).

## 10. Definition of done for the rebuild

1. Slices 1 to 5 are merged, each with its tests passing on the Windows runner; slices 6 and 7 are
   merged.
2. Every capability of the old tool that is kept has a replacement; the ones dropped (Deep, Nuclear,
   Obliterate deletion) are listed in the README with the reason.
3. The README is rewritten for the new tool.
4. **Final step: delete the `legacy/` folder** in its own commit.
