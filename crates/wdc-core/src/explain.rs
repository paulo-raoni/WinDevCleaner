//! `explain` (plan, sections 4.4 and 8, slice 3): why a path is here and what deleting it would
//! free. Read-only.
//!
//! It reports the rule that applies (class, evidence, cost, instructions), every condition
//! and its result, the act-scope verdict on the real final path, the size on disk, and every
//! name of each hard-linked file inside the item. "Expected freed" counts a file only when all
//! of its names are inside the item: a file with another name elsewhere frees 0 bytes (the
//! Cowork case of 2026-11-07).

use serde::Serialize;

use crate::act::{self, Forbid, Verdict};
use crate::fsops::{holder_names, Holder};
use crate::i18n::{Lang, Msg};
use crate::rules::{canon, Action, Class, CondResult, Condition, Context, Found, Origin, Rule, RuleSet};
use crate::scan::{scan, ScanOptions, DEDUP_THRESHOLD};

/// A file of at least 1 MB with more than one name.
#[derive(Clone, Debug, Serialize)]
pub struct HardLinked {
    pub path: String,
    pub bytes_on_disk: u64,
    pub links: u32,
    /// Every name of the file, as Windows lists them.
    pub names: Vec<String>,
    /// The names outside the item: while one exists, deleting the item frees nothing of it.
    pub names_outside: Vec<String>,
}

/// Why an item cannot be acted on.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum WhyNot {
    NoRule,
    LocalRule,
    ReportOnly,
    AppManaged,
    NeverTouch,
    ConditionNotMet { condition: Condition, found: Found },
    Missing,
    IsLink,
    AllowedRootItself,
    OutsideAllowedRoots,
    /// The item sits inside the allowed roots by its name, but a link leads its real place outside them.
    ResolvesOutsideAllowedRoots { final_path: String },
    Forbidden { forbid: Forbid },
    /// Part of the item could not be read, so its size is incomplete (set by `plan`).
    NotFullyReadable,
    /// Deleting the item would free 0 bytes (set by `plan`).
    NothingToFree,
    /// Another program has the item, or a file inside it, open (set by `plan`).
    HeldOpen { holders: Vec<Holder> },
}

impl WhyNot {
    pub fn text(&self, lang: Lang) -> String {
        match self {
            WhyNot::NoRule => lang.t(Msg::WhyNoRule).into(),
            WhyNot::LocalRule => lang.t(Msg::WhyLocalRule).into(),
            WhyNot::ReportOnly => lang.t(Msg::WhyReportOnly).into(),
            WhyNot::AppManaged => lang.t(Msg::WhyAppManaged).into(),
            WhyNot::NeverTouch => lang.t(Msg::WhyNeverTouch).into(),
            WhyNot::ConditionNotMet { found, .. } => lang.fmt(Msg::WhyCondition, &[("found", &found.text(lang))]),
            WhyNot::Missing => lang.t(Msg::WhyMissing).into(),
            WhyNot::IsLink => lang.t(Msg::WhyIsLink).into(),
            WhyNot::AllowedRootItself => lang.t(Msg::WhyAllowedRoot).into(),
            WhyNot::OutsideAllowedRoots => lang.t(Msg::WhyOutsideRoots).into(),
            WhyNot::ResolvesOutsideAllowedRoots { final_path } => lang.fmt(Msg::WhyResolvesOutsideRoots, &[("path", final_path)]),
            WhyNot::Forbidden { forbid } => lang.fmt(Msg::WhyForbidden, &[("why", forbid.text(lang))]),
            WhyNot::NotFullyReadable => lang.t(Msg::WhyNotFullyReadable).into(),
            WhyNot::NothingToFree => lang.t(Msg::WhyNothingToFree).into(),
            WhyNot::HeldOpen { holders } => {
                let msg = if holders.len() == 1 { Msg::WhyHeldOpenOne } else { Msg::WhyHeldOpen };
                lang.fmt(msg, &[("holders", &holder_names(holders))])
            }
        }
    }
}

