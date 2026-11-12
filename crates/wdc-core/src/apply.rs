//! `apply` (plan, sections 5.1, 5.2 and 5.4): moves the items of a plan into quarantine.
//!
//! It trusts nothing in the plan file. For every item it opens the item once without
//! following links, checks on that handle its real final path against the allowed and
//! forbidden roots and its file ID against the plan, re-runs the matching against the built-in
//! rules on the real path and re-checks every condition now, and only then renames it into
//! quarantine through the same handle. Items that changed, no longer match or fail a condition
//! are skipped and reported. Free space is read on each affected volume before and after.

use std::path::Path;

use serde::Serialize;

use crate::act;
use crate::explain::WhyNot;
use crate::fsops::{self, Holder, HolderProbe, OpError};
use crate::plan::{Plan, PlanItem, PlanRefusal};
use crate::presence::Proof;
use crate::quarantine::{self, Entry, PurgeReport, QuarantineError, VolumeMeasure};
use crate::rules::{canon, native, Action, Class, Condition, Context, Found, Origin, RuleSet};

/// Why an item was not moved.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum Skip {
    Missing,
    /// The item is a link now (junction, symbolic link, mount point): nothing is moved.
    IsLink,
    /// Its real path is outside the allowed roots, an allowed root itself, or forbidden.
    NotAllowed { final_path: String, why: WhyNot },
    /// It is not the item that was planned (another file ID), or a large file inside changed.
    Changed { what: String },
    /// No built-in rule matches its real path as an item now.
    NoLongerMatches { final_path: String },
    /// Another rule matches it now.
    RuleChanged { now: String },
    /// The rule that matches cannot act (report-only, app-managed, local).
    NotActionable { why: WhyNot },
    ConditionFailed { condition: Condition, found: Found },
    Quarantine { error: QuarantineError },
    /// The item could not be opened (before anything was decided about it).
    OpenFailed { error: OpError },
    /// The item was opened but the rename into quarantine failed.
    RenameFailed { error: OpError },
    /// Another program has the item, or a file inside it, open. `stage` says where Windows
    /// refused: `open` (the item) or `rename` (a file under a folder). Nothing was forced.
    HeldOpen { stage: &'static str, error: OpError, holders: Vec<Holder> },
}

impl Skip {
    pub fn text(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Msg;
        match self {
            Skip::Missing => lang.t(Msg::WhyMissing).into(),
            Skip::IsLink => lang.t(Msg::WhyIsLink).into(),
            Skip::NotAllowed { final_path, why } => {
                lang.fmt(Msg::ApplyNotAllowed, &[("path", final_path), ("why", &why.text(lang))])
            }
            Skip::Changed { what } => lang.fmt(Msg::ApplyChanged, &[("what", what)]),
            Skip::NoLongerMatches { final_path } => lang.fmt(Msg::ApplyNoMatch, &[("path", final_path)]),
            Skip::RuleChanged { now } => lang.fmt(Msg::ApplyRuleChanged, &[("rule", now)]),
            Skip::NotActionable { why } => why.text(lang),
            Skip::ConditionFailed { found, .. } => lang.fmt(Msg::WhyCondition, &[("found", &found.text(lang))]),
            Skip::Quarantine { error } => lang.fmt(Msg::ApplyQuarantineUnsafe, &[("detail", &format!("{error:?}"))]),
            Skip::OpenFailed { error } => lang.fmt(Msg::ApplyOpenFailed, &[("error", error.kind)]),
            Skip::RenameFailed { error } => lang.fmt(Msg::ApplyRenameFailed, &[("error", error.kind)]),
            Skip::HeldOpen { holders, .. } => {
                let msg = if fsops::holder_name_count(holders) > 1 { Msg::ApplyHeldOpenMany } else { Msg::ApplyHeldOpen };
                lang.fmt(msg, &[("holders", &fsops::holder_names(holders))])
            }
        }
    }
}

/// What happened to one item.
#[derive(Clone, Debug, Serialize)]
pub struct ItemOutcome {
    pub path: String,
    pub rule_id: String,
    pub expected_freed_bytes: u64,
    pub moved: Option<Entry>,
    pub skipped: Option<Skip>,
}

