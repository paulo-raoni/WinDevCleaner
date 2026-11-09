//! `apply` on Windows (plan, sections 5.1 to 5.4): links, short names, trailing dots, a
//! junction into `C:\Windows`, the quarantine on another volume, and free space measured on a
//! small VHDX volume the test creates. Needs administrator rights and a desktop session (the
//! CI runner has both); run with `--ignored`.
#![cfg(windows)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use wdc_core::apply::{apply, ApplyOptions, Skip};
use wdc_core::explain::WhyNot;
use wdc_core::plan::{draft, DraftOptions, Plan, PlanItem};
use wdc_core::presence::{Method, Proof};
use wdc_core::quarantine;
use wdc_core::rules::{Class, Context, RuleSet};
use wdc_testkit::{fresh_dir, guard_assert, guard_assert_eq, hold_exclusive, junction, short_path, write_file, Vhdx};

const MIB: u64 = 1024 * 1024;

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn proof() -> Proof {
    Proof::new(Method::AppApproval)
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A fake user whose profile is `profile`; no program the rules wait for is running.
fn ctx(profile: &Path, overrides: &[(&str, String)]) -> Context {
    let mut env: HashMap<String, String> = [
        ("USERPROFILE", s(profile)),
        ("APPDATA", s(&profile.join("AppData").join("Roaming"))),
        ("LOCALAPPDATA", s(&profile.join("AppData").join("Local"))),
        ("TEMP", s(&profile.join("AppData").join("Local").join("Temp"))),
        ("WINDIR", std::env::var("WINDIR").unwrap()),
        ("ProgramFiles", std::env::var("ProgramFiles").unwrap()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    for (k, v) in overrides {
        env.insert(k.to_string(), v.clone());
    }
    Context::new(env, Some(Default::default()), None)
}

fn tree(root: &Path) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let m = std::fs::symlink_metadata(e.path()).unwrap();
            let rel = e.path().strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if m.is_dir() {
                out.insert(format!("{rel}\\"), 0);
                stack.push(e.path());
            } else {
                out.insert(rel, m.len());
            }
        }
    }
    out
}

/// A sealed one-item plan for `path` with the file ID Windows gives it now: what an agent
/// could write by hand.
fn crafted(path: &Path, rule: &str) -> Plan {
    let item = PlanItem {
        path: s(path),
        file_id: wdc_core::fsops::file_id(path).expect("the crafted item exists"),
        is_dir: true,
        large_files: vec![],
        rule_id: rule.into(),
        rule_title: rule.into(),
        class: Class::RegenerableCache,
        bytes_on_disk: 0,
        expected_freed_bytes: 0,
        files: 0,
        measured_at: now(),
    };
    Plan::new(vec![item], now())
}

fn apply_plan(plan: &Plan, c: &Context, data: &Path, purge_now: bool) -> wdc_core::apply::ApplyReport {
    apply(plan, &RuleSet::built_in(), c, data, &ApplyOptions { purge_now, explained: true }, &proof(), now()).unwrap()
}

