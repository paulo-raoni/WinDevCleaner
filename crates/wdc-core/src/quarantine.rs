//! Quarantine (plan, section 5.2): `apply` renames items here; `restore` puts them back;
//! `purge` deletes them for good.
//!
//! - On the volume of the data folder: `<data>\quarantine\<plan-hash>\q\<n>`; on other volumes
//!   `<drive>:\.wdc-quarantine\<plan-hash>\q\<n>` (a rename cannot cross volumes).
//! - Every quarantine folder is created by the tool, then checked on an open handle: it is
//!   refused if it is a link or its real path is not the expected one (a link above it).
//! - Quarantine frees no space by itself. `purge` deletes items older than 7 days; earlier only
//!   for `regenerable-cache` and `install-leftover` items whose explanation was shown when they
//!   were approved, by the same rule for `apply --purge-now` and `purge --now`.
//! - The index of what is held lives in `<data>\quarantine\index.json`; every check that
//!   matters is made on disk, not trusted from the index.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsops;
use crate::rules::{canon, native, Class, Context, RuleSet};

/// Items stay this long before a normal purge.
pub const PURGE_AFTER_SECS: u64 = 7 * 24 * 3600;
/// Name of the quarantine folder at the root of volumes other than the data folder's.
pub const VOLUME_FOLDER: &str = ".wdc-quarantine";

/// One quarantined item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// `<first 8 digits of the plan hash>-<n>`.
    pub id: String,
    pub plan_hash: String,
    /// Where it was (its real path when it was moved).
    pub original: String,
    /// Where it is now.
    pub stored: String,
    pub file_id: String,
    pub is_dir: bool,
    pub rule_id: String,
    pub class: Class,
    pub bytes_on_disk: u64,
    pub expected_freed_bytes: u64,
    /// Unix seconds.
    pub moved_at: u64,
    /// Its explanation was shown to the person who approved.
    pub explained: bool,
}

fn index_path(data_dir: &Path) -> PathBuf {
    data_dir.join("quarantine").join("index.json")
}