#[derive(Clone, Debug, Default)]
pub struct ApplyOptions {
    /// Purge what was moved right away, where early purge allows it.
    pub purge_now: bool,
    /// The face showed each item's explanation to the person who approved.
    pub explained: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ApplyReport {
    pub plan_hash: String,
    pub items: Vec<ItemOutcome>,
    pub moved: usize,
    pub skipped: usize,
    pub moved_bytes: u64,
    /// Size on disk of the items that were not moved.
    pub skipped_bytes: u64,
    /// Size on disk of what `purge_now` deleted for good; absent when nothing was purged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purged_bytes: Option<u64>,
    /// Expected freed by a later purge of what was moved.
    pub expected_freed_bytes: u64,
    /// Free space around the moves (a rename frees nothing).
    pub volumes: Vec<VolumeMeasure>,
    pub purge: Option<PurgeReport>,
    pub quarantine_held_bytes: u64,
    pub proof: Proof,
}

/// The whole plan is refused.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Refused(pub PlanRefusal);

fn lower(p: &str) -> String {
    canon(p).to_lowercase()
}

fn why_not_allowed(v: &act::Verdict) -> Option<WhyNot> {
    if let Some(f) = &v.forbidden {
        return Some(WhyNot::Forbidden { forbid: f.clone() });
    }
    if v.is_allowed_root {
        return Some(WhyNot::AllowedRootItself);
    }
    if v.allowed_root.is_none() {
        return Some(WhyNot::OutsideAllowedRoots);
    }
    None
}

/// What a refusal by Windows means for `path`: held open when the error is access denied or a
/// sharing violation and the probe names who holds it; otherwise the plain failure. Nothing is
/// closed, signalled or forced either way.
#[allow(unused_mut, unused_assignments)]
fn refused(path: &Path, stage: &'static str, error: OpError, probe: &HolderProbe) -> Skip {
    if error.may_be_held() {
        let mut holders = Vec::new();
        // GUARD-BEGIN(apply-held)
        holders = probe.probe(path).unwrap_or_default();
        // GUARD-END(apply-held)
        if !holders.is_empty() {
            return Skip::HeldOpen { stage, error, holders };
        }
    }
    if stage == "open" {
        Skip::OpenFailed { error }
    } else {
        Skip::RenameFailed { error }
    }
}

/// Checks one item on its open handle. Returns the open item when it may be moved.
#[allow(unused_mut, unused_assignments)]
fn check(item: &PlanItem, rules: &RuleSet, ctx: &Context, data_dir: &str, probe: &HolderProbe) -> Result<(fsops::OpenItem, Class), Skip> {
    let open = match fsops::open_item(&native(&item.path)) {
        Ok(o) => o,
        Err(e) if e.kind == "not-found" => return Err(Skip::Missing),
        Err(e) => return Err(refused(&native(&item.path), "open", e, probe)),
    };
    // GUARD-BEGIN(apply-link)
    if open.is_link {
        return Err(Skip::IsLink);
    }
    // GUARD-END(apply-link)
    let final_path = open.final_path.clone();
    let verdict = act::verdict(&final_path, open.is_dir, ctx, Some(data_dir));
    let mut refused = None;
    // GUARD-BEGIN(apply-verdict)
    refused = why_not_allowed(&verdict);
    // GUARD-END(apply-verdict)
    if let Some(why) = refused {
        return Err(Skip::NotAllowed { final_path, why });
    }
    // GUARD-BEGIN(apply-file-id)
    if open.file_id != item.file_id {
        return Err(Skip::Changed { what: "item".into() });
    }
    // GUARD-END(apply-file-id)
    for f in &item.large_files {
        let p = native(&format!("{}\\{}", canon(&final_path), f.path));
        if fsops::file_id(&p).as_deref() != Some(f.file_id.as_str()) {
            return Err(Skip::Changed { what: f.path.clone() });
        }
    }
    // The rules decide again, on the real path, now.
    let m = rules.find(&final_path, ctx);
    let mut matches = true;
    // GUARD-BEGIN(apply-rematch)
    matches = m.as_ref().is_some_and(|m| !m.inside && lower(&m.item) == lower(&final_path));
    // GUARD-END(apply-rematch)
    let Some(m) = m.filter(|_| matches) else {
        return Err(Skip::NoLongerMatches { final_path });
    };
    if m.rule.id != item.rule_id {
        return Err(Skip::RuleChanged { now: m.rule.id.clone() });
    }
    if m.rule.origin != Origin::BuiltIn {
        return Err(Skip::NotActionable { why: WhyNot::LocalRule });
    }
    match m.rule.action {
        Action::Quarantine => {}
        Action::ReportOnly => return Err(Skip::NotActionable { why: WhyNot::ReportOnly }),
        Action::OpenAppSetting => return Err(Skip::NotActionable { why: WhyNot::AppManaged }),
    }
    if m.rule.class == Class::NeverTouch {
        return Err(Skip::NotActionable { why: WhyNot::NeverTouch });
    }
    let mut failed: Option<crate::rules::CondResult> = None;
    // GUARD-BEGIN(apply-conditions)
    failed = m.require_results.iter().find(|c| !c.passed).cloned();
    // GUARD-END(apply-conditions)
    if let Some(c) = failed {
        return Err(Skip::ConditionFailed { condition: c.condition, found: c.found });
    }
    Ok((open, m.rule.class))
}

/// Applies a plan. `rules` must be the built-in rules (local rules can never act anyway).
pub fn apply(
    plan: &Plan,
    rules: &RuleSet,
    ctx: &Context,
    data_dir: &Path,
    opts: &ApplyOptions,
    proof: &Proof,
    now_secs: u64,
) -> Result<ApplyReport, Refused> {
    if let Some(r) = plan.refusal(now_secs) {
        return Err(Refused(r));
    }
    let _ = std::fs::create_dir_all(data_dir);
    let data = data_dir.to_string_lossy().into_owned();
    let probe = HolderProbe::default();
    let measure: Vec<(String, u64)> = plan.items.iter().map(|i| (i.path.clone(), 0)).collect();
    let (items, volumes) = quarantine::measured(&measure, || {
        let mut out = Vec::new();
        let mut entries = Vec::new();
        for (n, item) in plan.items.iter().enumerate() {
            let result = check(item, rules, ctx, &data, &probe).and_then(|(open, class)| {
                let base = quarantine::base_for(&open.final_path, data_dir);
                let q = quarantine::prepare(&base, plan.short_hash()).map_err(|error| Skip::Quarantine { error })?;
                let target = quarantine::free_slot(&q, n);
                fsops::rename(&open, &target).map_err(|error| refused(Path::new(&open.final_path), "rename", error, &probe))?;
                let stored = fsops::final_path(&target).unwrap_or_else(|| target.to_string_lossy().into_owned());
                Ok(Entry {
                    id: format!("{}-{}", &plan.hash[..8], target.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default()),
                    plan_hash: plan.hash.clone(),
                    original: open.final_path.clone(),
                    stored,
                    file_id: open.file_id.clone(),
                    is_dir: open.is_dir,
                    rule_id: item.rule_id.clone(),
                    class,
                    bytes_on_disk: item.bytes_on_disk,
                    expected_freed_bytes: item.expected_freed_bytes,
                    moved_at: now_secs,
                    explained: opts.explained,
                })
            });
            match result {
                Ok(e) => {
                    entries.push(e.clone());
                    out.push(ItemOutcome {
                        path: item.path.clone(),
                        rule_id: item.rule_id.clone(),
                        expected_freed_bytes: item.expected_freed_bytes,
                        moved: Some(e),
                        skipped: None,
                    });
                }
                Err(s) => out.push(ItemOutcome {
                    path: item.path.clone(),
                    rule_id: item.rule_id.clone(),
                    expected_freed_bytes: item.expected_freed_bytes,
                    moved: None,
                    skipped: Some(s),
                }),
            }
        }
        let _ = quarantine::record(data_dir, &entries);
        out
    });
    let moved: Vec<&Entry> = items.iter().filter_map(|i| i.moved.as_ref()).collect();
    let moved_ids: Vec<String> = moved.iter().map(|e| e.id.clone()).collect();
    let moved_bytes = moved.iter().map(|e| e.bytes_on_disk).sum();
    let skipped_bytes = plan.items.iter().zip(&items).filter(|(_, o)| o.moved.is_none()).map(|(i, _)| i.bytes_on_disk).sum();
    let expected_freed_bytes = moved.iter().map(|e| e.expected_freed_bytes).sum();
    let purge = (opts.purge_now && !moved_ids.is_empty())
        .then(|| quarantine::purge(data_dir, Some(&moved_ids), true, now_secs, proof));
    let report = ApplyReport {
        plan_hash: plan.hash.clone(),
        moved: moved_ids.len(),
        skipped: items.len() - moved_ids.len(),
        items,
        moved_bytes,
        skipped_bytes,
        purged_bytes: purge.as_ref().map(|p| p.purged_bytes),
        expected_freed_bytes,
        volumes,
        purge,
        quarantine_held_bytes: quarantine::held_bytes(data_dir),
        proof: proof.clone(),
    };
    log(data_dir, "apply", &report);
    Ok(report)
}

/// Appends a report to `<data>\changes.jsonl`: every change the tool made, with measurements.
pub fn log<T: Serialize>(data_dir: &Path, kind: &str, report: &T) {
    use std::io::Write;
    let line = serde_json::json!({ "at": crate::time::rfc3339(crate::time::now_unix_ms()), "kind": kind, "report": report });
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(data_dir.join("changes.jsonl")) {
        let _ = writeln!(f, "{line}");
    }
}
