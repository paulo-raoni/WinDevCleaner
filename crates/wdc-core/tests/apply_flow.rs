//! The plan → apply → restore → purge flow on an ordinary folder, through the library
//! (plan, sections 5.1 and 5.2). Runs everywhere; the Windows-only cases (links, short names,
//! a separate volume, free space) are in `apply_windows.rs`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use wdc_core::apply::{apply, ApplyOptions, Skip};
use wdc_core::plan::{draft, DraftOptions, Plan};
use wdc_core::presence::{Method, Proof};
use wdc_core::quarantine::{self, PurgeRefusal};
use wdc_core::rules::{Context, RuleSet};
use wdc_testkit::{fresh_dir, guard_assert, guard_assert_eq};

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

/// A fake user: profile, AppData and Temp inside `base`.
fn ctx_for(base: &Path) -> Context {
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    let env: HashMap<String, String> = [
        ("USERPROFILE", s(base.join("u"))),
        ("APPDATA", s(base.join("u").join("AppData").join("Roaming"))),
        ("LOCALAPPDATA", s(base.join("u").join("AppData").join("Local"))),
        ("TEMP", s(base.join("u").join("AppData").join("Local").join("Temp"))),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    // No program of the rules is running.
    Context::new(env, Some(Default::default()), None)
}

fn write(p: &Path, len: usize) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, vec![7u8; len]).unwrap();
}

/// Relative path → length, for every file under `root`.
fn tree(root: &Path) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let m = std::fs::symlink_metadata(e.path()).unwrap();
            let rel = e.path().strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if m.is_dir() {
                out.insert(format!("{rel}/"), 0);
                stack.push(e.path());
            } else {
                out.insert(rel, m.len());
            }
        }
    }
    out
}

fn proof() -> Proof {
    Proof::new(Method::AppApproval)
}

struct Fixture {
    base: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    ctx: Context,
}

fn fixture(name: &str) -> Fixture {
    let base = fresh_dir(name);
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let roaming = base.join("u").join("AppData").join("Roaming");
    std::fs::create_dir_all(base.join("u").join("AppData").join("Local").join("Temp")).unwrap();
    let cache = roaming.join("Code").join("CachedData");
    write(&cache.join("a").join("one.bin"), 2 * 1024 * 1024);
    write(&cache.join("b.txt"), 5000);
    std::fs::create_dir_all(cache.join("empty")).unwrap();
    let ctx = ctx_for(&base);
    Fixture { base, data, cache, ctx }
}

fn draft_cache(f: &Fixture) -> Plan {
    let opts = DraftOptions { paths: vec![f.cache.to_string_lossy().into_owned()], ..Default::default() };
    let d = draft(&opts, &RuleSet::built_in(), &f.ctx, Some(&f.data.to_string_lossy()), now());
    assert_eq!(d.plan.items.len(), 1, "{:#?}", d.not_included);
    assert_eq!(d.plan.items[0].rule_id, "vscode-caches");
    assert_eq!(d.plan.items[0].large_files.len(), 1, "the 2 MB file is recorded by file ID");
    d.plan
}