#[test]
#[ignore]
fn on_its_own_volume_apply_frees_nothing_and_purge_frees_what_was_expected() {
    let vol = Vhdx::create("apply-measure", 512);
    let profile = vol.root.join("Users").join("me");
    let roaming = profile.join("AppData").join("Roaming");
    std::fs::create_dir_all(profile.join("AppData").join("Local").join("Temp")).unwrap();
    // A plain cache, and one whose only big file has another name outside it.
    let plain = roaming.join("Code").join("CachedData");
    std::fs::create_dir_all(plain.join("v1")).unwrap();
    write_file(&plain.join("v1").join("data.bin"), 24 * MIB);
    write_file(&plain.join("small.txt"), 3000);
    let linked = roaming.join("Code").join("GPUCache");
    std::fs::create_dir_all(&linked).unwrap();
    let outside = vol.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    write_file(&outside.join("big.bin"), 24 * MIB);
    std::fs::hard_link(outside.join("big.bin"), linked.join("big.bin")).unwrap();
    let data = fresh_dir("apply-measure-data");
    let c = ctx(&profile, &[]);

    let d = draft(&DraftOptions { paths: vec![s(&plain), s(&linked)], ..Default::default() }, &RuleSet::built_in(), &c, Some(&s(&data)), now());
    assert_eq!(d.plan.items.len(), 2, "{:#?}", d.not_included);
    let p_plain = d.plan.items.iter().find(|i| i.path.ends_with("CachedData")).unwrap().clone();
    let p_linked = d.plan.items.iter().find(|i| i.path.ends_with("GPUCache")).unwrap().clone();
    println!("plain: {} on disk, {} expected; linked: {} on disk, {} expected", p_plain.bytes_on_disk, p_plain.expected_freed_bytes, p_linked.bytes_on_disk, p_linked.expected_freed_bytes);
    assert!(p_plain.expected_freed_bytes >= 24 * MIB);
    assert_eq!(p_linked.expected_freed_bytes, 0, "the hard-linked file has a name outside: expected 0");
    let plain_tree = tree(&plain);

    // Apply: both move into W:\.wdc-quarantine (a rename cannot cross volumes); nothing is freed.
    let before = vol.free_bytes();
    let r = apply_plan(&d.plan, &c, &data, false);
    let after = vol.free_bytes();
    println!("apply: free {before} -> {after}; engine: {:?}", r.volumes);
    assert_eq!(r.moved, 2, "{:#?}", r.items);
    assert!(before.abs_diff(after) < MIB, "a rename frees nothing: {before} -> {after}");
    let held = quarantine::list(&data);
    for e in &held {
        assert!(e.stored.to_lowercase().starts_with(&format!("{}:\\.wdc-quarantine\\", vol.letter).to_lowercase()), "{e:?}");
    }
    let e_plain = held.iter().find(|e| e.original.ends_with("CachedData")).unwrap().clone();
    let e_linked = held.iter().find(|e| e.original.ends_with("GPUCache")).unwrap().clone();
    assert_eq!(tree(Path::new(&e_plain.stored)), plain_tree, "quarantine holds the same tree");

    // Purge the hard-linked item alone: expected 0, measured about 0.
    let before = vol.free_bytes();
    let r = quarantine::purge(&data, Some(std::slice::from_ref(&e_linked.id)), true, now(), &proof());
    let after = vol.free_bytes();
    println!("purge linked: free {before} -> {after}; engine: {:?}", r.volumes);
    assert!(r.items[0].purged, "{:#?}", r.items);
    assert!(after.abs_diff(before) < MIB, "deleting one name of a file with another frees nothing: {before} -> {after}");
    assert_eq!(r.volumes[0].expected_freed_bytes, 0);
    assert!(r.volumes[0].measured_freed().unwrap().unsigned_abs() < MIB, "{:?}", r.volumes);
    assert!(outside.join("big.bin").exists(), "the other name is untouched");

    // Purge the plain item: measured matches expected.
    let before = vol.free_bytes();
    let r = quarantine::purge(&data, Some(std::slice::from_ref(&e_plain.id)), true, now(), &proof());
    let after = vol.free_bytes();
    println!("purge plain: free {before} -> {after}; engine: {:?}", r.volumes);
    let freed = after as i64 - before as i64;
    assert!(freed.abs_diff(p_plain.expected_freed_bytes as i64) < MIB, "freed {freed}, expected {}", p_plain.expected_freed_bytes);
    let m = r.volumes[0].measured_freed().unwrap();
    assert!(m.abs_diff(p_plain.expected_freed_bytes as i64) < MIB, "engine measured {m}");
    assert_eq!(quarantine::held_bytes(&data), 0);
    drop(vol);
    wdc_testkit::remove_tree(&data);
}

/// Items of the shader-cache and Steam rules, each as (tag, rule id, path below the profile).
/// `nvidia-shader-caches` has three paths: all three are covered.
const SHADER_AND_STEAM: [(&str, &str, &[&str]); 4] = [
    ("dx", "nvidia-shader-caches", &["AppData", "Local", "NVIDIA", "DXCache"]),
    ("gl", "nvidia-shader-caches", &["AppData", "Local", "NVIDIA", "GLCache"]),
    ("compute", "nvidia-shader-caches", &["AppData", "Roaming", "NVIDIA", "ComputeCache"]),
    ("steam", "steam-htmlcache", &["AppData", "Local", "Steam", "htmlcache"]),
];

