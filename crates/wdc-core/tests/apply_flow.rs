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
        "edge-cache" => |b| cache_folder(local(b).join("Microsoft").join("Edge").join("User Data").join("Profile 3").join("Cache")),
        "pip-cache" => |b| {
            let package = local(b).join("Packages").join("PythonSoftwareFoundation.Python.3.12_qbz5n2kfra8p0");
            cache_folder(package.join("LocalCache").join("Local").join("pip").join("Cache"))
        },
        "uv-cache" => |b| cache_folder(local(b).join("uv").join("cache")),
        "playwright-browsers" => |b| cache_folder(local(b).join("ms-playwright").join("chromium-1148")),
        "temp-old" => |b| cache_folder(local(b).join("Temp").join("old-build-output")),
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
                // The clock is the real one again: the item, created moments ago, is too new.
                Condition::OlderThan { days } => {
                    let env = ctx_for(&f.base).env;
                    (format!("younger than {days} days"), Context::new(env, Some(Default::default()), None))
                }
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

// ---- the true size of every item, and a size that is incomplete (rc.2, task 1) ----

/// A fake user under `base`, with the clock far enough ahead for `temp-old` (older than 7 days).
fn temp_ctx(base: &Path) -> Context {
    let mut c = ctx_for(base);
    c.now = std::time::SystemTime::now() + std::time::Duration::from_secs(9 * 86_400);
    c
}

fn temp_dir(base: &Path) -> PathBuf {
    let t = local(base).join("Temp");
    std::fs::create_dir_all(&t).unwrap();
    t
}

fn draft_of(base: &Path, item: &Path) -> wdc_core::plan::Draft {
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let opts = DraftOptions { paths: vec![item.to_string_lossy().into_owned()], ..Default::default() };
    draft(&opts, &RuleSet::built_in(), &temp_ctx(base), Some(&data.to_string_lossy()), now())
}

#[test]
fn explain_of_a_file_reports_its_allocation_and_one_file() {
    let base = fresh_dir("file-explain");
    let file = temp_dir(&base).join("update.bin");
    write(&file, 24_576);
    let e = wdc_core::explain::explain(&file.to_string_lossy(), &RuleSet::built_in(), &temp_ctx(&base), None);
    assert_eq!(e.kind, "file");
    guard_assert!("file-root-size", e.bytes_on_disk >= 24_576, "the file's allocation is reported, got {}", e.bytes_on_disk);
    assert_eq!(e.logical_bytes, 24_576);
    assert_eq!(e.files, 1);
    assert!(e.complete && e.errors == 0 && e.first_error_kind.is_none());
    let json = serde_json::to_value(&e).unwrap();
    assert!(json["bytes_on_disk"].as_u64().unwrap() > 0);
    assert_eq!(json["files"], 1);
    assert_eq!(json["complete"], true);
    assert!(json.get("first_error_kind").is_none(), "absent when complete");
    assert!(e.size_line(wdc_core::i18n::Lang::En).ends_with("in 1 file."), "{}", e.size_line(wdc_core::i18n::Lang::En));
    assert!(e.size_line(wdc_core::i18n::Lang::PtBr).ends_with("em 1 arquivo."));
    assert!(e.partial_line(wdc_core::i18n::Lang::En).is_none());
    wdc_testkit::remove_tree(&base);
}

#[test]
fn a_temp_old_file_item_goes_through_the_plan_with_its_real_size() {
    let base = fresh_dir("file-plan");
    let file = temp_dir(&base).join("update.exe");
    write(&file, 24_576);
    let d = draft_of(&base, &file);
    let offered = d.plan.items.first().map_or(0, |i| i.bytes_on_disk);
    guard_assert!("file-root-size", offered >= 24_576, "the plan offers the file with its size, got {offered}: {:#?}", d.not_included);
    assert_eq!(d.plan.items.len(), 1, "{:#?}", d.not_included);
    let i = &d.plan.items[0];
    assert_eq!(i.rule_id, "temp-old");
    assert!(!i.is_dir);
    assert_eq!(i.expected_freed_bytes, i.bytes_on_disk);
    assert_eq!(i.files, 1);
    assert_eq!(d.plan.bytes_on_disk(), i.bytes_on_disk);
    wdc_testkit::remove_tree(&base);
}

