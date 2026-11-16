# Changelog

All notable changes to WinDevCleaner are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [1.0.0-rc.2] - 2026-11-12

### Added

- `scan --json` says when it cut a list: `errors_listed` / `errors_truncated` and `links_listed` / `links_truncated` beside `errors` and `links` (200 of each are listed while `totals` counts them all), and `errors_by_kind` counts the recorded errors by kind (the first 10,000 are recorded).
- `system_items`: the status `partial-needs-admin` (a folder with access errors below it), shown as "at least N; the rest needs an administrator", and `total_is_lower_bound` beside `total_bytes`. A missing `hiberfil.sys` on the system drive is reported `absent` with the note "hibernation is off". Items carry an optional English `note`.
- `programs --json` carries `leftovers_note`: why the folders left behind were not checked (first look, list saved; or N uninstall keys could not be read). The command line and the app say the same in both languages; the unreadable-keys case had no message before.
- `explain` carries `inside_allowed_root` and `checked_path` in `act_scope`.

- An icon for `WinDevCleaner.exe` and `wdc.exe` (Explorer, Start menu, taskbar), for the app window, for the Start menu shortcut and for the entry in Settings > Apps.
- The installer shows the MIT licence and a finish page that says where WinDevCleaner was installed and that `wdc` works in any new terminal, with an "Open WinDevCleaner now" option (checked by default). Silent installs (`/qn`) still show nothing.

- Every MCP tool result carries `app` (`connected`, `headless` or `blocked`, plus `app_reason` when blocked), and the server's instructions say whether the person is watching in the app: a headless server no longer claims they are.

### Changed

- `explain`: the `act_scope` field `is_allowed_root` was renamed `is_root_itself` on the wire. It always meant "this path IS an allowed root", which read as "inside an allowed root"; use `inside_allowed_root` for that. The rename is made now, before the JSON contract freezes at 1.0.0.
- `wdc scan --top N` now limits both lists of the human output, the largest folders and the largest files (defaults 20 and 10); the help says so. The JSON `top_files` is as before and `largest_dirs` is still cut by `--depth`.
- `wdc plan` ends the list of items left out with "... and N more left out (`--json` lists them all)" instead of the diff's "more changes (see --limit)".

### Fixed

- A `system_items` folder with access errors below it no longer counts its partial size as a full measurement (status `partial-needs-admin`, total marked as a lower bound).
- `wdc mcp` started with the app closed now looks for the app before each call and switches to it when it is opened, so the next call shows in the app's activity panel. An agent the person disconnected, or one refused because another agent is attached, still gets errors and never falls back to headless.
- `plan` leaves out an item another program holds open ("held open by: NVIDIA Container, NVIDIA Overlay. Close them and draft again."), found through the Windows Restart Manager, which only reads: no program is closed or signalled. For a large folder only the first 4,096 files are checked.
- `apply` names the holders when an item is held open after the plan ("held open by X; close it and run again") and reports an open failure (`open-failed`) apart from a rename failure (`rename-failed`); nothing is ever forced. `apply --json` carries `holders` and `stage` on such a skip.
- `temp-old` (and the VS Code staging-folder rules) no longer offer a folder whose contents changed recently: the age is that of the newest file inside, not only the folder's own date.
- The free-space line after `apply` says that the items are in quarantine and the space returns when they are purged, with the expected gain, instead of "expected 0 B" beside a small drop.
- `apply --json` and the `request_apply` reply carry `skipped_bytes` and `purged_bytes`, and the reply carries `moved`, `moved_bytes` and `skipped` beside `report`.
- The release no longer carries the install log and the `.wixpdb`.
- `explain`, `plan` and the approval card now show the real size of a file (a 541 MB `update.exe` showed 0 B), and a file with another name outside the item still counts as freeing nothing.
- `explain` says when a size is partial ("N items could not be read") and `--json` carries `errors`, `complete` and `first_error_kind`, instead of showing a folder it could not read as 0.
- `plan` no longer offers an item whose size is incomplete or an item that would free 0 bytes; both are listed under "left out" with their own reason.
- Rule sources are neutral, checkable statements (the product's documentation or the layout it keeps) instead of notes about one machine, and every rule now has a pt-BR source (`source_pt_br`, optional in local rule files), shown by `explain` in the CLI and the app. A test keeps the built-in rule files free of personal and private-machine wording.
- The pip rule's evidence names both cache locations; the NVIDIA artifact rule says deleting it "may need" an administrator.
- A `User Data` folder is treated as a browser profile only when it holds Chromium's `Local State` file (or cannot be checked) or sits under a known browser vendor's folder, so an application's own `User Data\Download` (CapCut) is no longer forbidden. Real browser profiles stay forbidden.
- An item that sits inside the allowed roots by name but resolves through a link to a place outside them (Edge's `User Data` junction) is refused with the reason `resolves-outside-allowed-roots` and the real path ("resolves through a link to D:\..., outside the allowed roots"). Links are still never followed.

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
