# Changelog

All notable changes to WinDevCleaner are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [1.0.0-rc.1] - 2026-11-11

The tool as delivered by the rebuild. A pre-release: it has not yet been used on a real machine.

### Added

- `wdc scan`: where the space is, as sizes on disk, in Windows background mode, with a compressed snapshot saved per scan.
- `wdc diff`: what grew, shrank, appeared or disappeared between two scans, and the large files that are new or grew.
- System items (Recycle Bin, paging, swap and hibernation files, error reports, dumps, live kernel reports, System Restore), read through Windows' own APIs and always report-only.
- `wdc explain` and the rule base: why a folder is there, the rule, class and evidence, every condition, the allowed and forbidden checks, and what deleting it would free; 17 cleaning rules, plus user rules in the rules folder that are always report-only.
- `wdc plan`, `apply`, `quarantine`, `restore` and `purge`: a plan of choices, proof that a person is present (Windows Hello or a one-time code in a desktop dialog), quarantine by rename, restore, purge after 7 days (earlier only for caches and install leftovers), and free space measured before and after.
- `wdc scan --jobs N` (1 to 4) and the app's "Scan faster" switch.
- `wdc programs`: the installed programs with the figure each installer reports and its own uninstall command, read-only, and report-only leftovers of programs that are gone.
- `wdc mcp`, an MCP server for AI agents (`scan`, `tree`, `diff`, `system_items`, `programs`, `explain`, `plan`, `quarantine`, `request_apply`), and the desktop app's agent mode: every call visible live, pause and disconnect buttons, and approval cards. An agent can never apply, restore or purge.
- `WinDevCleaner.exe`, the desktop app, and the command line in English and Brazilian Portuguese (`--lang en|pt-BR`).
- A per-user MSI installer that adds the install folder to `PATH` and a Start menu shortcut.

### Removed

- The old Python tool, still in the git history (last commit with it: `5ff166f`), and its modes:
  - Cleaner DEEP: it removed whole installed programs and the `Temp` folder itself, and in a real run reported 10.5 GB freed while about 1.4 GB was removed.
  - Cleaner NUCLEAR: it deleted all of `AppData\Local\Programs` and the Edge `User Data` folder (profiles, history, saved passwords); deleting programs and user data is not cleaning.
  - Obliterate's folder deletion: it built paths from typed text and deleted them with no dry run or confirmation, so `.` or `..\Windows` reached folders such as `C:\Program Files` and `C:\Windows`.

### Known limitations

- Not yet used on a real machine; Windows behaviour has been tested only on GitHub's `windows-latest` runner.
- The MSI is not code-signed, so SmartScreen shows "unknown publisher".
- With `--jobs` greater than 1, a scan holds each folder's record until it merges them and uses more memory than with one job.
- Only 17 built-in rules clean anything; everything else is explained or report-only.
- NVIDIA app update artifacts under `ProgramData` are never deleted by the tool (that needs administrator rights); `explain` gives the manual steps.
- System Restore can be read only by an administrator.
- The size shown by `wdc programs` is what the installer reports, and some report nothing; the tool never uninstalls.
- Updates, hotfixes, system components and Store (MSIX) apps are not listed by `wdc programs`.
- The crash dumps of the old tool's list are not cleaned; they are report-only.