#[test]
fn a_file_with_a_second_name_outside_the_item_reports_expected_freed_zero() {
    let base = fresh_dir("file-linked");
    let file = temp_dir(&base).join("shared.bin");
    write(&file, 8192);
    let other = base.join("elsewhere").join("shared-too.bin");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::hard_link(&file, &other).unwrap();
    let e = wdc_core::explain::explain(&file.to_string_lossy(), &RuleSet::built_in(), &temp_ctx(&base), None);
    assert!(e.bytes_on_disk >= 8192, "the file is measured: {}", e.bytes_on_disk);
    assert_eq!(e.hard_links.len(), 1, "{:#?}", e.hard_links);
    guard_assert_eq!("expected-freed", e.expected_freed_bytes, 0, "the file frees nothing while its other name exists");
    wdc_testkit::remove_tree(&base);
}

#[test]
fn a_zero_byte_item_is_left_out_as_nothing_to_free() {
    let base = fresh_dir("zero");
    let file = temp_dir(&base).join("empty.tmp");
    write(&file, 0);
    let d = draft_of(&base, &file);
    let left_out = d.not_included.iter().find(|n| n.path.ends_with("empty.tmp"));
    guard_assert!("plan-nothing", d.plan.items.is_empty() && left_out.is_some(), "the 0-byte item is not offered: {:#?}", d.plan.items);
    let why = &left_out.unwrap().why_not;
    assert_eq!(why, &vec![wdc_core::explain::WhyNot::NothingToFree]);
    assert_eq!(serde_json::to_value(&why[0]).unwrap()["reason"], "nothing-to-free");
    let (en, pt) = (why[0].text(wdc_core::i18n::Lang::En), why[0].text(wdc_core::i18n::Lang::PtBr));
    assert!(!en.is_empty() && !pt.is_empty() && en != pt);
    wdc_testkit::remove_tree(&base);
}

#[test]
fn the_reason_codes_of_the_two_new_exclusions_are_stable() {
    use wdc_core::explain::WhyNot;
    assert_eq!(serde_json::to_value(WhyNot::NotFullyReadable).unwrap()["reason"], "not-fully-readable");
    assert_eq!(serde_json::to_value(WhyNot::NothingToFree).unwrap()["reason"], "nothing-to-free");
    for w in [WhyNot::NotFullyReadable, WhyNot::NothingToFree] {
        assert_ne!(w.text(wdc_core::i18n::Lang::En), w.text(wdc_core::i18n::Lang::PtBr));
    }
}

/// An item that is both incomplete and 0 bytes is left out as not-fully-readable: its size is
/// unknown, so "nothing to free" would state a size nobody measured. The cloud session runs as
/// root and cannot make a portable item with an unreadable part, so the decision `draft` takes is
/// tested where it is made, with the two facts given directly.
#[test]
fn an_incomplete_item_that_also_frees_nothing_is_left_out_as_not_fully_readable() {
    use wdc_core::explain::WhyNot;
    use wdc_core::plan::left_out_for_size;
    assert_eq!(left_out_for_size(false, 0), Some(WhyNot::NotFullyReadable), "incomplete and 0 bytes: the incomplete reason wins");
    assert_eq!(left_out_for_size(false, 4096), Some(WhyNot::NotFullyReadable));
    assert_eq!(left_out_for_size(true, 0), Some(WhyNot::NothingToFree));
    assert_eq!(left_out_for_size(true, 4096), None);
}

/// The Restart Manager may reject a directory, which would make every folder probe fail: a
/// folder registers only its files (within the cap), a file registers itself.
#[test]
fn the_holder_probe_registers_a_file_itself_and_a_folder_only_through_its_files() {
    let base = fresh_dir("probe-files");
    let folder = base.join("cache");
    std::fs::create_dir_all(folder.join("sub")).unwrap();
    for (i, d) in [&folder, &folder.join("sub")].into_iter().enumerate() {
        for f in 0..3 {
            std::fs::write(d.join(format!("f{i}{f}.bin")), b"x").unwrap();
        }
    }
    let all = wdc_core::fsops::probe_files(&folder, 100);
    assert_eq!(all.len(), 6, "{all:?}");
    assert!(!all.contains(&folder) && all.iter().all(|p| p.is_file()), "{all:?}");
    assert_eq!(wdc_core::fsops::probe_files(&folder, 4).len(), 4, "capped");
    let one = folder.join("f00.bin");
    assert_eq!(wdc_core::fsops::probe_files(&one, 4), vec![one.clone()]);
    assert!(wdc_core::fsops::probe_files(&base.join("missing"), 4).is_empty());
    wdc_testkit::remove_tree(&base);
}

