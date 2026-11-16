//! Plans (plan, section 5.1): a list of choices, not a list of authorisations.
//!
//! A plan names each item with its path, the file ID of the item itself, the file IDs of the
//! files of 1 MB or more inside it, the rule, the class, the size on disk, the bytes expected to
//! be freed and when it was measured. `apply` trusts none of it: it re-checks everything on the
//! open item. Drafting a plan changes nothing on disk except the plan file itself.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::explain::{explain, WhyNot};
use crate::fsops::{Holder, HolderProbe};
use crate::rules::{canon, glob, native, Action, Class, Context, Origin, RuleSet};

/// Version of the plan file format.
pub const FORMAT: u32 = 1;
/// A plan older than this is refused as a whole.
pub const MAX_AGE_SECS: u64 = 24 * 3600;
/// Clock skew tolerated for a plan dated in the future.
pub const FUTURE_SKEW_SECS: u64 = 300;

/// A file of 1 MB or more inside an item, by its path relative to the item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LargeFile {
    pub path: String,
    pub file_id: String,
}

/// One item of a plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanItem {
    pub path: String,
    pub file_id: String,
    pub is_dir: bool,
    pub large_files: Vec<LargeFile>,
    pub rule_id: String,
    /// The rule's English title, for people reading the file.
    pub rule_title: String,
    pub class: Class,
    pub bytes_on_disk: u64,
    pub expected_freed_bytes: u64,
    pub files: u64,
    /// When the item was measured (Unix seconds).
    pub measured_at: u64,
}

/// A plan file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub format: u32,
    /// Unix seconds.
    pub created_at: u64,
    pub tool_version: String,
    pub items: Vec<PlanItem>,
    /// SHA-256 of everything above, hex.
    pub hash: String,
}

#[derive(Serialize)]
struct Hashed<'a> {
    format: u32,
    created_at: u64,
    tool_version: &'a str,
    items: &'a [PlanItem],
}

impl Plan {
    pub fn new(items: Vec<PlanItem>, created_at: u64) -> Plan {
        let mut p = Plan { format: FORMAT, created_at, tool_version: crate::VERSION.into(), items, hash: String::new() };
        p.seal();
        p
    }

    fn compute_hash(&self) -> String {
        let h = Hashed { format: self.format, created_at: self.created_at, tool_version: &self.tool_version, items: &self.items };
        let bytes = serde_json::to_vec(&h).expect("plan serializes");
        Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Sets the hash from the content.
    pub fn seal(&mut self) {
        self.hash = self.compute_hash();
    }

    /// The content matches the hash (the file was not edited after it was written).
    pub fn hash_matches(&self) -> bool {
        self.hash == self.compute_hash()
    }

    /// The first 16 hex digits of the hash: names the plan's quarantine folder and file.
    pub fn short_hash(&self) -> &str {
        &self.hash[..self.hash.len().min(16)]
    }

    pub fn expected_freed_bytes(&self) -> u64 {
        self.items.iter().map(|i| i.expected_freed_bytes).sum()
    }

    pub fn bytes_on_disk(&self) -> u64 {
        self.items.iter().map(|i| i.bytes_on_disk).sum()
    }

    /// Why the plan as a whole cannot be applied now, if it cannot.
    pub fn refusal(&self, now_secs: u64) -> Option<PlanRefusal> {
        if self.format != FORMAT {
            return Some(PlanRefusal::UnknownFormat { format: self.format });
        }
        if !self.hash_matches() {
            return Some(PlanRefusal::Edited);
        }
        // GUARD-BEGIN(plan-age)
        if now_secs > self.created_at + MAX_AGE_SECS {
            return Some(PlanRefusal::TooOld { hours: (now_secs - self.created_at) / 3600 });
        }
        // GUARD-END(plan-age)
        if self.created_at > now_secs + FUTURE_SKEW_SECS {
            return Some(PlanRefusal::FromTheFuture);
        }
        None
    }

    pub fn load(path: &Path) -> Result<Plan, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Writes the plan to `path` (the folder must exist).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).expect("plan serializes"))?;
        std::fs::rename(&tmp, path)
    }

    /// Saves the plan in the data folder (`plans\<short-hash>.json`) and returns the path.
    pub fn save_in(&self, data_dir: &Path) -> std::io::Result<PathBuf> {
        let dir = data_dir.join("plans");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", self.short_hash()));
        self.save(&path)?;
        Ok(path)
    }

    /// A plan saved in the data folder, by its hash or the start of it (at least 8 digits).
    pub fn find_in(data_dir: &Path, hash: &str) -> Result<Plan, String> {
        let h = hash.trim().to_ascii_lowercase();
        if h.len() < 8 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("not a plan hash: {hash}"));
        }
        let dir = data_dir.join("plans");
        let rd = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if name.ends_with(".json") && (name.starts_with(&h) || h.starts_with(name.trim_end_matches(".json"))) {
                let plan = Plan::load(&e.path())?;
                if plan.hash.starts_with(&h) {
                    return Ok(plan);
                }
            }
        }
        Err(format!("no saved plan with hash {hash}"))
    }
}