/// Exact manual steps for an item the tool never deletes itself (version 1: NVIDIA update
/// artifacts, which may need administrator rights).
#[derive(Clone, Debug, PartialEq)]
pub enum ManualSteps {
    /// The artifact is not newer than the installed driver: how to remove it by hand.
    Remove { command: String, parent: String },
    /// Not to be deleted, and why (a newer artifact, or versions that could not be compared).
    Keep { found: Found },
}

/// One numbered step; `command` is the line to copy, shown apart from the sentence.
#[derive(Clone, Debug, PartialEq)]
pub struct ManualStep {
    pub text: String,
    pub command: Option<String>,
}

impl ManualSteps {
    /// The steps in a language. The command line itself is never translated.
    pub fn steps(&self, lang: Lang) -> Vec<ManualStep> {
        let plain = |text: String| ManualStep { text, command: None };
        match self {
            ManualSteps::Remove { command, parent } => vec![
                plain(lang.t(Msg::ManualCloseApp).into()),
                plain(lang.t(Msg::ManualOpenPowerShell).into()),
                ManualStep { text: lang.t(Msg::ManualRunCommand).into(), command: Some(command.clone()) },
                plain(lang.fmt(Msg::ManualExplainAgain, &[("parent", parent)])),
            ],
            ManualSteps::Keep { found: Found::DriverNewer { artifact, installed } } => {
                vec![plain(lang.fmt(Msg::ManualKeepNewer, &[("artifact", artifact), ("installed", installed)]))]
            }
            ManualSteps::Keep { found } => vec![plain(lang.fmt(Msg::ManualKeepUnknown, &[("why", &found.text(lang))]))],
        }
    }

    /// The `manual_steps` of the JSON output: stable English, the command as its own element.
    pub fn english(&self) -> Vec<String> {
        self.steps(Lang::En).into_iter().flat_map(|s| std::iter::once(s.text).chain(s.command)).collect()
    }
}

/// `Remove-Item` for `path`, quoted for PowerShell single quotes (a `'` is doubled).
fn remove_command(path: &str) -> String {
    format!("Remove-Item -LiteralPath '{}' -Recurse -Force", path.replace('\'', "''"))
}

