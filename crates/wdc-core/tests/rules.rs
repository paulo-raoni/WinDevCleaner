//! Slice 3 (plan, section 8): fixture trees per rule, the Cowork case, and a local rule that
//! cannot make an item actionable. The fixtures stand in for the user's folders through the
//! environment variables the rules use, so the built-in rules run unchanged.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use wdc_core::explain::{explain, unknown_large, WhyNot};
use wdc_core::rules::{Action, Class, Context, Found, Origin, RuleSet};
use wdc_testkit::{fresh_dir, guard_assert, guard_assert_eq};

const MIB: usize = 1024 * 1024;

struct Fixture {
    base: PathBuf,
    home: PathBuf,
    local: PathBuf,
    roaming: PathBuf,
    program_data: PathBuf,
    windir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        // The real long path (the runner's TEMP uses a short name such as RUNNER~1, and
        // explain compares with the final path Windows resolves).
        let base = std::fs::canonicalize(fresh_dir(name)).unwrap();
        let text = base.to_string_lossy().into_owned();
        let base = PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text));
        let f = Fixture {
            home: base.join("home"),
            local: base.join("home").join("AppData").join("Local"),
            roaming: base.join("home").join("AppData").join("Roaming"),
            program_data: base.join("ProgramData"),
            windir: base.join("Windows"),
            base,
        };
        for d in [&f.home, &f.local, &f.roaming, &f.program_data, &f.windir] {
            std::fs::create_dir_all(d).unwrap();
        }
        f
    }

    fn ctx(&self, running: &[&str], nvidia: Option<&str>) -> Context {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let env: HashMap<String, String> = [
            ("USERPROFILE", s(&self.home)),
            ("LOCALAPPDATA", s(&self.local)),
            ("APPDATA", s(&self.roaming)),
            ("TEMP", s(&self.local.join("Temp"))),
            ("ProgramData", s(&self.program_data)),
            ("WINDIR", s(&self.windir)),
            ("SystemDrive", s(&self.base)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let running: HashSet<String> = running.iter().map(|s| s.to_string()).collect();
        Context::new(env, Some(running), nvidia.map(str::to_string))
    }
}

fn write(path: &Path, len: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let data: Vec<u8> = (0..len).map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8).collect();
    std::fs::write(path, data).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[test]
fn vscode_obsolete_extension_needs_the_obsolete_list_no_reference_and_vscode_closed() {
    let f = Fixture::new("rule-vscode-obsolete");
    let ext = f.home.join(".vscode").join("extensions");
    write(&ext.join("ms-python.python-2024.1.0").join("out").join("main.js"), 2 * MIB);
    write(&ext.join("ms-python.python-2024.2.0").join("out").join("main.js"), MIB);
    write(&ext.join("rust-lang.rust-analyzer-0.3.1").join("server.exe"), MIB);
    std::fs::write(
        ext.join(".obsolete"),
        r#"{"ms-python.python-2024.1.0": true, "rust-lang.rust-analyzer-0.3.1": true}"#,
    )
    .unwrap();
    // rust-analyzer is listed but still referenced: VS Code did not finish switching.
    std::fs::write(
        ext.join("extensions.json"),
        r#"[{"identifier":{"id":"ms-python.python"},"version":"2024.2.0","relativeLocation":"ms-python.python-2024.2.0"},
            {"identifier":{"id":"rust-lang.rust-analyzer"},"location":{"path":"/c:/x/rust-lang.rust-analyzer-0.3.1"}}]"#,
    )
    .unwrap();
    let rules = RuleSet::built_in();
    let closed = f.ctx(&[], None);

    let e = explain(&s(&ext.join("ms-python.python-2024.1.0").join("out").join("main.js")), &rules, &closed, None);
    let rule = e.rule.as_ref().expect("a rule applies");
    assert_eq!(rule.id, "vscode-obsolete-extension");
    assert!(e.item.ends_with("ms-python.python-2024.1.0"), "the item is the extension folder: {}", e.item);
    assert_eq!(rule.class, Class::InstallLeftover);
    assert!(e.actionable, "{:?}", e.why_not);
    assert!(e.expected_freed_bytes >= 2 * MIB as u64);

    // Still referenced by extensions.json: the rule applies, the item is not eligible.
    let e = explain(&s(&ext.join("rust-lang.rust-analyzer-0.3.1")), &rules, &closed, None);
    assert_eq!(e.rule.as_ref().unwrap().id, "vscode-obsolete-extension");
    guard_assert!("not-referenced-by", !e.actionable, "a referenced folder is not eligible: {:?}", e.require_conditions);
    assert!(e.require_conditions.iter().any(|c| matches!(c.found, Found::Referenced { .. })));

    // VS Code running: not eligible.
    let open = f.ctx(&["Code.exe"], None);
    let e = explain(&s(&ext.join("ms-python.python-2024.1.0")), &rules, &open, None);
    guard_assert!("process-not-running", !e.actionable, "VS Code is running: {:?}", e.require_conditions);
    assert_eq!(e.running_owners, vec!["Code.exe".to_string()]);

    // Not listed: an installed extension, managed by VS Code.
    let e = explain(&s(&ext.join("ms-python.python-2024.2.0")), &rules, &closed, None);
    guard_assert_eq!("listed-in-file", e.rule.as_ref().unwrap().id.as_str(), "vscode-installed-extension", "an installed extension is not obsolete");
    assert_eq!(e.rule.as_ref().unwrap().class, Class::AppManaged);
    assert!(!e.actionable);
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn vscode_guid_staging_folder_needs_a_day_and_no_reference() {
    let f = Fixture::new("rule-vscode-guid");
    let ext = f.home.join(".vscode").join("extensions");
    let guid = ext.join(".3f2504e0-4f89-11d3-9a0c-0305e82c3301");
    write(&guid.join("extension").join("package.json"), MIB);
    write(&ext.join("not-a-guid").join("x.bin"), MIB);
    std::fs::write(ext.join("extensions.json"), "[]").unwrap();
    std::fs::write(ext.join(".obsolete"), "{}").unwrap();
    let rules = RuleSet::built_in();

    // Two days later the interrupted install is a leftover.
    let mut later = f.ctx(&[], None);
    later.now = SystemTime::now() + Duration::from_secs(2 * 86_400);
    let e = explain(&s(&guid), &rules, &later, None);
    guard_assert_eq!("name-matches-regex", e.rule.as_ref().map(|r| r.id.as_str()), Some("vscode-staging-folder"), "GUID-named folder");
    assert!(e.actionable, "{:?}", e.why_not);

    // Created just now: it may be an install in progress.
    let now = f.ctx(&[], None);
    let e = explain(&s(&guid), &rules, &now, None);
    assert_eq!(e.rule.as_ref().unwrap().id, "vscode-staging-folder");
    guard_assert!("older-than", !e.actionable, "a fresh staging folder is not eligible: {:?}", e.require_conditions);

    // Any other name is not a staging folder.
    let e = explain(&s(&ext.join("not-a-guid")), &rules, &later, None);
    guard_assert_eq!("name-matches-regex", e.rule.as_ref().map(|r| r.id.as_str()), Some("vscode-installed-extension"), "not a GUID");
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn vscode_family_caches_are_regenerable_while_the_app_is_closed() {
    let f = Fixture::new("rule-vscode-caches");
    let rules = RuleSet::built_in();
    for (app, dir, process) in [("vscode", "Code", "Code.exe"), ("cursor", "Cursor", "Cursor.exe")] {
        let cache = f.roaming.join(dir).join("Code Cache").join("js");
        write(&cache.join("index"), MIB);
        let e = explain(&s(&cache.join("index")), &rules, &f.ctx(&[], None), None);
        let r = e.rule.as_ref().unwrap();
        assert_eq!(r.id, format!("{app}-caches"));
        assert_eq!(r.class, Class::RegenerableCache);
        assert!(e.item.ends_with("Code Cache"), "{}", e.item);
        assert!(e.actionable, "{:?}", e.why_not);
        let e = explain(&s(&cache), &rules, &f.ctx(&[process], None), None);
        assert!(!e.actionable);
    }
    // workspaceStorage holds state: report-only.
    let ws = f.roaming.join("Code").join("User").join("workspaceStorage").join("abc");
    write(&ws.join("state.vscdb"), MIB);
    let e = explain(&s(&ws), &rules, &f.ctx(&[], None), None);
    assert_eq!(e.rule.as_ref().unwrap().id, "vscode-workspace-storage");
    assert!(e.why_not.contains(&WhyNot::ReportOnly));
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn nvidia_artifacts_older_equal_and_newer_than_the_installed_driver() {
    let f = Fixture::new("rule-nvidia");
    let ota = f.program_data.join("NVIDIA Corporation").join("NVIDIA app").join("UpdateFramework").join("ota-artifacts");
    for name in ["NVIDIA-552.22-dch", "NVIDIA-560.94-dch", "NVIDIA-566.14-dch"] {
        write(&ota.join(name).join("setup.exe"), MIB);
    }
    // A hashed folder: the version is in a file name inside it.
    write(&ota.join("a1b2c3d4").join("566.36-desktop-win11-64bit-dch.exe"), MIB);
    let rules = RuleSet::built_in();
    // Windows writes NVIDIA 560.94 as 32.0.15.6094.
    let ctx = f.ctx(&[], Some("32.0.15.6094"));
    let check = |name: &str| {
        let e = explain(&s(&ota.join(name)), &rules, &ctx, None);
        assert_eq!(e.rule.as_ref().unwrap().id, "nvidia-ota-artifacts");
        assert_eq!(e.rule.as_ref().unwrap().action, Action::ReportOnly, "report-only in version 1");
        assert!(!e.actionable, "never actionable in version 1");
        e.require_conditions[0].clone()
    };
    let older = check("NVIDIA-552.22-dch");
    guard_assert!("driver-version", older.passed, "an older artifact is a leftover: {:?}", older.found);
    assert!(matches!(older.found, Found::DriverOlder { .. }));
    let equal = check("NVIDIA-560.94-dch");
    guard_assert!("driver-version", equal.passed, "the installed driver's artifact is a leftover: {:?}", equal.found);
    assert!(matches!(equal.found, Found::DriverEqual { .. }));
    let newer = check("NVIDIA-566.14-dch");
    assert!(!newer.passed, "a newer artifact may be a pending update");
    assert_eq!(newer.found, Found::DriverNewer { artifact: "566.14".into(), installed: "560.94".into() });
    let inside = check("a1b2c3d4");
    assert_eq!(inside.found, Found::DriverNewer { artifact: "566.36".into(), installed: "560.94".into() });
    // No driver installed: nothing is a leftover.
    let e = explain(&s(&ota.join("NVIDIA-552.22-dch")), &rules, &f.ctx(&[], None), None);
    assert_eq!(e.require_conditions[0].found, Found::NoDriver);
    wdc_testkit::remove_tree(&f.base);
}

/// A fixture whose folder name holds an apostrophe, so the path needs PowerShell quoting.
fn nvidia_ota(f: &Fixture) -> PathBuf {
    f.program_data.join("NVIDIA Corporation").join("NVIDIA app").join("UpdateFramework").join("ota-artifacts")
}

#[test]
fn nvidia_older_and_equal_artifacts_come_with_the_exact_manual_steps() {
    let f = Fixture::new("rule-nvidia-steps-it's");
    let ota = nvidia_ota(&f);
    for name in ["NVIDIA-552.22-dch", "NVIDIA-560.94-dch"] {
        write(&ota.join(name).join("setup.exe"), MIB);
    }
    let rules = RuleSet::built_in();
    let ctx = f.ctx(&[], Some("32.0.15.6094"));
    for name in ["NVIDIA-552.22-dch", "NVIDIA-560.94-dch"] {
        let item = ota.join(name);
        assert!(s(&item).contains('\''), "the fixture path holds an apostrophe: {}", s(&item));
        // Asked about the folder, and about a file inside it: the same item, the same steps.
        for asked in [item.clone(), item.join("setup.exe")] {
            let e = explain(&s(&asked), &rules, &ctx, None);
            assert!(!e.actionable, "{name}: report-only stays: never actionable");
            assert_eq!(e.rule.as_ref().unwrap().action, Action::ReportOnly);
            let steps = e.manual_steps.clone().unwrap_or_else(|| panic!("{name}: manual steps expected"));
            let command = format!("Remove-Item -LiteralPath '{}' -Recurse -Force", s(&item).replace('\'', "''"));
            assert!(command.contains("''"), "{command}");
            assert_eq!(steps.len(), 5, "{steps:?}");
            assert_eq!(steps[0], "Close the NVIDIA app.");
            assert_eq!(steps[1], "Open PowerShell as administrator.");
            assert_eq!(steps[3], command, "the command is its own element, exactly");
            assert!(steps[4].contains("wdc explain") && steps[4].contains(&s(&ota)), "{steps:?}");
            // The JSON the agents read carries the same array.
            let json = serde_json::to_value(&e).unwrap();
            assert_eq!(json["manual_steps"], serde_json::json!(steps));
            // The wording follows the language; the command line never does.
            let pt = e.manual.as_ref().unwrap().steps(wdc_core::i18n::Lang::PtBr);
            assert_ne!(pt[0].text, steps[0]);
            assert_eq!(pt[2].command.as_deref(), Some(command.as_str()));
        }
    }
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn nvidia_newer_unknown_and_unversioned_artifacts_get_no_command() {
    let f = Fixture::new("rule-nvidia-nosteps");
    let ota = nvidia_ota(&f);
    write(&ota.join("NVIDIA-566.14-dch").join("setup.exe"), MIB);
    write(&ota.join("NVIDIA-552.22-dch").join("setup.exe"), MIB);
    write(&ota.join("no-version-here").join("blob.bin"), MIB);
    let rules = RuleSet::built_in();
    let installed = f.ctx(&[], Some("32.0.15.6094"));
    let none = f.ctx(&[], None);
    let cases = [
        ("a newer artifact", "NVIDIA-566.14-dch", &installed, "pending update"),
        ("no installed driver", "NVIDIA-552.22-dch", &none, "could not be compared"),
        ("no version in the name", "no-version-here", &installed, "could not be compared"),
    ];
    for (what, name, ctx, reason) in cases {
        let e = explain(&s(&ota.join(name)), &rules, ctx, None);
        assert!(!e.actionable, "{what}");
        let steps = e.manual_steps.clone().unwrap_or_else(|| panic!("{what}: a step saying not to delete it"));
        guard_assert!("nvidia-steps", !steps.iter().any(|x| x.contains("Remove-Item")), "{what}: no delete command may be given: {steps:?}");
        assert_eq!(steps.len(), 1, "{what}: {steps:?}");
        assert!(steps[0].starts_with("Do not delete") && steps[0].contains(reason), "{what}: the step names the reason: {steps:?}");
        assert!(matches!(e.manual, Some(wdc_core::explain::ManualSteps::Keep { .. })), "{what}");
    }
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn only_the_nvidia_artifacts_have_manual_steps() {
    let f = Fixture::new("rule-nosteps-others");
    let rules = RuleSet::built_in();
    let ctx = f.ctx(&[], Some("32.0.15.6094"));
    let cache = f.roaming.join("Code").join("Code Cache");
    write(&cache.join("index"), MIB);
    let steam = f.local.join("Steam").join("htmlcache");
    write(&steam.join("Cache").join("data_1"), MIB);
    let dx = f.local.join("NVIDIA").join("DXCache");
    write(&dx.join("a.nvph"), MIB);
    for (what, path) in [("vscode-caches", &cache), ("steam-htmlcache", &steam), ("nvidia-shader-caches", &dx)] {
        let e = explain(&s(path), &rules, &ctx, None);
        assert_eq!(e.rule.as_ref().unwrap().id, what);
        assert!(e.manual.is_none() && e.manual_steps.is_none(), "{what}: no manual steps");
        let json = serde_json::to_value(&e).unwrap();
        assert!(json.get("manual_steps").is_none(), "{what}: the field is absent, not empty");
    }
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn shader_caches_steam_claude_and_windows_rules() {
    let f = Fixture::new("rule-others");
    let rules = RuleSet::built_in();
    let ctx = f.ctx(&[], None);

    let dx = f.local.join("NVIDIA").join("DXCache");
    write(&dx.join("a.nvph"), MIB);
    let e = explain(&s(&dx), &rules, &ctx, None);
    assert_eq!(e.rule.as_ref().unwrap().id, "nvidia-shader-caches");
    assert!(e.actionable, "{:?}", e.why_not);
    assert!(e.rule.as_ref().unwrap().instructions.as_deref().unwrap().contains("Shader Cache Size"));

    let steam = f.local.join("Steam").join("htmlcache");
    write(&steam.join("Cache").join("data_1"), MIB);
    assert!(explain(&s(&steam), &rules, &ctx, None).actionable);
    assert!(!explain(&s(&steam), &rules, &f.ctx(&["steam.exe"], None), None).actionable);

    // Claude desktop: app-managed, report-only, with the app's own steps.
    let bundle = f.local.join("Claude-Data").join("vm").join("bundle.bin");
    write(&bundle, MIB);
    let e = explain(&s(&bundle), &rules, &ctx, None);
    let r = e.rule.as_ref().unwrap();
    assert_eq!((r.id.as_str(), r.class, r.action), ("claude-desktop-data", Class::AppManaged, Action::ReportOnly));
    assert!(e.item.ends_with("Claude-Data"));
    assert!(r.instructions.as_deref().unwrap().contains("Clear Cache and Restart"));
    assert!(!e.actionable);

    // Windows: never touched, with Windows' own tool as advice.
    let winsxs = f.windir.join("WinSxS").join("amd64_x");
    write(&winsxs.join("f.dll"), MIB);
    let e = explain(&s(&winsxs), &rules, &ctx, None);
    let r = e.rule.as_ref().unwrap();
    assert_eq!((r.id.as_str(), r.class), ("windows-component-store", Class::NeverTouch));
    assert!(e.why_not.contains(&WhyNot::NeverTouch));
    assert!(r.instructions.as_deref().unwrap().contains("StartComponentCleanup"));
    let e = explain(&s(&f.windir.join("System32").join("x.dll")), &rules, &ctx, None);
    assert_eq!(e.rule.as_ref().unwrap().id, "windows");
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn a_local_rule_cannot_make_an_item_actionable() {
    let f = Fixture::new("rule-local");
    let data = f.base.join("data");
    std::fs::create_dir_all(data.join("rules")).unwrap();
    // An agent writes a rule that calls its target a cache and asks for quarantine.
    std::fs::write(
        data.join("rules").join("agent.toml"),
        r#"
[[rule]]
id = "agent-says-cache"
title = "Totally a cache"
title_pt_br = "Com certeza um cache"
class = "regenerable-cache"
action = "quarantine"
paths = ['%LOCALAPPDATA%\MyApp\Data']
evidence = "trust me"
evidence_pt_br = "confie"
source = "the agent"
cost = "none"
cost_pt_br = "nenhum"
last_verified = "2026-11-08"
"#,
    )
    .unwrap();
    let target = f.local.join("MyApp").join("Data");
    write(&target.join("important.db"), MIB);
    let rules = RuleSet::with_local(&data);
    assert!(rules.local_errors.is_empty(), "{:?}", rules.local_errors);
    let e = explain(&s(&target), &rules, &f.ctx(&[], None), Some(&s(&data)));
    let r = e.rule.as_ref().expect("the local rule explains the folder");
    assert_eq!(r.id, "agent-says-cache");
    assert!(matches!(r.origin, Origin::Local(_)));
    guard_assert_eq!("local-report-only", r.action, Action::ReportOnly, "a local rule is always report-only");
    guard_assert!("local-origin", e.why_not.contains(&WhyNot::LocalRule), "only built-in rules make items actionable");
    assert!(!e.actionable);
    // The same folder without the local rule is unknown.
    let e = explain(&s(&target), &RuleSet::built_in(), &f.ctx(&[], None), None);
    assert!(e.unknown && !e.actionable);
    wdc_testkit::remove_tree(&f.base);
}

#[test]
fn large_folders_no_rule_knows_are_flagged() {
    let f = Fixture::new("rule-unknown");
    write(&f.local.join("Mystery").join("blob.bin"), 3 * MIB);
    write(&f.local.join("NVIDIA").join("DXCache").join("a.nvph"), 3 * MIB);
    let report = wdc_core::scan::scan(
        &wdc_core::scan::ScanOptions { roots: vec![f.local.clone()], ..Default::default() },
        None,
        None,
    );
    let flagged = unknown_large(&report, &RuleSet::built_in(), &f.ctx(&[], None), 2 * MIB as u64);
    let names: Vec<String> = flagged.iter().map(|(p, _)| p.clone()).collect();
    assert!(names.iter().any(|p| p.ends_with("Mystery")), "{names:?}");
    guard_assert!("unknown-flag", !names.iter().any(|p| p.ends_with("DXCache")), "a known cache is not unknown: {names:?}");
    // Only the deepest folders: not every parent of the mystery.
    assert!(!names.iter().any(|p| p.ends_with("Local")), "{names:?}");
    wdc_testkit::remove_tree(&f.base);
}

/// The Cowork case of 2026-11-07: a large file with two names, one outside the item. Deleting
/// the item would free nothing of it. Windows only (hard links and their names).
#[cfg(windows)]
#[test]
fn the_cowork_case_two_names_one_outside_expected_freed_zero() {
    let f = Fixture::new("rule-cowork");
    let item = f.local.join("AnthropicClaude").join("vm_bundles").join("claudevm.bundle");
    let inside = item.join("rootfs.vhdx.part");
    std::fs::create_dir_all(&item).unwrap();
    wdc_testkit::write_file(&inside, 64 * MIB as u64);
    let outside = f.local.join("Claude-Data").join("vm_bundles").join("rootfs.vhdx.part");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::hard_link(&inside, &outside).unwrap();
    write(&item.join("manifest.json"), 4096);

    let rules = RuleSet::built_in();
    let e = explain(&s(&item), &rules, &f.ctx(&[], None), None);
    println!("{}", serde_json::to_string_pretty(&e).unwrap());
    assert!(e.bytes_on_disk >= 64 * MIB as u64);
    assert_eq!(e.hard_links.len(), 1);
    let h = &e.hard_links[0];
    assert_eq!(h.links, 2);
    assert_eq!(h.names.len(), 2, "every name of the file is listed: {:?}", h.names);
    assert_eq!(h.names_outside.len(), 1, "{:?}", h.names);
    assert!(h.names_outside[0].ends_with(r"Claude-Data\vm_bundles\rootfs.vhdx.part"));
    let small = e.bytes_on_disk - h.bytes_on_disk;
    guard_assert_eq!("expected-freed", e.expected_freed_bytes, small, "the 64 MB file frees nothing while its other name exists");

    // Both names inside the item: all of it is freed.
    let both = f.local.join("both");
    std::fs::create_dir_all(&both).unwrap();
    wdc_testkit::write_file(&both.join("a.bin"), 64 * MIB as u64);
    std::fs::hard_link(both.join("a.bin"), both.join("b.bin")).unwrap();
    let e = explain(&s(&both), &rules, &f.ctx(&[], None), None);
    assert_eq!(e.hard_links[0].names_outside.len(), 0, "{:?}", e.hard_links);
    assert_eq!(e.expected_freed_bytes, e.bytes_on_disk);
    wdc_testkit::remove_tree(&f.base);
}