/// Why a whole plan is refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum PlanRefusal {
    UnknownFormat { format: u32 },
    /// The content does not match the hash: draft the plan again.
    Edited,
    TooOld { hours: u64 },
    FromTheFuture,
}

impl PlanRefusal {
    pub fn text(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Msg;
        match self {
            PlanRefusal::UnknownFormat { format } => lang.fmt(Msg::PlanUnknownFormat, &[("format", &format.to_string())]),
            PlanRefusal::Edited => lang.t(Msg::PlanEdited).into(),
            PlanRefusal::TooOld { hours } => lang.fmt(Msg::PlanTooOld, &[("hours", &hours.to_string())]),
            PlanRefusal::FromTheFuture => lang.t(Msg::PlanFromFuture).into(),
        }
    }
}

/// What to draft.
#[derive(Clone, Debug, Default)]
pub struct DraftOptions {
    /// Explicit paths; empty means every place a built-in rule that can act knows.
    pub paths: Vec<String>,
    /// Only these classes (empty: all).
    pub classes: Vec<Class>,
    /// Only these rule ids (empty: all).
    pub rules: Vec<String>,
    /// Finds who holds an item open: the Restart Manager on Windows, nobody elsewhere. A test
    /// passes its own.
    pub holders: HolderProbe,
}

/// A candidate that is not in the plan, with why.
#[derive(Clone, Debug, Serialize)]
pub struct NotIncluded {
    pub path: String,
    pub rule_id: Option<String>,
    pub why_not: Vec<WhyNot>,
}

/// A drafted plan and the candidates left out.
#[derive(Clone, Debug, Serialize)]
pub struct Draft {
    pub plan: Plan,
    pub not_included: Vec<NotIncluded>,
}

/// Every existing path matching an expanded pattern (`*` and `?` inside one name; a final
/// `**` stands for the item before it). Links are listed, never entered.
pub fn expand_pattern(pattern: &str) -> Vec<String> {
    let p = canon(pattern);
    let segs: Vec<&str> = p.split('\\').collect();
    let mut current: Vec<String> = vec![segs[0].to_string()];
    for s in &segs[1..] {
        if s.is_empty() {
            continue;
        }
        if *s == "**" {
            break;
        }
        let mut next = Vec::new();
        for c in &current {
            if s.contains('*') || s.contains('?') {
                let dir = if c.is_empty() || c.ends_with(':') { format!("{c}\\") } else { c.clone() };
                let Ok(rd) = std::fs::read_dir(native(&dir)) else { continue };
                let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
                names.sort();
                for n in names.into_iter().filter(|n| glob(s, n)) {
                    next.push(format!("{c}\\{n}"));
                }
            } else {
                next.push(format!("{c}\\{s}"));
            }
        }
        current = next;
    }
    current.retain(|c| std::fs::symlink_metadata(native(c)).is_ok());
    current
}

fn rel(item: &str, path: &str) -> String {
    let item = canon(item);
    let p = canon(path);
    if p.len() > item.len() && p.get(..item.len()).is_some_and(|x| x.eq_ignore_ascii_case(&item)) {
        p[item.len()..].trim_start_matches('\\').to_string()
    } else {
        p
    }
}

/// Why an actionable item is left out of the plan for its size, if it is. The incomplete reason
/// is checked first: an item that could not be fully read has an unknown size, and "frees nothing"
/// would claim a size that is not known.
#[allow(unused_variables)]
pub fn left_out_for_size(complete: bool, expected_freed_bytes: u64) -> Option<WhyNot> {
    // An incomplete size is not a size the person can approve.
    // GUARD-BEGIN(plan-complete)
    if !complete {
        return Some(WhyNot::NotFullyReadable);
    }
    // GUARD-END(plan-complete)
    // An item that frees nothing still costs a proof of presence and quarantine churn.
    // GUARD-BEGIN(plan-nothing)
    if expected_freed_bytes == 0 {
        return Some(WhyNot::NothingToFree);
    }
    // GUARD-END(plan-nothing)
    None
}