/// The manual steps of the explained item, if its rule gives any: only `nvidia-ota-artifacts`,
/// and the command only when the artifact is not newer than the installed driver.
#[allow(unused_mut, unused_assignments)]
fn manual_steps(rule: &Rule, require: &[CondResult], real_path: &str) -> Option<ManualSteps> {
    if rule.id != "nvidia-ota-artifacts" || rule.origin != Origin::BuiltIn {
        return None;
    }
    let cond = require.iter().find(|c| matches!(c.condition, Condition::DriverVersionNotNewer { .. }))?;
    let mut give_command = true;
    // GUARD-BEGIN(nvidia-steps)
    give_command = cond.passed;
    // GUARD-END(nvidia-steps)
    if !give_command {
        return Some(ManualSteps::Keep { found: cond.found.clone() });
    }
    let parent = std::path::Path::new(real_path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    Some(ManualSteps::Remove { command: remove_command(real_path), parent })
}

/// The rule part of an explanation.
#[derive(Clone, Debug, Serialize)]
pub struct RuleInfo {
    pub id: String,
    pub title: String,
    pub class: Class,
    pub action: Action,
    pub origin: Origin,
    pub evidence: String,
    pub source: String,
    pub cost: String,
    pub instructions: Option<String>,
    pub last_verified: String,
    /// The whole rule, for the texts in other languages.
    #[serde(skip)]
    pub full: crate::rules::Rule,
}

/// Everything `explain` knows about a path.
#[derive(Clone, Debug, Serialize)]
pub struct Explanation {
    pub path: String,
    pub exists: bool,
    pub kind: &'static str,
    /// The path Windows resolves from an open handle (links not followed).
    pub final_path: Option<String>,
    /// The item the answer is about: the path itself, or the rule's item containing it.
    pub item: String,
    pub bytes_on_disk: u64,
    pub logical_bytes: u64,
    pub files: u64,
    /// Items inside it that could not be read: the size above leaves them out.
    pub errors: u64,
    /// Every part of it was read (`errors == 0`).
    pub complete: bool,
    /// The kind of the first read error; absent when complete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_error_kind: Option<&'static str>,
    /// What deleting the item would free: files with a name outside the item count 0.
    pub expected_freed_bytes: u64,
    pub hard_links: Vec<HardLinked>,
    /// Every file of at least 1 MB inside the item (a plan records their file IDs).
    #[serde(skip)]
    pub large_files: Vec<String>,
    pub rule: Option<RuleInfo>,
    /// No rule knows this path.
    pub unknown: bool,
    pub match_conditions: Vec<CondResult>,
    pub require_conditions: Vec<CondResult>,
    /// Programs the rule waits for that are running now.
    pub running_owners: Vec<String>,
    pub act_scope: Verdict,
    /// Whether a plan could include this item now.
    pub actionable: bool,
    /// Why not.
    pub why_not: Vec<WhyNot>,
    /// Exact manual steps, for the items whose rule has them (NVIDIA update artifacts).
    #[serde(skip)]
    pub manual: Option<ManualSteps>,
    /// The same steps in English, the command as its own element; absent for every other rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual_steps: Option<Vec<String>>,
}

/// The names of one hard-linked file inside (or beside) the item. Another name outside the
/// item keeps the data on disk: deleting the item frees nothing of that file.
fn check_names(
    path: &str,
    bytes_on_disk: u64,
    item_key: &str,
    seen: &mut std::collections::HashSet<String>,
    hard_links: &mut Vec<HardLinked>,
    outside_bytes: &mut u64,
) {
    let Some((links, names)) = platform::names(path) else { return };
    if links <= 1 {
        return;
    }
    let names_outside: Vec<String> = names
        .iter()
        .filter(|n| {
            let k = canon(n).to_lowercase();
            !(k == item_key || k.starts_with(&format!("{item_key}\\")))
        })
        .cloned()
        .collect();
    // Another name elsewhere keeps the data on disk: deleting the item frees nothing of it.
    // GUARD-BEGIN(expected-freed)
    if !names_outside.is_empty() || names.len() < links as usize {
        *outside_bytes += bytes_on_disk;
    }
    // GUARD-END(expected-freed)
    let key = names.iter().map(|n| canon(n).to_lowercase()).min().unwrap_or_default();
    if seen.insert(key) {
        hard_links.push(HardLinked { path: path.to_string(), bytes_on_disk, links, names, names_outside });
    }
}

impl Explanation {
    /// "Size on disk: ... in N files." (singular for one file).
    pub fn size_line(&self, lang: Lang) -> String {
        let size = crate::fmt::bytes(self.bytes_on_disk);
        if self.files == 1 {
            lang.fmt(Msg::ExplainSizeOne, &[("size", &size)])
        } else {
            lang.fmt(Msg::ExplainSize, &[("size", &size), ("files", &crate::fmt::count(self.files))])
        }
    }

    /// "Partial: N items could not be read ..." when the size is incomplete.
    pub fn partial_line(&self, lang: Lang) -> Option<String> {
        match self.errors {
            0 => None,
            1 => Some(lang.t(Msg::ExplainPartialOne).into()),
            n => Some(lang.fmt(Msg::ExplainPartial, &[("n", &crate::fmt::count(n))])),
        }
    }
}

/// Explains `path`. `data_dir` locates the local rules and the quarantine folder.
pub fn explain(path: &str, rules: &RuleSet, ctx: &Context, data_dir: Option<&str>) -> Explanation {
    let abs = std::path::absolute(crate::rules::native(&canon(path)))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string());
    let m = rules.find(&abs, ctx);
    let item = m.as_ref().map(|m| m.item.clone()).unwrap_or_else(|| canon(&abs));
    let item_native = crate::rules::native(&item);
    let meta = std::fs::symlink_metadata(&item_native);
    let exists = meta.is_ok();
    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
    let kind = match &meta {
        Err(_) => "missing",
        Ok(m) if m.file_type().is_symlink() => "link",
        Ok(m) if m.is_dir() => "folder",
        Ok(_) => "file",
    };
    let final_path = if exists { platform::final_path(&item_native) } else { None };

    // Size, and every file of at least 1 MB with its names. A file is measured from its own
    // handle (the scanners walk folders); a folder by a scan rooted at it.
    let mut bytes_on_disk = 0;
    let mut logical_bytes = 0;
    let mut files = 0;
    let mut errors = 0u64;
    let mut first_error_kind = None;
    let mut hard_links = Vec::new();
    let mut outside_bytes = 0u64;
    let mut large_files = Vec::new();
    // Windows lists a file's names in their long form: compare with the item's final path,
    // not with the text given (which may use short names such as RUNNER~1).
    let item_key = canon(final_path.as_deref().unwrap_or(&item)).to_lowercase();
    let mut seen = std::collections::HashSet::new();
    if exists && kind == "file" {
        match platform::file_facts(&item_native) {
            Ok(f) => {
                // GUARD-BEGIN(file-root-size)
                bytes_on_disk = f.allocation;
                // GUARD-END(file-root-size)
                logical_bytes = f.length;
                files = 1;
                let path = item_native.to_string_lossy().into_owned();
                if f.allocation >= DEDUP_THRESHOLD {
                    large_files.push(path.clone());
                }
                check_names(&path, f.allocation, &item_key, &mut seen, &mut hard_links, &mut outside_bytes);
            }
            Err(code) => {
                errors = 1;
                first_error_kind = Some(crate::scan::error_kind(code));
            }
        }
    } else if exists && kind != "link" {
        let report = scan(&ScanOptions { roots: vec![item_native.clone()], top_files: 0, collect_large: true, ..Default::default() }, None, None);
        bytes_on_disk = report.totals.bytes_on_disk;
        logical_bytes = report.totals.logical_bytes;
        files = report.totals.files;
        errors = report.totals.errors;
        first_error_kind = report.errors.first().map(|e| e.kind);
        large_files = report.large_files.iter().filter(|f| f.bytes_on_disk >= DEDUP_THRESHOLD).map(|f| f.path.clone()).collect();
        for f in report.large_files.iter().filter(|f| f.bytes_on_disk >= DEDUP_THRESHOLD) {
            check_names(&f.path, f.bytes_on_disk, &item_key, &mut seen, &mut hard_links, &mut outside_bytes);
        }
    }
    // Every part of the item was read.
    #[allow(unused_assignments)]
    let mut complete = true;
    // GUARD-BEGIN(explain-complete)
    complete = errors == 0;
    // GUARD-END(explain-complete)
    let expected_freed_bytes = bytes_on_disk.saturating_sub(outside_bytes);

    let checked = final_path.clone().unwrap_or_else(|| canon(&item));
    let act_scope = act::verdict(&checked, is_dir, ctx, data_dir);

    let mut why_not = Vec::new();
    let (rule, match_conditions, require_conditions, running_owners) = match &m {
        None => {
            why_not.push(WhyNot::NoRule);
            (None, Vec::new(), Vec::new(), Vec::new())
        }
        Some(m) => {
            let r = &m.rule;
            // GUARD-BEGIN(local-origin)
            if r.origin != Origin::BuiltIn {
                why_not.push(WhyNot::LocalRule);
            }
            // GUARD-END(local-origin)
            match r.action {
                Action::Quarantine => {}
                Action::ReportOnly => why_not.push(WhyNot::ReportOnly),
                Action::OpenAppSetting => why_not.push(WhyNot::AppManaged),
            }
            if r.class == Class::NeverTouch {
                why_not.push(WhyNot::NeverTouch);
            }
            for c in m.require_results.iter().filter(|c| !c.passed) {
                why_not.push(WhyNot::ConditionNotMet { condition: c.condition.clone(), found: c.found.clone() });
            }
            let running: Vec<String> = m
                .require_results
                .iter()
                .filter_map(|c| match &c.found {
                    Found::Running { names } => Some(names.clone()),
                    _ => None,
                })
                .flatten()
                .collect();
            let info = RuleInfo {
                id: r.id.clone(),
                title: r.title.clone(),
                class: r.class,
                action: r.action,
                origin: r.origin.clone(),
                evidence: r.evidence.clone(),
                source: r.source.clone(),
                cost: r.cost.clone(),
                instructions: r.instructions.clone(),
                last_verified: r.last_verified.clone(),
                full: r.clone(),
            };
            (Some(info), m.match_results.clone(), m.require_results.clone(), running)
        }
    };
    if !exists {
        why_not.push(WhyNot::Missing);
    }
    if kind == "link" {
        why_not.push(WhyNot::IsLink);
    }
    if act_scope.is_allowed_root {
        why_not.push(WhyNot::AllowedRootItself);
    } else if act_scope.allowed_root.is_none() {
        match &final_path {
            // GUARD-BEGIN(resolves-outside)
            Some(f) if canon(f).to_lowercase() != canon(&item).to_lowercase() => {
                why_not.push(WhyNot::ResolvesOutsideAllowedRoots { final_path: canon(f) })
            }
            // GUARD-END(resolves-outside)
            _ => why_not.push(WhyNot::OutsideAllowedRoots),
        }
    }
    if let Some(f) = &act_scope.forbidden {
        why_not.push(WhyNot::Forbidden { forbid: f.clone() });
    }
    let actionable = why_not.is_empty();
    let manual = rule.as_ref().and_then(|r| {
        let real = final_path.clone().unwrap_or_else(|| crate::rules::native(&item).to_string_lossy().into_owned());
        manual_steps(&r.full, &require_conditions, &real)
    });
    let manual_steps = manual.as_ref().map(|m| m.english());
    Explanation {
        path: path.to_string(),
        exists,
        kind,
        final_path,
        item: crate::rules::native(&item).to_string_lossy().into_owned(),
        bytes_on_disk,
        logical_bytes,
        files,
        errors,
        complete,
        first_error_kind,
        expected_freed_bytes,
        hard_links,
        large_files,
        unknown: rule.is_none(),
        rule,
        match_conditions,
        require_conditions,
        running_owners,
        act_scope,
        actionable,
        why_not,
        manual,
        manual_steps,
    }
}