/// What the quarantine holds now.
pub fn list(data_dir: &Path) -> Vec<Entry> {
    std::fs::read_to_string(index_path(data_dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(data_dir: &Path, entries: &[Entry]) -> std::io::Result<()> {
    let path = index_path(data_dir);
    std::fs::create_dir_all(path.parent().expect("index has a folder"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(entries).expect("index serializes"))?;
    std::fs::rename(&tmp, &path)
}

/// Adds entries to the index.
pub fn record(data_dir: &Path, new: &[Entry]) -> std::io::Result<()> {
    let mut all = list(data_dir);
    all.extend_from_slice(new);
    save(data_dir, &all)
}

fn forget(data_dir: &Path, ids: &[String]) -> std::io::Result<()> {
    let all: Vec<Entry> = list(data_dir).into_iter().filter(|e| !ids.contains(&e.id)).collect();
    save(data_dir, &all)
}

/// Bytes on disk held in quarantine now.
pub fn held_bytes(data_dir: &Path) -> u64 {
    list(data_dir).iter().map(|e| e.bytes_on_disk).sum()
}

fn lower(p: &str) -> String {
    canon(p).to_lowercase()
}

fn same_volume(a: &str, b: &str) -> bool {
    match (fsops::volume_root(&native(a)), fsops::volume_root(&native(b))) {
        (Some(x), Some(y)) => lower(&x) == lower(&y),
        _ => false,
    }
}

/// Why a quarantine folder could not be prepared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum QuarantineError {
    /// It (or a folder above it) is a link, or resolves somewhere else.
    NotPlain { path: String, resolves_to: String },
    Create { path: String, error: String },
}

/// The base quarantine folder for an item on the volume of `item_path`.
pub fn base_for(item_path: &str, data_dir: &Path) -> PathBuf {
    let data = data_dir.to_string_lossy();
    if same_volume(item_path, &data) {
        data_dir.join("quarantine")
    } else {
        let root = fsops::volume_root(&native(item_path)).unwrap_or_default();
        native(&root).join(VOLUME_FOLDER)
    }
}

/// Creates `<base>\<short-hash>\q` and checks it on an open handle.
#[allow(unused_mut, unused_assignments)]
pub fn prepare(base: &Path, short_hash: &str) -> Result<PathBuf, QuarantineError> {
    let dir = base.join(short_hash).join("q");
    std::fs::create_dir_all(&dir).map_err(|e| QuarantineError::Create { path: dir.display().to_string(), error: e.to_string() })?;
    // Every folder the tool created, from the base down, must be a plain folder whose real
    // path is the one expected below the real path of the folder holding the base.
    let holder = base.parent().unwrap_or(base);
    let mut want = std::fs::canonicalize(holder)
        .map(|r| canon(r.to_string_lossy().trim_start_matches(r"\\?\")))
        .map_err(|e| QuarantineError::Create { path: holder.display().to_string(), error: e.to_string() })?;
    let mut p = holder.to_path_buf();
    let base_name = base.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for name in [base_name.as_str(), short_hash, "q"] {
        p = p.join(name);
        want = format!("{want}\\{name}");
        let ok = match fsops::open_item(&p) {
            Ok(item) => {
                let mut plain = item.is_dir;
                // GUARD-BEGIN(quarantine-plain)
                plain = plain && !item.is_link && lower(&item.final_path) == lower(&want);
                // GUARD-END(quarantine-plain)
                plain
            }
            Err(_) => false,
        };
        if !ok {
            let resolves_to = fsops::final_path(&p).unwrap_or_default();
            return Err(QuarantineError::NotPlain { path: p.display().to_string(), resolves_to });
        }
    }
    Ok(dir)
}

/// A free slot `q\<n>` in a prepared folder.
pub fn free_slot(q: &Path, start: usize) -> PathBuf {
    let mut n = start;
    loop {
        let p = q.join(n.to_string());
        if std::fs::symlink_metadata(&p).is_err() {
            return p;
        }
        n += 1;
    }
}

/// Whether `path` (a real path) is inside a quarantine folder.
pub fn is_inside_quarantine(path: &str, data_dir: &Path) -> bool {
    let p = lower(path);
    let data_q = lower(&format!("{}\\quarantine", canon(&data_dir.to_string_lossy())));
    let real_q = std::fs::canonicalize(data_dir.join("quarantine"))
        .map(|r| lower(r.to_string_lossy().trim_start_matches(r"\\?\")))
        .unwrap_or_default();
    let under = |b: &str| !b.is_empty() && p.starts_with(&format!("{b}\\"));
    if under(&data_q) || under(&real_q) {
        return true;
    }
    // `<drive>:\.wdc-quarantine\...`
    let segs: Vec<&str> = p.split('\\').filter(|s| !s.is_empty()).collect();
    segs.len() > 2 && segs[1] == VOLUME_FOLDER
}

/// Free space measured on a volume before and after an operation.
#[derive(Clone, Debug, Serialize)]
pub struct VolumeMeasure {
    pub volume: String,
    pub free_before: Option<u64>,
    pub free_after: Option<u64>,
    pub expected_freed_bytes: u64,
}

impl VolumeMeasure {
    pub fn measured_freed(&self) -> Option<i64> {
        Some(self.free_after? as i64 - self.free_before? as i64)
    }
}

/// Measures free space on the volumes of `paths` around `f`.
pub fn measured<T>(paths: &[(String, u64)], f: impl FnOnce() -> T) -> (T, Vec<VolumeMeasure>) {
    let mut vols: Vec<(String, u64)> = Vec::new();
    for (p, expected) in paths {
        let v = fsops::volume_root(&native(p)).unwrap_or_default();
        match vols.iter_mut().find(|(x, _)| lower(x) == lower(&v)) {
            Some(e) => e.1 += expected,
            None => vols.push((v, *expected)),
        }
    }
    let before: Vec<Option<u64>> = vols.iter().map(|(v, _)| fsops::free_space(&native(v))).collect();
    let r = f();
    let out = vols
        .into_iter()
        .zip(before)
        .map(|((volume, expected), free_before)| {
            let free_after = fsops::free_space(&native(&volume));
            VolumeMeasure { volume, free_before, free_after, expected_freed_bytes: expected }
        })
        .collect();
    (r, out)
}

/// Why an item may not be purged now.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum PurgeRefusal {
    /// Younger than 7 days, and early purge was not asked.
    TooRecent { days_left: u64 },
    /// Early purge is only for regenerable caches and install leftovers (by the built-in rule,
    /// not by what the index says).
    NotCacheOrLeftover { rule_id: String, class: Option<Class> },
    /// Its explanation was not shown when it was approved.
    NotExplained,
    /// The stored path is not inside a quarantine folder.
    OutsideQuarantine { path: String },
    /// What is stored is not what was moved.
    Changed,
    DeleteFailed { error: String },
}

impl PurgeRefusal {
    pub fn text(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Msg;
        match self {
            PurgeRefusal::TooRecent { days_left } => lang.fmt(Msg::PurgeTooRecent, &[("days", &days_left.to_string())]),
            PurgeRefusal::NotCacheOrLeftover { rule_id, .. } => lang.fmt(Msg::PurgeNotCache, &[("rule", rule_id)]),
            PurgeRefusal::NotExplained => lang.t(Msg::PurgeNotExplained).into(),
            PurgeRefusal::OutsideQuarantine { path } => lang.fmt(Msg::PurgeOutside, &[("path", path)]),
            PurgeRefusal::Changed => lang.t(Msg::PurgeChanged).into(),
            PurgeRefusal::DeleteFailed { error } => lang.fmt(Msg::PurgeFailed, &[("error", error)]),
        }
    }
}

/// The one rule for purging before 7 days, used by `apply --purge-now` and `purge --now`.
#[allow(unused_mut, unused_assignments)]
pub fn early_purge_allowed(entry: &Entry, built_in: &RuleSet) -> Result<(), PurgeRefusal> {
    let class = built_in.rules.iter().find(|r| r.id == entry.rule_id).map(|r| r.class);
    let mut ok = true;
    // GUARD-BEGIN(early-purge-class)
    ok = matches!(class, Some(Class::RegenerableCache) | Some(Class::InstallLeftover));
    // GUARD-END(early-purge-class)
    if !ok {
        return Err(PurgeRefusal::NotCacheOrLeftover { rule_id: entry.rule_id.clone(), class });
    }
    if !entry.explained {
        return Err(PurgeRefusal::NotExplained);
    }
    Ok(())
}

/// One item's purge result.
#[derive(Clone, Debug, Serialize)]
pub struct PurgeOutcome {
    pub id: String,
    pub original: String,
    pub bytes_on_disk: u64,
    pub purged: bool,
    pub refused: Option<PurgeRefusal>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PurgeReport {
    pub items: Vec<PurgeOutcome>,
    pub purged_bytes: u64,
    pub volumes: Vec<VolumeMeasure>,
    pub quarantine_held_bytes: u64,
}

/// Deletes quarantined items for good: those older than 7 days, and with `early` the younger
/// ones early purge allows. `ids` limits it to some items. Needs proof of presence.
pub fn purge(
    data_dir: &Path,
    ids: Option<&[String]>,
    early: bool,
    now_secs: u64,
    _proof: &crate::presence::Proof,
) -> PurgeReport {
    let built_in = RuleSet::built_in();
    let entries: Vec<Entry> =
        list(data_dir).into_iter().filter(|e| ids.map(|ids| ids.iter().any(|i| i == &e.id)).unwrap_or(true)).collect();
    let mut decisions: Vec<(Entry, Option<PurgeRefusal>)> = Vec::new();
    for e in entries {
        let age = now_secs.saturating_sub(e.moved_at);
        let mut refusal = None;
        // GUARD-BEGIN(purge-age)
        if age < PURGE_AFTER_SECS {
            refusal = Some(PurgeRefusal::TooRecent { days_left: (PURGE_AFTER_SECS - age).div_ceil(86_400) });
        }
        // GUARD-END(purge-age)
        if refusal.is_some() && early {
            refusal = early_purge_allowed(&e, &built_in).err();
        }
        decisions.push((e, refusal));
    }
    let to_measure: Vec<(String, u64)> =
        decisions.iter().filter(|(_, r)| r.is_none()).map(|(e, _)| (e.stored.clone(), e.expected_freed_bytes)).collect();
    let (items, volumes) = measured(&to_measure, || {
        let mut items = Vec::new();
        let mut gone = Vec::new();
        for (e, refusal) in decisions {
            let refused = match refusal {
                Some(r) => Some(r),
                None => match delete_stored(&e, data_dir) {
                    Ok(()) => {
                        gone.push(e.id.clone());
                        None
                    }
                    Err(r) => Some(r),
                },
            };
            items.push(PurgeOutcome {
                id: e.id.clone(),
                original: e.original.clone(),
                bytes_on_disk: e.bytes_on_disk,
                purged: refused.is_none(),
                refused,
            });
        }
        let _ = forget(data_dir, &gone);
        items
    });
    let purged_bytes = items.iter().filter(|i| i.purged).map(|i| i.bytes_on_disk).sum();
    PurgeReport { items, purged_bytes, volumes, quarantine_held_bytes: held_bytes(data_dir) }
}

fn delete_stored(e: &Entry, data_dir: &Path) -> Result<(), PurgeRefusal> {
    let stored = native(&e.stored);
    let item = match fsops::open_item(&stored) {
        Ok(i) => i,
        // Already gone: nothing to delete.
        Err(err) if err.kind == "not-found" => return Ok(()),
        Err(err) => return Err(PurgeRefusal::DeleteFailed { error: format!("{err:?}") }),
    };
    if !is_inside_quarantine(&item.final_path, data_dir) {
        return Err(PurgeRefusal::OutsideQuarantine { path: item.final_path.clone() });
    }
    if item.file_id != e.file_id {
        return Err(PurgeRefusal::Changed);
    }
    drop(item);
    fsops::delete_tree(&stored).map_err(|err| PurgeRefusal::DeleteFailed { error: err.to_string() })
}

/// Why an item could not be restored.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum RestoreRefusal {
    UnknownId,
    /// Something is at the original path again (the app recreated the folder): both are kept
    /// and the person chooses.
    OriginalExists { original: String, stored: String },
    /// The folder it was in is gone, or is now a link or somewhere else.
    ParentChanged { parent: String },
    /// The original place is not one the tool may act on.
    NotAllowed,
    /// What is stored is not what was moved.
    Changed,
    Missing,
    RenameFailed { error: crate::fsops::OpError },
}

impl RestoreRefusal {
    pub fn text(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Msg;
        match self {
            RestoreRefusal::UnknownId => lang.t(Msg::RestoreUnknown).into(),
            RestoreRefusal::OriginalExists { original, stored } => {
                lang.fmt(Msg::RestoreExists, &[("original", original), ("stored", stored)])
            }
            RestoreRefusal::ParentChanged { parent } => lang.fmt(Msg::RestoreParentChanged, &[("path", parent)]),
            RestoreRefusal::NotAllowed => lang.t(Msg::RestoreNotAllowed).into(),
            RestoreRefusal::Changed => lang.t(Msg::PurgeChanged).into(),
            RestoreRefusal::Missing => lang.t(Msg::RestoreMissing).into(),
            RestoreRefusal::RenameFailed { error } => lang.fmt(Msg::ApplyRenameFailed, &[("error", error.kind)]),
        }
    }
}

/// Puts a quarantined item back where it was, only if that path is free. Needs proof.
#[allow(unused_mut, unused_assignments)]
pub fn restore(data_dir: &Path, id: &str, ctx: &Context, _proof: &crate::presence::Proof) -> Result<Entry, RestoreRefusal> {
    let entries = list(data_dir);
    let e = entries.iter().find(|e| e.id == id).cloned().ok_or(RestoreRefusal::UnknownId)?;
    let original = native(&e.original);
    let mut free = true;
    // GUARD-BEGIN(restore-free-path)
    free = std::fs::symlink_metadata(&original).is_err();
    // GUARD-END(restore-free-path)
    if !free {
        return Err(RestoreRefusal::OriginalExists { original: e.original.clone(), stored: e.stored.clone() });
    }
    let parent = canon(&e.original).rsplit_once('\\').map(|(p, _)| p.to_string()).unwrap_or_default();
    match fsops::open_item(&native(&parent)) {
        Ok(p) if !p.is_link && p.is_dir && lower(&p.final_path) == lower(&parent) => {}
        _ => return Err(RestoreRefusal::ParentChanged { parent }),
    }
    let d = data_dir.to_string_lossy().into_owned();
    if !crate::act::verdict(&e.original, e.is_dir, ctx, Some(&d)).may_act() {
        return Err(RestoreRefusal::NotAllowed);
    }
    let item = match fsops::open_item(&native(&e.stored)) {
        Ok(i) => i,
        Err(err) if err.kind == "not-found" => return Err(RestoreRefusal::Missing),
        Err(err) => return Err(RestoreRefusal::RenameFailed { error: err }),
    };
    if item.file_id != e.file_id || !is_inside_quarantine(&item.final_path, data_dir) {
        return Err(RestoreRefusal::Changed);
    }
    fsops::rename(&item, &original).map_err(|error| RestoreRefusal::RenameFailed { error })?;
    drop(item);
    let _ = forget(data_dir, std::slice::from_ref(&e.id));
    Ok(e)
}