/// A junction swapped in for the item after planning, or planned as the junction itself, moves
/// nothing: the item (`below` the fake profile) belongs to `rule`.
fn junction_moves_nothing(tag: &str, rule: &str, below: &[&str]) {
    let base = fresh_dir(&format!("apply-junction-{tag}"));
    let profile = base.join("u");
    std::fs::create_dir_all(profile.join("AppData").join("Local").join("Temp")).unwrap();
    let cache = below.iter().fold(profile.clone(), |p, name| p.join(name));
    std::fs::create_dir_all(&cache).unwrap();
    write_file(&cache.join("a.bin"), 2 * MIB);
    let data = base.join("data");
    let c = ctx(&profile, &[]);
    // The target is inside C:\Windows.
    let target = PathBuf::from(std::env::var("WINDIR").unwrap()).join("Temp").join(format!("wdc-junction-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&target).unwrap();
    write_file(&target.join("system.dat"), 4096);

    // Swapped in after planning.
    let d = draft(&DraftOptions { paths: vec![s(&cache)], ..Default::default() }, &RuleSet::built_in(), &c, Some(&s(&data)), now());
    assert_eq!(d.plan.items.len(), 1, "{rule}: {:#?}", d.not_included);
    assert_eq!(d.plan.items[0].rule_id, rule, "{tag}");
    std::fs::rename(&cache, base.join("moved-away")).unwrap();
    junction(&cache, &target);
    let r = apply_plan(&d.plan, &c, &data, true);
    assert_eq!(r.moved, 0, "{rule} ({tag}): {:#?}", r.items);
    // Its file ID differs from the plan's too, but it is refused first because it is a link.
    guard_assert!("apply-link", matches!(r.items[0].skipped, Some(Skip::IsLink)), "{rule} ({tag}) refused as a link: {:#?}", r.items);

    // Planned as the junction itself, with its own file ID (a hand-written plan).
    let r = apply_plan(&crafted(&cache, rule), &c, &data, true);
    guard_assert_eq!("apply-link", r.moved, 0, "{rule} ({tag}): a junction is never moved: {:#?}", r.items);
    assert!(matches!(r.items[0].skipped, Some(Skip::IsLink)), "{rule} ({tag}): {:#?}", r.items);
    assert!(target.join("system.dat").exists() && cache.exists(), "{rule} ({tag}): the junction and its target are untouched");
    std::fs::remove_dir(&cache).unwrap();
    wdc_testkit::remove_tree(&target);
    wdc_testkit::remove_tree(&base);
}

#[test]
#[ignore]
fn a_junction_planned_or_swapped_in_moves_nothing() {
    junction_moves_nothing("vscode", "vscode-caches", &["AppData", "Roaming", "Code", "CachedData"]);
}

#[test]
#[ignore]
fn a_junction_swapped_in_for_a_shader_or_steam_cache_moves_nothing() {
    for (tag, rule, below) in SHADER_AND_STEAM {
        junction_moves_nothing(tag, rule, below);
    }
}

/// A program holding a file of the item with no sharing (a running game, Steam) keeps the whole
/// folder in place: apply renames the folder through one handle, and Windows refuses that while
/// a file inside is open. Nothing is moved, nothing is in quarantine; with the handle gone the
/// same plan applies, and restore gives the identical tree.
#[test]
#[ignore]
fn a_file_held_open_keeps_the_whole_folder_in_place() {
    for (tag, rule, below) in SHADER_AND_STEAM {
        let base = fresh_dir(&format!("apply-held-{tag}"));
        let profile = base.join("u");
        std::fs::create_dir_all(profile.join("AppData").join("Local").join("Temp")).unwrap();
        let item = below.iter().fold(profile.clone(), |p, name| p.join(name));
        std::fs::create_dir_all(item.join("sub")).unwrap();
        write_file(&item.join("sub").join("big.bin"), 2 * MIB);
        write_file(&item.join("held.dat"), 4096);
        write_file(&item.join("other.txt"), 100);
        let data = base.join("data");
        let c = ctx(&profile, &[]);
        let d = draft(&DraftOptions { paths: vec![s(&item)], ..Default::default() }, &RuleSet::built_in(), &c, Some(&s(&data)), now());
        assert_eq!(d.plan.items.len(), 1, "{rule} ({tag}): {:#?}", d.not_included);
        assert_eq!(d.plan.items[0].rule_id, rule, "{tag}");
        let before = tree(&item);

        let held = hold_exclusive(&item.join("held.dat"));
        let r = apply_plan(&d.plan, &c, &data, false);
        println!("{rule} ({tag}) with a file held open: {:?}", r.items[0].skipped);
        assert_eq!(r.moved, 0, "{rule} ({tag}): a held file keeps the folder in place: {:#?}", r.items);
        assert!(r.items[0].skipped.is_some(), "{rule} ({tag}): the item is reported skipped: {:#?}", r.items);
        assert_eq!(tree(&item), before, "{rule} ({tag}): the tree is identical");
        assert!(quarantine::list(&data).is_empty(), "{rule} ({tag}): nothing recorded in quarantine");
        let in_quarantine = if data.join("quarantine").exists() { tree(&data.join("quarantine")) } else { BTreeMap::new() };
        assert!(
            in_quarantine.values().all(|len| *len == 0) && !in_quarantine.keys().any(|k| k.ends_with("held.dat") || k.ends_with("big.bin")),
            "{rule} ({tag}): no copy in quarantine: {in_quarantine:?}"
        );

        // The program lets go: the same plan applies, and restore puts everything back.
        drop(held);
        let r = apply_plan(&d.plan, &c, &data, false);
        assert_eq!(r.moved, 1, "{rule} ({tag}): {:#?}", r.items);
        assert!(!item.exists(), "{rule} ({tag}): the item moved out");
        let entry = quarantine::list(&data)[0].clone();
        assert_eq!(tree(Path::new(&entry.stored)), before, "{rule} ({tag}): quarantine holds the same tree");
        quarantine::restore(&data, &entry.id, &c, &proof()).unwrap_or_else(|e| panic!("{rule} ({tag}): restore refused: {e:?}"));
        assert_eq!(tree(&item), before, "{rule} ({tag}): restore gives the identical tree");
        wdc_testkit::remove_tree(&base);
    }
}
#[test]
#[ignore]
fn forbidden_places_are_refused_however_the_path_is_written() {
    let vol = Vhdx::create("apply-forbidden", 128);
    let profile = vol.root.join("Users").join("me");
    let documents = profile.join("Documents");
    std::fs::create_dir_all(profile.join("AppData").join("Local").join("Temp")).unwrap();
    // AppData kept inside Documents: the cache rule matches there, the place is personal.
    let docs_appdata = documents.join("AppData");
    let in_docs = docs_appdata.join("Code").join("CachedData");
    std::fs::create_dir_all(&in_docs).unwrap();
    write_file(&in_docs.join("x.bin"), 4096);
    let data = fresh_dir("apply-forbidden-data");

    let mut cases: Vec<(String, PathBuf, Context)> = Vec::new();
    // 8.3 short name: DOCUME~1 is Documents.
    let short = short_path(&in_docs);
    assert!(!s(&short).contains("Documents"), "the volume gives short names: {}", short.display());
    cases.push(("short name".into(), short, ctx(&profile, &[("APPDATA", s(&docs_appdata))])));
    // Trailing dots: Windows drops them from the last name.
    cases.push(("trailing dots".into(), PathBuf::from(format!("{}...", s(&in_docs))), ctx(&profile, &[("APPDATA", s(&docs_appdata))])));
    // `..` back into Documents.
    let roaming = profile.join("AppData").join("Roaming");
    std::fs::create_dir_all(roaming.join("Code").join("CachedData")).unwrap();
    cases.push((
        "dot-dot".into(),
        roaming.join("Code").join("CachedData").join("..").join("..").join("..").join("..").join("Documents").join("AppData").join("Code").join("CachedData"),
        ctx(&profile, &[("APPDATA", s(&docs_appdata))]),
    ));
    // A junction above the item, into C:\Windows.
    let win = PathBuf::from(std::env::var("WINDIR").unwrap()).join("Temp").join(format!("wdc-forbidden-{}", std::process::id()));
    std::fs::create_dir_all(win.join("Code").join("CachedData")).unwrap();
    write_file(&win.join("Code").join("CachedData").join("system.dat"), 4096);
    let jroaming = profile.join("AppData").join("JRoaming");
    std::fs::create_dir_all(&jroaming).unwrap();
    junction(&jroaming.join("Code"), &win.join("Code"));
    cases.push(("junction into C:\\Windows".into(), jroaming.join("Code").join("CachedData"), ctx(&profile, &[("APPDATA", s(&jroaming))])));
    // A user folder planned directly.
    let desktop = profile.join("Desktop");
    std::fs::create_dir_all(&desktop).unwrap();
    cases.push(("user folder".into(), desktop.clone(), ctx(&profile, &[])));
    // An allowed root itself.
    cases.push(("allowed root".into(), roaming.join("Code").join("CachedData"), ctx(&profile, &[("TEMP", s(&roaming.join("Code").join("CachedData")))])));

    for (name, path, c) in &cases {
        let plan = crafted(path, "vscode-caches");
        let r = apply_plan(&plan, c, &data, true);
        println!("{name}: {} -> {:?}", path.display(), r.items[0].skipped);
        guard_assert_eq!("apply-verdict", r.moved, 0, "{name}: {} must not move: {:#?}", path.display(), r.items);
        let ok = matches!(&r.items[0].skipped, Some(Skip::NotAllowed { .. }) | Some(Skip::NoLongerMatches { .. }));
        assert!(ok, "{name}: {:#?}", r.items);
        if name == "user folder" || name == "short name" || name == "trailing dots" || name == "junction into C:\\Windows" {
            guard_assert!(
                "apply-verdict",
                matches!(&r.items[0].skipped, Some(Skip::NotAllowed { why: WhyNot::Forbidden { .. } | WhyNot::OutsideAllowedRoots, .. })),
                "{name}: refused for its real place: {:#?}",
                r.items
            );
        }
        if name == "allowed root" {
            assert!(matches!(&r.items[0].skipped, Some(Skip::NotAllowed { why: WhyNot::AllowedRootItself, .. })), "{:#?}", r.items);
        }
    }
    assert!(in_docs.join("x.bin").exists() && win.join("Code").join("CachedData").join("system.dat").exists() && desktop.exists());
    std::fs::remove_dir(jroaming.join("Code")).unwrap();
    wdc_testkit::remove_tree(&win);
    drop(vol);
    wdc_testkit::remove_tree(&data);
}

#[test]
#[ignore]
fn a_quarantine_folder_that_is_a_link_is_refused() {
    let base = fresh_dir("apply-qlink");
    let profile = base.join("u");
    std::fs::create_dir_all(profile.join("AppData").join("Local").join("Temp")).unwrap();
    let cache = profile.join("AppData").join("Roaming").join("Code").join("CachedData");
    std::fs::create_dir_all(&cache).unwrap();
    write_file(&cache.join("a.bin"), 2 * MIB);
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let elsewhere = base.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    junction(&data.join("quarantine"), &elsewhere);
    let c = ctx(&profile, &[]);
    let d = draft(&DraftOptions { paths: vec![s(&cache)], ..Default::default() }, &RuleSet::built_in(), &c, Some(&s(&data)), now());
    assert_eq!(d.plan.items.len(), 1, "{:#?}", d.not_included);
    let r = apply_plan(&d.plan, &c, &data, false);
    guard_assert_eq!("quarantine-plain", r.moved, 0, "a quarantine folder that is a link is refused: {:#?}", r.items);
    assert!(matches!(r.items[0].skipped, Some(Skip::Quarantine { .. })), "{:#?}", r.items);
    assert!(cache.join("a.bin").exists());
    std::fs::remove_dir(data.join("quarantine")).unwrap();
    wdc_testkit::remove_tree(&base);
}
