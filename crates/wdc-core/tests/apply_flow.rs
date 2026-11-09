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

// ---------------------------------------------------------------------------------------------
// The same suite for every built-in quarantine rule (plan, section 8, slice 5).
//
// The rules are enumerated from `RuleSet::built_in()`; a fixture is looked up by rule id. A
// quarantine rule without a fixture turns `every_quarantine_rule_has_a_fixture` red, and the
// other tests below name the rule in every failure.
// ---------------------------------------------------------------------------------------------

use wdc_core::rules::{Action, Class, Condition, Origin, Rule};

const MIB: usize = 1024 * 1024;
const GUID: &str = ".3f2504e0-4f89-11d3-9a0c-0305e82c3301";
const OBSOLETE: &str = "ms-python.python-2024.1.0";

fn quarantine_rules() -> Vec<Rule> {
    RuleSet::built_in().rules.into_iter().filter(|r| r.action == Action::Quarantine && r.origin == Origin::BuiltIn).collect()
}

/// Builds the item of a rule under `base\u` and returns its path: a few files, one of 2 MB, and
/// every `require` condition of the rule true.
type Builder = fn(&Path) -> PathBuf;

fn profile(base: &Path) -> PathBuf {
    base.join("u")
}
fn roaming(base: &Path) -> PathBuf {
    profile(base).join("AppData").join("Roaming")
}
fn local(base: &Path) -> PathBuf {
    profile(base).join("AppData").join("Local")
}

/// Contents shared by every item: a large file, a small one and an empty folder.
fn fill(item: &Path) {
    write(&item.join("a").join("one.bin"), 2 * MIB);
    write(&item.join("b.txt"), 5000);
    std::fs::create_dir_all(item.join("empty")).unwrap();
}