#[test]
fn apply_then_restore_gives_the_identical_tree() {
    let f = fixture("flow-restore");
    let before = tree(&f.cache);
    let plan = draft_cache(&f);
    let saved = plan.save_in(&f.data).unwrap();
    let plan = Plan::load(&saved).unwrap();
    let r = apply(&plan, &RuleSet::built_in(), &f.ctx, &f.data, &ApplyOptions { explained: true, ..Default::default() }, &proof(), now())
        .unwrap();
    assert_eq!(r.moved, 1, "{:#?}", r.items);
    assert!(!f.cache.exists(), "the item moved out");
    let held = quarantine::list(&f.data);
    assert_eq!(held.len(), 1);
    assert_eq!(r.quarantine_held_bytes, held[0].bytes_on_disk);
    let stored = PathBuf::from(&held[0].stored);
    assert_eq!(tree(&stored), before, "quarantine holds the same tree");

    quarantine::restore(&f.data, &held[0].id, &f.ctx, &proof()).unwrap();
    assert_eq!(tree(&f.cache), before, "restore gives the identical tree");
    assert!(quarantine::list(&f.data).is_empty());
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn restore_refuses_when_the_folder_was_recreated() {
    let f = fixture("flow-recreated");
    let plan = draft_cache(&f);
    apply(&plan, &RuleSet::built_in(), &f.ctx, &f.data, &ApplyOptions { explained: true, ..Default::default() }, &proof(), now()).unwrap();
    // The app recreates its cache folder meanwhile.
    write(&f.cache.join("new.txt"), 10);
    let id = quarantine::list(&f.data)[0].id.clone();
    let r = quarantine::restore(&f.data, &id, &f.ctx, &proof());
    guard_assert!(
        "restore-free-path",
        matches!(r, Err(quarantine::RestoreRefusal::OriginalExists { .. })),
        "restore must stop and report both paths: {r:?}"
    );
    assert_eq!(tree(&f.cache).len(), 1, "the recreated folder is untouched");
    assert_eq!(quarantine::list(&f.data).len(), 1, "the quarantined copy is kept");
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn a_condition_that_became_false_skips_the_item() {
    let f = fixture("flow-condition");
    let plan = draft_cache(&f);
    // VS Code started after planning.
    let mut env = f.ctx.env.clone();
    env.insert("X".into(), "1".into());
    let running = Context::new(env, Some(["Code.exe".to_string()].into_iter().collect()), None);
    let r = apply(&plan, &RuleSet::built_in(), &running, &f.data, &ApplyOptions::default(), &proof(), now()).unwrap();
    guard_assert_eq!("apply-conditions", r.moved, 0, "a running owner keeps the item: {:#?}", r.items);
    assert!(matches!(r.items[0].skipped, Some(Skip::ConditionFailed { .. })), "{:#?}", r.items);
    assert!(f.cache.exists());
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn an_item_replaced_after_planning_is_skipped() {
    let f = fixture("flow-replaced");
    let plan = draft_cache(&f);
    // Another folder takes the item's name; its large file is even the same file (a hard
    // link), so only the item's own file ID tells them apart.
    let away = f.base.join("elsewhere");
    std::fs::rename(&f.cache, &away).unwrap();
    std::fs::create_dir_all(f.cache.join("a")).unwrap();
    std::fs::hard_link(away.join("a").join("one.bin"), f.cache.join("a").join("one.bin")).unwrap();
    let r = apply(&plan, &RuleSet::built_in(), &f.ctx, &f.data, &ApplyOptions::default(), &proof(), now()).unwrap();
    guard_assert_eq!("apply-file-id", r.moved, 0, "another folder with the same name is not the planned item: {:#?}", r.items);
    assert!(matches!(&r.items[0].skipped, Some(Skip::Changed { what }) if what == "item"), "{:#?}", r.items);
    wdc_testkit::remove_tree(&f.base);
}

/// A sealed one-item plan for `path`, as an agent could write it by hand.
fn crafted(path: &Path) -> Plan {
    let item = wdc_core::plan::PlanItem {
        path: path.to_string_lossy().into_owned(),
        file_id: wdc_core::fsops::file_id(path).expect("the crafted item exists"),
        is_dir: true,
        large_files: vec![],
        rule_id: "vscode-caches".into(),
        rule_title: "VS Code caches".into(),
        class: wdc_core::rules::Class::RegenerableCache,
        bytes_on_disk: 0,
        expected_freed_bytes: 0,
        files: 0,
        measured_at: now(),
    };
    Plan::new(vec![item], now())
}

fn refused(f: &Fixture, ctx: &Context, path: &Path) -> Skip {
    let r = apply(&crafted(path), &RuleSet::built_in(), ctx, &f.data, &ApplyOptions::default(), &proof(), now()).unwrap();
    guard_assert_eq!("apply-verdict/apply-rematch", r.moved, 0, "{} must not move: {:#?}", path.display(), r.items);
    assert!(path.exists(), "{} is untouched", path.display());
    r.items[0].skipped.clone().unwrap()
}

#[test]
fn forbidden_places_and_non_items_are_refused_on_the_real_path() {
    let f = fixture("flow-forbidden");
    let profile = f.base.join("u");
    let roaming = profile.join("AppData").join("Roaming");

    // A folder that holds a Git repository, even where a cache rule matches.
    let repo = roaming.join("Code").join("Cache");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let s = refused(&f, &f.ctx, &repo);
    assert!(matches!(&s, Skip::NotAllowed { why: wdc_core::explain::WhyNot::Forbidden { .. }, .. }), "{s:?}");
    std::fs::remove_dir_all(repo.join(".git")).unwrap();

    // AppData moved into Documents: the rule matches, the place is personal.
    let docs_appdata = profile.join("Documents").join("AppData");
    let in_docs = docs_appdata.join("Code").join("GPUCache");
    std::fs::create_dir_all(&in_docs).unwrap();
    let mut env = f.ctx.env.clone();
    env.insert("APPDATA".into(), docs_appdata.to_string_lossy().into_owned());
    let s = refused(&f, &Context::new(env, Some(Default::default()), None), &in_docs);
    assert!(matches!(&s, Skip::NotAllowed { .. }), "{s:?}");

    // An allowed root itself is never an item.
    let mut env = f.ctx.env.clone();
    env.insert("TEMP".into(), repo.to_string_lossy().into_owned());
    let s = refused(&f, &Context::new(env, Some(Default::default()), None), &repo);
    assert!(matches!(&s, Skip::NotAllowed { why: wdc_core::explain::WhyNot::AllowedRootItself, .. }), "{s:?}");

    // A folder inside a rule's item is not an item.
    let inside = f.cache.join("a");
    let s = refused(&f, &f.ctx, &inside);
    assert!(matches!(&s, Skip::NoLongerMatches { .. }), "{s:?}");

    // `..` that reads like a cache path but leads into Documents.
    let target = profile.join("Documents").join("thesis");
    std::fs::create_dir_all(&target).unwrap();
    let dotdot = f.cache.join("..").join("..").join("..").join("..").join("Documents").join("thesis");
    let s = refused(&f, &f.ctx, &dotdot);
    assert!(matches!(&s, Skip::NotAllowed { .. } | Skip::NoLongerMatches { .. }), "{s:?}");

    // Inside an allowed root, but no rule knows it.
    let important = profile.join("AppData").join("Local").join("Important");
    std::fs::create_dir_all(&important).unwrap();
    let s = refused(&f, &f.ctx, &important);
    assert!(matches!(&s, Skip::NoLongerMatches { .. }), "{s:?}");
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn a_plan_older_than_a_day_or_edited_is_refused_whole() {
    let f = fixture("flow-old");
    let plan = draft_cache(&f);
    let r = apply(&plan, &RuleSet::built_in(), &f.ctx, &f.data, &ApplyOptions::default(), &proof(), now() + 25 * 3600);
    guard_assert!("plan-age", r.is_err(), "a 25-hour-old plan must be refused");
    let mut edited = plan.clone();
    edited.items[0].path = f.base.join("u").to_string_lossy().into_owned();
    assert!(apply(&edited, &RuleSet::built_in(), &f.ctx, &f.data, &ApplyOptions::default(), &proof(), now()).is_err());
    assert!(f.cache.exists());
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn early_purge_is_only_for_caches_and_leftovers_that_were_explained() {
    let f = fixture("flow-purge");
    let plan = draft_cache(&f);
    apply(&plan, &RuleSet::built_in(), &f.ctx, &f.data, &ApplyOptions { explained: true, ..Default::default() }, &proof(), now()).unwrap();
    let entry = quarantine::list(&f.data)[0].clone();

    // Without --now nothing younger than 7 days goes.
    let r = quarantine::purge(&f.data, None, false, now(), &proof());
    guard_assert!("purge-age", !r.items[0].purged, "a fresh item waits 7 days");
    assert!(matches!(r.items[0].refused, Some(PurgeRefusal::TooRecent { days_left: 7 })), "{:#?}", r.items);

    // The index says "cache", but the built-in rule decides: an app-managed rule is refused.
    let mut lie = entry.clone();
    lie.rule_id = "vscode-workspace-storage".into();
    guard_assert!(
        "early-purge-class",
        matches!(quarantine::early_purge_allowed(&lie, &RuleSet::built_in()), Err(PurgeRefusal::NotCacheOrLeftover { .. })),
        "early purge of an app-managed item must be refused"
    );
    let mut unexplained = entry.clone();
    unexplained.explained = false;
    assert_eq!(quarantine::early_purge_allowed(&unexplained, &RuleSet::built_in()), Err(PurgeRefusal::NotExplained));
    assert_eq!(quarantine::early_purge_allowed(&entry, &RuleSet::built_in()), Ok(()));

    // With --now a cache goes at once.
    let r = quarantine::purge(&f.data, None, true, now(), &proof());
    assert!(r.items[0].purged, "{:#?}", r.items);
    assert!(!Path::new(&entry.stored).exists());
    assert_eq!(r.quarantine_held_bytes, 0);

    // After 7 days anything quarantined goes without --now.
    let f2 = fixture("flow-purge-late");
    let plan = draft_cache(&f2);
    apply(&plan, &RuleSet::built_in(), &f2.ctx, &f2.data, &ApplyOptions::default(), &proof(), now()).unwrap();
    let r = quarantine::purge(&f2.data, None, false, now() + 8 * 86_400, &proof());
    assert!(r.items[0].purged, "{:#?}", r.items);
    wdc_testkit::remove_tree(&f2.base);
    wdc_testkit::remove_tree(&f.base);
}