// ---- items another program holds open, and folders with recent content (rc.2, task 2) ----

fn fake_holders() -> Vec<wdc_core::fsops::Holder> {
    vec![
        wdc_core::fsops::Holder { pid: 4242, name: "NVIDIA Container".into() },
        wdc_core::fsops::Holder { pid: 4343, name: "NVIDIA Overlay".into() },
    ]
}

#[test]
fn a_cache_folder_another_program_holds_open_is_left_out_naming_the_holders() {
    use wdc_core::explain::WhyNot;
    use wdc_core::fsops::HolderProbe;
    use wdc_core::i18n::Lang;
    let f = fixture("flow-held-plan");
    let rules = RuleSet::built_in();
    let data = f.data.to_string_lossy().into_owned();
    let draft_with = |probe: HolderProbe| {
        let opts = DraftOptions { paths: vec![f.cache.to_string_lossy().into_owned()], holders: probe, ..Default::default() };
        draft(&opts, &rules, &f.ctx, Some(&data), now())
    };

    // Controls that do not depend on the exclusion: nobody holds it, or the probe itself fails.
    let free = draft_with(HolderProbe::new(|_| Some(Vec::new())));
    assert_eq!(free.plan.items.len(), 1, "nobody holds it: {:#?}", free.not_included);
    let failed = draft_with(HolderProbe::new(|_| None));
    assert_eq!(failed.plan.items.len(), 1, "a probe that fails keeps the item: {:#?}", failed.not_included);
    assert!(failed.not_included.is_empty(), "and says nothing: {:#?}", failed.not_included);

    // Held by two programs.
    let held = draft_with(HolderProbe::new(|_| Some(fake_holders())));
    let why = held.not_included.iter().find(|n| n.path.ends_with("CachedData")).map(|n| n.why_not.clone());
    guard_assert!(
        "plan-held",
        held.plan.items.is_empty() && why == Some(vec![WhyNot::HeldOpen { holders: fake_holders() }]),
        "a held folder is left out as held-open with its holders: items {:?}, left out {:#?}",
        held.plan.items.len(),
        held.not_included
    );
    let why = why.unwrap();
    assert_eq!(serde_json::to_value(&why[0]).unwrap()["reason"], "held-open");
    assert_eq!(serde_json::to_value(&why[0]).unwrap()["holders"][0]["pid"], 4242);
    assert_eq!(why[0].text(Lang::En), "held open by: NVIDIA Container, NVIDIA Overlay. Close them and draft again.");
    assert_eq!(why[0].text(Lang::PtBr), "mantido aberto por: NVIDIA Container, NVIDIA Overlay. Feche-os e gere o plano de novo.");
    // One holder: the singular.
    let one = vec![wdc_core::fsops::Holder { pid: 7, name: "NVIDIA Container".into() }];
    let one = WhyNot::HeldOpen { holders: one };
    assert_eq!(one.text(Lang::En), "held open by: NVIDIA Container. Close it and draft again.");
    assert_eq!(one.text(Lang::PtBr), "mantido aberto por: NVIDIA Container. Feche-o e gere o plano de novo.");
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn held_open_and_open_failed_skips_read_in_both_languages_and_stay_distinct_in_json() {
    use wdc_core::fsops::OpError;
    use wdc_core::i18n::Lang;
    let held = Skip::HeldOpen { stage: "rename", error: OpError::from_code(5), holders: fake_holders() };
    assert_eq!(held.text(Lang::En), "held open by NVIDIA Container, NVIDIA Overlay; close them and run again");
    let one = Skip::HeldOpen { stage: "open", error: OpError::from_code(32), holders: fake_holders()[..1].to_vec() };
    assert_eq!(one.text(Lang::En), "held open by NVIDIA Container; close it and run again");
    assert_eq!(one.text(Lang::PtBr), "mantido aberto por NVIDIA Container; feche-o e rode de novo");
    let j = serde_json::to_value(&held).unwrap();
    assert_eq!((j["reason"].as_str(), j["stage"].as_str(), j["holders"][1]["name"].as_str()), (Some("held-open"), Some("rename"), Some("NVIDIA Overlay")));
    let open = serde_json::to_value(Skip::OpenFailed { error: OpError::from_code(5) }).unwrap();
    let rename = serde_json::to_value(Skip::RenameFailed { error: OpError::from_code(5) }).unwrap();
    assert_eq!((open["reason"].as_str(), rename["reason"].as_str()), (Some("open-failed"), Some("rename-failed")));
    assert!(Skip::OpenFailed { error: OpError::from_code(5) }.text(Lang::PtBr).contains("abri"));
}

#[test]
fn the_free_space_line_after_a_move_says_the_items_are_in_quarantine() {
    use wdc_core::i18n::{Lang, Msg};
    let args = [("volume", "C:\\"), ("before", "12.2 GiB"), ("after", "12.2 GiB"), ("measured", "-16.0 KiB"), ("expected", "148 MB")];
    let en = Lang::En.fmt(Msg::CliFreeSpaceMoved, &args);
    let pt = Lang::PtBr.fmt(Msg::CliFreeSpaceMoved, &args);
    assert!(en.contains("in quarantine, not deleted") && en.contains("when they are purged (148 MB expected)"), "{en}");
    assert!(pt.contains("na quarentena, não foram apagados") && pt.contains("(148 MB esperados)"), "{pt}");
    assert!(!en.contains("expected 0") && !pt.contains("esperado 0"));
}

/// Sets the modified time of a file (a future time stands for "changed recently" when the
/// clock of the test is moved ahead).
fn touch(p: &Path, t: std::time::SystemTime) {
    std::fs::OpenOptions::new().write(true).open(p).unwrap().set_modified(t).unwrap();
}

#[test]
fn a_temp_folder_with_recent_content_is_not_offered_and_an_old_one_is() {
    use wdc_core::explain::WhyNot;
    let base = fresh_dir("temp-recent");
    let t = temp_dir(&base);
    let recent = t.join("DiagOutputDir");
    write(&recent.join("old.etl"), 8192);
    write(&recent.join("logs").join("deep").join("trace.etl"), 8192);
    let quiet = t.join("quiet");
    write(&quiet.join("a").join("b.tmp"), 8192);
    // The test clock is 9 days ahead of the real one. A file changed 3 days ahead of the real
    // clock is 6 days old to the rule: under the 7 days, though the folder itself is 9 days old.
    let ahead = |days: u64| std::time::SystemTime::now() + std::time::Duration::from_secs(days * 86_400);
    touch(&recent.join("logs").join("deep").join("trace.etl"), ahead(3));

    let d_quiet = draft_of(&base, &quiet);
    assert_eq!(d_quiet.plan.items.len(), 1, "a folder where nothing is recent is still offered: {:#?}", d_quiet.not_included);
    let d = draft_of(&base, &recent);
    let why = d.not_included.iter().find(|n| n.path.ends_with("DiagOutputDir")).map(|n| n.why_not.clone());
    guard_assert!(
        "older-than",
        d.plan.items.is_empty() && matches!(why.as_deref(), Some([WhyNot::ConditionNotMet { .. }])),
        "a folder with a file changed recently is not offered: {:#?} {:#?}",
        d.plan.items,
        why
    );
    // The same folder is offered once the recent file is as old as the rest.
    touch(&recent.join("logs").join("deep").join("trace.etl"), std::time::SystemTime::now());
    let d = draft_of(&base, &recent);
    assert_eq!(d.plan.items.len(), 1, "{:#?}", d.not_included);
    wdc_testkit::remove_tree(&base);
}

/// A link inside the folder is not followed: its target's recent content does not count.
#[cfg(unix)]
#[test]
fn the_age_walk_does_not_follow_links() {
    let base = fresh_dir("temp-link");
    let t = temp_dir(&base);
    let outside = base.join("outside");
    write(&outside.join("busy.log"), 8192);
    touch(&outside.join("busy.log"), std::time::SystemTime::now() + std::time::Duration::from_secs(3 * 86_400));
    let item = t.join("with-link");
    write(&item.join("a.tmp"), 8192);
    std::os::unix::fs::symlink(&outside, item.join("to-outside")).unwrap();
    let d = draft_of(&base, &item);
    assert_eq!(d.plan.items.len(), 1, "the link's target is not walked: {:#?}", d.not_included);
    wdc_testkit::remove_tree(&base);
}