/// Drafts a plan: every eligible item, explained. Reads only; writes nothing.
pub fn draft(opts: &DraftOptions, rules: &RuleSet, ctx: &Context, data_dir: Option<&str>, now_secs: u64) -> Draft {
    let mut candidates: Vec<String> = Vec::new();
    if opts.paths.is_empty() {
        for r in rules.rules.iter().filter(|r| r.action == Action::Quarantine && r.origin == Origin::BuiltIn) {
            for pattern in &r.paths {
                if let Some(p) = ctx.expand(pattern) {
                    candidates.extend(expand_pattern(&p));
                }
            }
        }
    } else {
        candidates.extend(opts.paths.iter().cloned());
    }
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    let mut not_included = Vec::new();
    for c in candidates {
        let abs = std::path::absolute(native(&canon(&c))).map(|p| p.to_string_lossy().into_owned()).unwrap_or(c.clone());
        let m = rules.find(&abs, ctx);
        let key = canon(m.as_ref().map(|m| m.item.as_str()).unwrap_or(&abs)).to_lowercase();
        if !seen.insert(key) {
            continue;
        }
        if let Some(m) = &m {
            if (!opts.classes.is_empty() && !opts.classes.contains(&m.rule.class))
                || (!opts.rules.is_empty() && !opts.rules.iter().any(|r| r.eq_ignore_ascii_case(&m.rule.id)))
            {
                continue;
            }
            // Report-only and app-managed items are not measured: they cannot be in a plan.
            if m.rule.action != Action::Quarantine || m.rule.origin != Origin::BuiltIn {
                let why = if m.rule.origin != Origin::BuiltIn {
                    WhyNot::LocalRule
                } else if m.rule.action == Action::ReportOnly {
                    WhyNot::ReportOnly
                } else {
                    WhyNot::AppManaged
                };
                not_included.push(NotIncluded { path: m.item.clone(), rule_id: Some(m.rule.id.clone()), why_not: vec![why] });
                continue;
            }
        } else if !opts.classes.is_empty() || !opts.rules.is_empty() {
            continue;
        }
        let e = explain(&abs, rules, ctx, data_dir);
        let rule_id = e.rule.as_ref().map(|r| r.id.clone());
        if !e.actionable {
            not_included.push(NotIncluded { path: e.item.clone(), rule_id, why_not: e.why_not.clone() });
            continue;
        }
        if let Some(why) = left_out_for_size(e.complete, e.expected_freed_bytes) {
            not_included.push(NotIncluded { path: e.item.clone(), rule_id, why_not: vec![why] });
            continue;
        }
        let r = e.rule.as_ref().expect("an actionable item has a rule");
        let item_path = e.final_path.clone().unwrap_or_else(|| e.item.clone());
        // Renaming a folder fails while a file under it is open: leave out what another program
        // holds. A probe that fails says nothing and keeps the item (apply still fails closed).
        #[allow(unused_mut, unused_assignments)]
        let mut held: Vec<Holder> = Vec::new();
        // GUARD-BEGIN(plan-held)
        held = opts.holders.probe(&native(&item_path)).unwrap_or_default();
        // GUARD-END(plan-held)
        if !held.is_empty() {
            not_included.push(NotIncluded { path: e.item.clone(), rule_id, why_not: vec![WhyNot::HeldOpen { holders: held }] });
            continue;
        }
        let Some(file_id) = crate::fsops::file_id(&native(&item_path)) else {
            not_included.push(NotIncluded { path: e.item.clone(), rule_id, why_not: vec![WhyNot::Missing] });
            continue;
        };
        let large_files = e
            .large_files
            .iter()
            .filter_map(|f| {
                // The scan lists files under the item as it was given (perhaps with short names);
                // the plan keeps them relative to the item.
                let r = rel(&e.item, f);
                let r = if r == canon(f) { rel(&item_path, f) } else { r };
                if r == canon(f) {
                    return None;
                }
                crate::fsops::file_id(&native(f)).map(|id| LargeFile { path: r, file_id: id })
            })
            .collect();
        items.push(PlanItem {
            path: item_path,
            file_id,
            is_dir: e.kind == "folder",
            large_files,
            rule_id: r.id.clone(),
            rule_title: r.title.clone(),
            class: r.class,
            bytes_on_disk: e.bytes_on_disk,
            expected_freed_bytes: e.expected_freed_bytes,
            files: e.files,
            measured_at: now_secs,
        });
    }
    Draft { plan: Plan::new(items, now_secs), not_included }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str) -> PlanItem {
        PlanItem {
            path: path.into(),
            file_id: "1:2".into(),
            is_dir: true,
            large_files: vec![],
            rule_id: "vscode-caches".into(),
            rule_title: "VS Code caches".into(),
            class: Class::RegenerableCache,
            bytes_on_disk: 10,
            expected_freed_bytes: 10,
            files: 1,
            measured_at: 1000,
        }
    }

    #[test]
    fn the_hash_covers_every_item_and_old_plans_are_refused() {
        let p = Plan::new(vec![item(r"C:\a")], 1_000_000);
        assert!(p.hash_matches());
        assert_eq!(p.hash.len(), 64);
        assert_eq!(p.refusal(1_000_000 + 3600), None);
        let mut edited = p.clone();
        edited.items[0].path = r"C:\Windows".into();
        assert_eq!(edited.refusal(1_000_000), Some(PlanRefusal::Edited));
        assert_eq!(p.refusal(1_000_000 + MAX_AGE_SECS + 1), Some(PlanRefusal::TooOld { hours: 24 }));
        assert_eq!(p.refusal(1_000_000 - FUTURE_SKEW_SECS - 1), Some(PlanRefusal::FromTheFuture));
    }

    #[test]
    fn relative_paths_of_large_files() {
        assert_eq!(rel(r"C:\a\b", r"C:\A\B\c\d.bin"), r"c\d.bin");
    }
}