/// An obsolete extension: listed in `.obsolete`, not referenced by `extensions.json`.
fn extension_folder(base: &Path, dot: &str) -> PathBuf {
    let ext = profile(base).join(dot).join("extensions");
    let item = ext.join(OBSOLETE);
    fill(&item);
    write(&ext.join("ms-python.python-2024.2.0").join("out").join("main.js"), 100);
    std::fs::write(ext.join(".obsolete"), format!(r#"{{"{OBSOLETE}": true}}"#)).unwrap();
    std::fs::write(
        ext.join("extensions.json"),
        r#"[{"identifier":{"id":"ms-python.python"},"version":"2024.2.0","relativeLocation":"ms-python.python-2024.2.0"}]"#,
    )
    .unwrap();
    item
}

/// An interrupted install: a GUID-named folder (older than a day once the rule's age is added to
/// the context's clock), not referenced by `extensions.json`.
fn staging_folder(base: &Path, dot: &str) -> PathBuf {
    let ext = profile(base).join(dot).join("extensions");
    let item = ext.join(GUID);
    fill(&item);
    std::fs::write(ext.join(".obsolete"), "{}").unwrap();
    std::fs::write(ext.join("extensions.json"), "[]").unwrap();
    item
}

fn cache_folder(item: PathBuf) -> PathBuf {
    fill(&item);
    item
}

fn builder(rule_id: &str) -> Option<Builder> {
    Some(match rule_id {
        "vscode-obsolete-extension" => |b| extension_folder(b, ".vscode"),
        "vscode-insiders-obsolete-extension" => |b| extension_folder(b, ".vscode-insiders"),
        "cursor-obsolete-extension" => |b| extension_folder(b, ".cursor"),
        "vscode-staging-folder" => |b| staging_folder(b, ".vscode"),
        "vscode-insiders-staging-folder" => |b| staging_folder(b, ".vscode-insiders"),
        "cursor-staging-folder" => |b| staging_folder(b, ".cursor"),
        "vscode-caches" => |b| cache_folder(roaming(b).join("Code").join("Code Cache")),
        "vscode-insiders-caches" => |b| cache_folder(roaming(b).join("Code - Insiders").join("GPUCache")),
        "cursor-caches" => |b| cache_folder(roaming(b).join("Cursor").join("CachedData")),
        "cpptools-ipch" => |b| cache_folder(local(b).join("Microsoft").join("vscode-cpptools").join("ipch")),
        "nvidia-shader-caches" => |b| cache_folder(local(b).join("NVIDIA").join("DXCache")),
        "steam-htmlcache" => |b| cache_folder(local(b).join("Steam").join("htmlcache")),
        _ => return None,
    })
}

struct RuleFx {
    rule: Rule,
    base: PathBuf,
    data: PathBuf,
    item: PathBuf,
    ctx: Context,
}

/// A context for this fixture: the named programs running, and the clock far enough ahead for
/// every `older-than` condition of the rule.
fn ctx_with(f: &RuleFx, running: &[&str]) -> Context {
    let env = ctx_for(&f.base).env;
    let mut c = Context::new(env, Some(running.iter().map(|n| n.to_string()).collect()), None);
    let days = f.rule.require.iter().filter_map(|c| if let Condition::OlderThan { days } = c { Some(*days as u64) } else { None }).max();
    if let Some(d) = days {
        c.now = std::time::SystemTime::now() + std::time::Duration::from_secs((d + 1) * 86_400);
    }
    c
}

fn rule_fx(rule: &Rule, tag: &str) -> RuleFx {
    let build = builder(&rule.id).unwrap_or_else(|| panic!("{}: no fixture", rule.id));
    let base = fresh_dir(&format!("rule-{}-{tag}", rule.id));
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(local(&base).join("Temp")).unwrap();
    let item = build(&base);
    let mut f = RuleFx { rule: rule.clone(), base: base.clone(), data, item, ctx: ctx_for(&base) };
    f.ctx = ctx_with(&f, &[]);
    f
}

/// The plan for the rule's item: exactly that item, with its large file recorded.
fn plan_of(f: &RuleFx) -> Plan {
    let id = &f.rule.id;
    let opts = DraftOptions { paths: vec![f.item.to_string_lossy().into_owned()], ..Default::default() };
    let d = draft(&opts, &RuleSet::built_in(), &f.ctx, Some(&f.data.to_string_lossy()), now());
    assert_eq!(d.plan.items.len(), 1, "{id}: {:#?}", d.not_included);
    assert_eq!(&d.plan.items[0].rule_id, id, "{id}: the item belongs to its rule");
    assert!(!d.plan.items[0].large_files.is_empty(), "{id}: the 2 MB file is recorded by file ID");
    d.plan
}

fn run(f: &RuleFx, plan: &Plan, ctx: &Context, opts: &ApplyOptions) -> wdc_core::apply::ApplyReport {
    apply(plan, &RuleSet::built_in(), ctx, &f.data, opts, &proof(), now()).unwrap()
}

fn explained() -> ApplyOptions {
    ApplyOptions { explained: true, ..Default::default() }
}

#[test]
fn every_quarantine_rule_has_a_fixture() {
    let rules = quarantine_rules();
    assert!(!rules.is_empty(), "the built-in rules have quarantine rules");
    let missing: Vec<&str> = rules.iter().filter(|r| builder(&r.id).is_none()).map(|r| r.id.as_str()).collect();
    assert!(missing.is_empty(), "quarantine rules without a fixture in apply_flow.rs: {missing:?}");
}

#[test]
fn every_quarantine_rule_plans_its_item_and_apply_then_restore_gives_the_identical_tree() {
    for rule in quarantine_rules() {
        let id = &rule.id;
        let f = rule_fx(&rule, "restore");
        let before = tree(&f.item);
        let plan = plan_of(&f);
        let saved = plan.save_in(&f.data).unwrap();
        let plan = Plan::load(&saved).unwrap();
        let r = run(&f, &plan, &f.ctx, &explained());
        assert_eq!(r.moved, 1, "{id}: {:#?}", r.items);
        assert!(!f.item.exists(), "{id}: the item moved out");
        let held = quarantine::list(&f.data);
        assert_eq!(held.len(), 1, "{id}");
        assert_eq!(held[0].rule_id, *id);
        assert_eq!(r.quarantine_held_bytes, held[0].bytes_on_disk, "{id}");
        assert_eq!(tree(&PathBuf::from(&held[0].stored)), before, "{id}: quarantine holds the same tree");

        quarantine::restore(&f.data, &held[0].id, &f.ctx, &proof()).unwrap_or_else(|e| panic!("{id}: restore refused: {e:?}"));
        assert_eq!(tree(&f.item), before, "{id}: restore gives the identical tree");
        assert!(quarantine::list(&f.data).is_empty(), "{id}");
        wdc_testkit::remove_tree(&f.base);
    }
}

#[test]
fn restore_refuses_when_the_folder_was_recreated_for_every_quarantine_rule() {
    let mut failures = Vec::new();
    for rule in quarantine_rules() {
        let id = &rule.id;
        let f = rule_fx(&rule, "recreated");
        let plan = plan_of(&f);
        run(&f, &plan, &f.ctx, &explained());
        // The app recreates the folder meanwhile.
        write(&f.item.join("new.txt"), 10);
        let entry_id = quarantine::list(&f.data)[0].id.clone();
        let r = quarantine::restore(&f.data, &entry_id, &f.ctx, &proof());
        if !matches!(r, Err(quarantine::RestoreRefusal::OriginalExists { .. })) {
            failures.push(format!("{id}: restore must stop and report both paths: {r:?}"));
        }
        let (items, kept) = (tree(&f.item).len(), quarantine::list(&f.data).len());
        if items != 1 {
            failures.push(format!("{id}: the recreated folder is untouched: {items} files found, expected 1"));
        }
        if kept != 1 {
            failures.push(format!("{id}: the quarantined copy is kept: {kept} entries found, expected 1"));
        }
        wdc_testkit::remove_tree(&f.base);
    }
    guard_assert!("restore-free-path", failures.is_empty(), "{failures:#?}");
}

#[test]
fn an_item_replaced_after_planning_is_skipped_for_every_quarantine_rule() {
    let mut failures = Vec::new();
    for rule in quarantine_rules() {
        let id = &rule.id;
        let f = rule_fx(&rule, "replaced");
        let plan = plan_of(&f);
        // Another folder takes the item's name; its large files are even the same files (hard
        // links), so only the item's own file ID tells them apart.
        let away = f.base.join("elsewhere");
        std::fs::rename(&f.item, &away).unwrap();
        for (rel, len) in tree(&away) {
            if len >= MIB as u64 {
                let to = f.item.join(&rel);
                std::fs::create_dir_all(to.parent().unwrap()).unwrap();
                std::fs::hard_link(away.join(&rel), to).unwrap();
            }
        }
        let r = run(&f, &plan, &f.ctx, &ApplyOptions::default());
        let skipped_as_changed = matches!(&r.items[0].skipped, Some(Skip::Changed { what }) if what == "item");
        if r.moved != 0 || !skipped_as_changed {
            failures.push(format!("{id}: another folder with the same name is not the planned item: {:#?}", r.items));
        }
        if !(away.exists() && f.item.exists()) {
            failures.push(format!(
                "{id}: nothing was moved or removed: elsewhere exists = {}, item exists = {}",
                away.exists(),
                f.item.exists()
            ));
        }
        wdc_testkit::remove_tree(&f.base);
    }
    guard_assert!("apply-file-id", failures.is_empty(), "{failures:#?}");
}

#[test]
fn a_condition_that_became_false_skips_the_item_for_every_quarantine_rule() {
    let mut failures = Vec::new();
    for rule in quarantine_rules() {
        let id = &rule.id;
        // `nvidia-shader-caches` has no `require` condition (a game holding a file is caught by
        // the rename itself, see apply_windows.rs), so there is nothing to make false: it is
        // the only rule exempt from this test.
        if rule.require.is_empty() {
            continue;
        }
        let f = rule_fx(&rule, "condition");
        let before = tree(&f.item);
        let plan = plan_of(&f);
        let mut tried = 0;
        for cond in &rule.require {
            let mut restore_file: Option<(PathBuf, String)> = None;
            let (what, ctx) = match cond {
                // The owner program started after planning.
                Condition::ProcessNotRunning { names } => (format!("{} started", names[0]), ctx_with(&f, &[names[0].as_str()])),
                // The folder became referenced by the extensions list after planning.
                Condition::NotReferencedBy { file, .. } => {
                    let path = wdc_core::rules::native(&f.ctx.expand(file).unwrap());
                    let name = f.item.file_name().unwrap().to_string_lossy().into_owned();
                    restore_file = Some((path.clone(), std::fs::read_to_string(&path).unwrap()));
                    std::fs::write(&path, format!(r#"[{{"identifier":{{"id":"x"}},"version":"1","relativeLocation":"{name}"}}]"#)).unwrap();
                    ("referenced by extensions.json".to_string(), ctx_with(&f, &[]))
                }
                // An age cannot be made to shrink after planning.
                _ => continue,
            };
            tried += 1;
            let r = run(&f, &plan, &ctx, &ApplyOptions::default());
            if let Some((path, text)) = restore_file {
                std::fs::write(path, text).unwrap();
            }
            let kind_ok = matches!(&r.items[0].skipped, Some(Skip::ConditionFailed { condition, .. }) if std::mem::discriminant(condition) == std::mem::discriminant(cond));
            if r.moved != 0 || !kind_ok || !f.item.exists() || tree(&f.item) != before {
                failures.push(format!("{id}: {what} must keep the item in place: {:#?}", r.items));
                break;
            }
        }
        if tried == 0 {
            failures.push(format!("{id}: has conditions but none this test can make false"));
        }
        wdc_testkit::remove_tree(&f.base);
    }
    guard_assert!("apply-conditions", failures.is_empty(), "{failures:#?}");
}

#[test]
fn early_purge_follows_the_class_of_every_quarantine_rule() {
    for rule in quarantine_rules() {
        let id = &rule.id;
        let f = rule_fx(&rule, "purge");
        let plan = plan_of(&f);
        run(&f, &plan, &f.ctx, &explained());
        let entry = quarantine::list(&f.data)[0].clone();
        let allowed = matches!(rule.class, Class::RegenerableCache | Class::InstallLeftover);
        let verdict = quarantine::early_purge_allowed(&entry, &RuleSet::built_in());
        assert_eq!(verdict.is_ok(), allowed, "{id} ({:?}): early purge follows the class: {verdict:?}", rule.class);
        if allowed {
            let r = quarantine::purge(&f.data, None, true, now(), &proof());
            assert!(r.items[0].purged, "{id}: {:#?}", r.items);
            assert!(!Path::new(&entry.stored).exists(), "{id}: the purged copy is gone");
            assert_eq!(r.quarantine_held_bytes, 0, "{id}");
        }
        wdc_testkit::remove_tree(&f.base);
    }
}