/// The deepest folders of at least `min_bytes` that no rule knows (plan 4.1, "Discovery"):
/// a folder is listed when none of its subfolders reaches `min_bytes` by itself, so the list
/// points at where the space is, not at every parent of it.
pub fn unknown_large(report: &crate::scan::ScanReport, rules: &RuleSet, ctx: &Context, min_bytes: u64) -> Vec<(String, u64)> {
    let mut big_child = vec![false; report.dirs.len()];
    for d in &report.dirs {
        if d.parent != crate::scan::NO_PARENT && d.bytes >= min_bytes {
            big_child[d.parent as usize] = true;
        }
    }
    let mut out: Vec<(String, u64)> = report
        .dirs
        .iter()
        .enumerate()
        .filter(|(i, d)| d.bytes >= min_bytes && !big_child[*i])
        .map(|(i, d)| (report.dir_path(i as u32), d.bytes))
        .collect();
    // GUARD-BEGIN(unknown-flag)
    out.retain(|(p, _)| rules.covers(p, ctx).is_none());
    // GUARD-END(unknown-flag)
    out.sort_by_key(|x| std::cmp::Reverse(x.1));
    out
}

/// What a file says about itself.
struct FileFacts {
    allocation: u64,
    length: u64,
}

#[cfg(windows)]
mod platform {
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileStandardInfo, FindClose, FindFirstFileNameW, FindNextFileNameW,
        GetFileInformationByHandleEx, GetFinalPathNameByHandleW, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, OPEN_EXISTING, VOLUME_NAME_DOS,
    };

    use super::FileFacts;
    use crate::win::{display_path, verbatim, OwnedHandle};

    fn open(path: &Path) -> Option<OwnedHandle> {
        open_code(path).ok()
    }

    fn open_code(path: &Path) -> Result<OwnedHandle, u32> {
        let mut w = verbatim(path).map_err(|e| e.raw_os_error().unwrap_or(0) as u32)?;
        w.push(0);
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        OwnedHandle::new(h).ok_or_else(crate::win::last_error)
    }

    /// Allocation size, length and link count of a file, from `FILE_STANDARD_INFO`; the
    /// Windows error code when it cannot be read.
    pub fn file_facts(path: &Path) -> Result<FileFacts, u32> {
        let h = open_code(path)?;
        let mut info: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                h.raw(),
                FileStandardInfo,
                &mut info as *mut FILE_STANDARD_INFO as *mut core::ffi::c_void,
                std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
            )
        };
        if ok == 0 {
            return Err(crate::win::last_error());
        }
        Ok(FileFacts { allocation: info.AllocationSize as u64, length: info.EndOfFile as u64 })
    }

    /// The real path of the object itself (links not followed), from an open handle.
    pub fn final_path(path: &Path) -> Option<String> {
        let h = open(path)?;
        let mut buf = vec![0u16; 32_768];
        let n = unsafe {
            GetFinalPathNameByHandleW(h.raw(), buf.as_mut_ptr(), buf.len() as u32, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS)
        };
        if n == 0 || n as usize >= buf.len() {
            return None;
        }
        Some(display_path(&buf[..n as usize]))
    }

    /// Link count read from the file, and every name of it (`FindFirstFileNameW`).
    pub fn names(path: &str) -> Option<(u32, Vec<String>)> {
        let h = open(Path::new(path))?;
        let mut info: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                h.raw(),
                FileStandardInfo,
                &mut info as *mut FILE_STANDARD_INFO as *mut core::ffi::c_void,
                std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
            )
        };
        if ok == 0 {
            return None;
        }
        let links = info.NumberOfLinks;
        if links <= 1 {
            return Some((links, vec![path.to_string()]));
        }
        // Names come back relative to the volume root ("\Users\..."): prefix the drive.
        let drive: String = path.chars().take(2).collect();
        let mut w = verbatim(Path::new(path)).ok()?;
        w.push(0);
        let mut names = Vec::new();
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        let find = unsafe { FindFirstFileNameW(w.as_ptr(), 0, &mut len, buf.as_mut_ptr()) };
        if find as isize == -1 {
            return Some((links, names));
        }
        loop {
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            names.push(format!("{drive}{}", String::from_utf16_lossy(&buf[..n])));
            len = buf.len() as u32;
            if unsafe { FindNextFileNameW(find, &mut len, buf.as_mut_ptr()) } == 0 {
                break;
            }
        }
        unsafe { FindClose(find) };
        Some((links, names))
    }
}

#[cfg(not(windows))]
mod platform {
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    use super::FileFacts;

    pub fn final_path(path: &Path) -> Option<String> {
        let parent = std::fs::canonicalize(path.parent()?).ok()?;
        Some(parent.join(path.file_name()?).to_string_lossy().into_owned())
    }

    /// Allocation (blocks) and length of a file; the OS error number when it cannot be read.
    pub fn file_facts(path: &Path) -> Result<FileFacts, u32> {
        let m = std::fs::symlink_metadata(path).map_err(|e| e.raw_os_error().unwrap_or(0) as u32)?;
        Ok(FileFacts { allocation: m.blocks() * 512, length: m.len() })
    }

    /// POSIX cannot list a file's other names: they are reported as unknown (outside).
    pub fn names(path: &str) -> Option<(u32, Vec<String>)> {
        let m = std::fs::symlink_metadata(path).ok()?;
        Some((m.nlink() as u32, vec![path.to_string()]))
    }
}
