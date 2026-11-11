//! Installed programs (plan, sections 5.5 and 8, slice 7): the uninstall keys read back exactly,
//! patches skipped, duplicates merged, and the folders an uninstall left behind found by exact
//! name only. The pure parts run everywhere; the registry fixtures run on the Windows runner.

use std::path::PathBuf;

use wdc_core::programs::{
    leftovers, list_children, load_list, look, parse_install_date, programs_from_raw, remember, save_list, size_bytes,
    Child, Field, Program, RawEntry, Remembered, Scope, View,
};
use wdc_testkit::{fresh_dir, guard_assert};

fn raw(key: &str, name: Option<&str>) -> RawEntry {
    RawEntry {
        key_name: key.to_string(),
        scope: Some(Scope::Machine),
        view: Some(View::X64),
        display_name: name.map(String::from),
        ..Default::default()
    }
}

fn kept(entries: Vec<RawEntry>) -> Vec<String> {
    programs_from_raw(entries).into_iter().map(|p| p.key_name).collect()
}

#[test]
fn entries_that_are_not_programs_are_skipped() {
    let plain = raw("plain", Some("Plain"));
    let mut system = raw("system", Some("System part"));
    system.system_component = Some(1);
    let mut not_system = raw("not-system", Some("Not a system part"));
    not_system.system_component = Some(0);
    let mut child = raw("child", Some("Patch of something"));
    child.parent_key_name = Some("OperatingSystem".into());
    let mut patches = Vec::new();
    for (i, kind) in ["Update", "Hotfix", "Security Update", "update", "SECURITY UPDATE"].iter().enumerate() {
        let mut p = raw(&format!("patch-{i}"), Some("A patch"));
        p.release_type = Some(kind.to_string());
        patches.push(p);
    }
    let mut other_release = raw("release-other", Some("Released as something else"));
    other_release.release_type = Some("Application".into());
    let empty_name = raw("empty-name", Some("   "));
    let no_name = raw("no-name", None);

    let mut all = vec![plain, system, not_system, child, other_release, empty_name, no_name];
    all.extend(patches);
    let mut names = kept(all);
    names.sort();
    assert_eq!(names, vec!["not-system", "plain", "release-other"]);
}

#[test]
fn missing_values_are_absent_never_empty_strings() {
    let mut e = raw("k", Some("Name"));
    e.publisher = Some(String::new());
    e.display_version = Some("  ".into());
    e.install_location = Some(String::new());
    e.uninstall_string = None;
    e.quiet_uninstall_string = Some(String::new());
    let p = programs_from_raw(vec![e]).remove(0);
    assert_eq!(p.publisher, None);
    assert_eq!(p.version, None);
    assert_eq!(p.install_location, None);
    assert_eq!(p.uninstall_command, None);
    assert_eq!(p.quiet_uninstall_command, None);
    assert_eq!(p.reported_size_bytes, None);
    assert_eq!(p.install_date, None);
}

#[test]
fn dates_and_sizes_are_parsed() {
    assert_eq!(parse_install_date("20240131").as_deref(), Some("2024-01-31"));
    assert_eq!(parse_install_date("20240229").as_deref(), Some("2024-02-29"));
    assert_eq!(parse_install_date("19991231").as_deref(), Some("1999-12-31"));
    for bad in ["", "2024", "2024-01-31", "20241301", "20240001", "20240132", "20230229", "20240431", "2024013a", "202401311", "00000101"] {
        assert_eq!(parse_install_date(bad), None, "{bad:?} is malformed");
    }
    // EstimatedSize is KB; the tool gives bytes.
    assert_eq!(size_bytes(0), 0);
    assert_eq!(size_bytes(1), 1024);
    assert_eq!(size_bytes(u32::MAX), u32::MAX as u64 * 1024);
    let mut e = raw("k", Some("Sized"));
    e.estimated_size_kb = Some(2048);
    e.install_date = Some("not a date".into());
    let p = programs_from_raw(vec![e]).remove(0);
    assert_eq!(p.reported_size_bytes, Some(2 * 1024 * 1024));
    assert_eq!(p.install_date, None);
}

#[test]
fn the_same_program_in_two_places_is_one_program() {
    let mut a = raw("a", Some("Tool"));
    a.display_version = Some("1.0".into());
    a.uninstall_string = Some("tool-uninstall.exe".into());
    a.scope = Some(Scope::Machine);
    a.view = Some(View::X64);
    let mut b = a.clone();
    b.key_name = "b".into();
    b.view = Some(View::X32);
    let mut c = a.clone();
    c.key_name = "c".into();
    c.scope = Some(Scope::User);
    // Another version, or another uninstall command, is another program.
    let mut other_version = a.clone();
    other_version.key_name = "v2".into();
    other_version.display_version = Some("2.0".into());
    let mut other_command = a.clone();
    other_command.key_name = "cmd".into();
    other_command.uninstall_string = Some("other.exe".into());
    let list = programs_from_raw(vec![a, b, c, other_version, other_command]);
    assert_eq!(list.len(), 3, "{list:?}");
    assert_eq!(list.iter().filter(|p| p.version.as_deref() == Some("1.0") && p.uninstall_command.as_deref() == Some("tool-uninstall.exe")).count(), 1);
}

#[test]
fn programs_are_ordered_by_reported_size_then_by_name() {
    let mut entries = Vec::new();
    for (key, name, kb) in [
        ("small", "Small", Some(10)),
        ("zeta", "zeta unsized", None),
        ("big", "Big", Some(5000)),
        ("alpha", "Alpha unsized", None),
        ("mid-b", "Mid B", Some(300)),
        ("mid-a", "mid a", Some(300)),
        ("huge", "Huge", Some(u32::MAX)),
    ] {
        let mut e = raw(key, Some(name));
        e.estimated_size_kb = kb;
        entries.push(e);
    }
    assert_eq!(kept(entries), vec!["huge", "big", "mid-a", "mid-b", "small", "alpha", "zeta"]);
}

fn rem(name: &str, publisher: Option<&str>) -> Remembered {
    Remembered { name: name.to_string(), publisher: publisher.map(String::from), scope: Scope::User }
}

fn child(name: &str) -> Child {
    Child { root: PathBuf::from("roots").join("appdata"), name: name.to_string() }
}

fn folders(found: &[wdc_core::programs::Leftover]) -> Vec<String> {
    let mut v: Vec<String> = found.iter().map(|l| l.folder.clone()).collect();
    v.sort();
    v
}

#[test]
fn a_folder_named_exactly_like_the_program_is_reported_and_a_longer_or_shorter_name_is_not() {
    let previous = vec![rem("Foo", Some("Acme Tools"))];
    let current: Vec<Remembered> = Vec::new();
    let children: Vec<Child> =
        ["FOO", "Foo Bar", "FooBar", "Foo 2", "Fo", "oo", "xFoo", "Foo ", " Foo", "Foo-", "The Foo", "Acme"].iter().map(|n| child(n)).collect();
    let found = leftovers(&previous, &current, &children);

    let exact = child("FOO").path().display().to_string();
    let mut failures = Vec::new();
    if !found.iter().any(|l| l.folder == exact && l.matched == Field::Name) {
        failures.push(format!("the folder named like the program was not reported: {found:?}"));
    }
    guard_assert!("leftover-exact", failures.is_empty(), "{failures:?}");
    // Only the exact one: a prefix, a suffix, a number or a space never matches.
    assert_eq!(folders(&found), vec![exact], "{found:?}");
    let l = &found[0];
    assert_eq!(l.program, "Foo");
    assert_eq!(l.class, wdc_core::rules::Class::InstallLeftover);
    assert!(l.candidate && !l.actionable);
    assert!(l.note.contains("person must confirm"), "{}", l.note);
    assert!(l.note.contains("wdc explain"), "{}", l.note);
    assert!(l.explain.starts_with("wdc explain "), "{}", l.explain);
}

#[test]
fn a_publisher_folder_is_not_reported_while_another_program_has_that_publisher() {
    let previous = vec![rem("Tool One", Some("Acme"))];
    let children = vec![child("ACME"), child("Acme Inc"), child("Tool One")];

    // Another installed program still has the publisher: the folder is still in use.
    let still = vec![rem("Tool Two", Some("acme"))];
    let found = leftovers(&previous, &still, &children);
    let reported_publisher = found.iter().any(|l| l.matched == Field::Publisher);
    guard_assert!("leftover-publisher", !reported_publisher, "the publisher folder was reported while Tool Two still has the publisher: {found:?}");
    // The folder named like the program itself is still a candidate.
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].matched, Field::Name);

    // No program has the publisher any more: the folder is reported, once.
    let found = leftovers(&previous, &[], &children);
    let publisher: Vec<_> = found.iter().filter(|l| l.matched == Field::Publisher).collect();
    assert_eq!(publisher.len(), 1, "{found:?}");
    assert_eq!(publisher[0].folder, child("ACME").path().display().to_string());
    assert_eq!(publisher[0].program, "Tool One");
    // `Acme Inc` is not `Acme`.
    assert_eq!(found.len(), 2, "{found:?}");

    // Two uninstalled programs of one publisher still give one folder, once.
    let previous = vec![rem("Tool One", Some("Acme")), rem("Tool Two", Some("Acme"))];
    let found = leftovers(&previous, &[], &[child("Acme")]);
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn a_program_in_both_lists_produces_no_leftover() {
    let both = vec![rem("Foo", Some("Acme")), rem("Bar", None)];
    let children = vec![child("Foo"), child("Acme"), child("Bar")];
    assert!(leftovers(&both, &both, &children).is_empty());
    // The same program, written in another case, is still installed.
    let now = vec![rem("FOO", Some("ACME")), rem("bar", None)];
    assert!(leftovers(&both, &now, &children).is_empty());
    // Only what is gone leaves candidates.
    let now = vec![rem("Foo", Some("Acme"))];
    let found = leftovers(&both, &now, &children);
    assert_eq!(folders(&found), vec![child("Bar").path().display().to_string()]);
    // A program that was not in the previous list is never a leftover.
    assert!(leftovers(&[], &now, &children).is_empty());
}

#[test]
fn only_direct_children_of_the_three_roots_are_looked_at() {
    let base = fresh_dir("programs-roots");
    let appdata = base.join("Roaming");
    let local = base.join("Local");
    let programs = local.join("Programs");
    for d in [
        appdata.join("Foo").join("Foo"),
        programs.join("Bar"),
        local.join("Bar"),
        local.join("Deep").join("Bar"),
        local.join("Deep").join("Foo"),
        base.join("Elsewhere").join("Foo"),
        base.join("Elsewhere").join("Bar"),
    ] {
        std::fs::create_dir_all(d).unwrap();
    }
    // A file with a program's name is not a folder.
    std::fs::write(appdata.join("Baz"), b"x").unwrap();

    let previous = vec![rem("Foo", None), rem("Bar", None), rem("Baz", None)];
    let found = leftovers(&previous, &[], &list_children(&[appdata.clone(), programs.clone(), local.clone()]));
    let mut expected = vec![
        appdata.join("Foo").display().to_string(),
        programs.join("Bar").display().to_string(),
        local.join("Bar").display().to_string(),
    ];
    expected.sort();
    assert_eq!(folders(&found), expected, "{found:?}");
    // A root that does not exist gives nothing.
    assert!(list_children(&[base.join("Missing")]).is_empty());
}

#[test]
fn the_list_of_a_look_is_saved_and_compared_at_the_next_one() {
    let data = fresh_dir("programs-list");
    assert!(load_list(&data).is_none());
    let list = vec![rem("Foo", Some("Acme")), rem("Bar", None)];
    save_list(&data, &list).unwrap();
    assert_eq!(load_list(&data), Some(list.clone()));

    // A look with no source (nothing installed) is the first look, then not.
    let roots = vec![data.join("nothing")];
    let data2 = fresh_dir("programs-look");
    let first = look(&[], &data2, &roots);
    assert!(first.first_look && !first.leftovers_checked && first.programs.is_empty() && first.errors.is_empty());
    assert_eq!(load_list(&data2), Some(Vec::new()));
    let second = look(&[], &data2, &roots);
    assert!(!second.first_look && second.leftovers_checked && second.leftovers.is_empty());

    // The previous list had two programs; now none are installed: their folders are candidates.
    std::fs::create_dir_all(roots[0].join("Foo")).unwrap();
    save_list(&data2, &list).unwrap();
    let third = look(&[], &data2, &roots);
    assert_eq!(third.leftovers.len(), 1, "{:?}", third.leftovers);
    let json = third.to_json();
    assert_eq!(json["leftovers"][0]["class"], "install-leftover");
    assert_eq!(json["leftovers"][0]["actionable"], false);
    assert_eq!(json["leftovers"][0]["candidate"], true);
    assert_eq!(json["count"], 0);
    assert!(json["size_note"].as_str().unwrap().contains("installer"));
    assert!(json["note"].as_str().unwrap().contains("never runs"));
    // Whatever the list said, `remember` keeps name, publisher and scope.
    let p = Program {
        key_name: "k".into(),
        scope: Scope::Machine,
        view: View::X32,
        name: "N".into(),
        publisher: Some("P".into()),
        version: None,
        install_date: None,
        reported_size_bytes: None,
        install_location: None,
        uninstall_command: None,
        quiet_uninstall_command: None,
    };
    let r = remember(std::slice::from_ref(&p));
    assert_eq!((r[0].name.as_str(), r[0].publisher.as_deref(), r[0].scope), ("N", Some("P"), Scope::Machine));
    let json = serde_json::to_value(&p).unwrap();
    assert_eq!((json["scope"].as_str(), json["view"].as_str()), (Some("machine"), Some("32")));
}

#[cfg(windows)]
mod registry {
    //! Fixture uninstall keys written under a unique test path in HKCU and HKLM (64- and 32-bit
    //! views), read back through the `sources` argument.

    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteKeyExW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, KEY_WRITE, REG_DWORD,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    use super::*;
    use wdc_core::programs::{read_programs, Hive, Source};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn flag(view: View) -> u32 {
        match view {
            View::X64 => KEY_WOW64_64KEY,
            View::X32 => KEY_WOW64_32KEY,
        }
    }

    fn root(hive: Hive) -> HKEY {
        match hive {
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }

    /// One place the fixture lives in.
    #[derive(Clone, Copy)]
    struct Place {
        hive: Hive,
        view: View,
        tag: &'static str,
    }

    const PLACES: [Place; 3] = [
        Place { hive: Hive::CurrentUser, view: View::X64, tag: "user64" },
        Place { hive: Hive::LocalMachine, view: View::X64, tag: "machine64" },
        Place { hive: Hive::LocalMachine, view: View::X32, tag: "machine32" },
    ];

    fn base() -> String {
        format!(r"SOFTWARE\WinDevCleaner-test-{}", std::process::id())
    }

    fn uninstall() -> String {
        format!(r"{}\Uninstall", base())
    }

    fn sources() -> Vec<Source> {
        PLACES.iter().map(|p| Source { hive: p.hive, path: uninstall(), view: p.view }).collect()
    }

    /// Entry names created in each place, so the guard can delete them.
    const ENTRIES: [&str; 10] =
        ["full", "minimal", "bad-date", "system-component-0", "no-name", "system-component", "parent", "update", "hotfix", "security-update"];

    /// Deletes every fixture key when dropped, also when an assertion fails.
    struct Fixture;

    impl Drop for Fixture {
        fn drop(&mut self) {
            for place in PLACES {
                let delete = |path: &str| unsafe {
                    RegDeleteKeyExW(root(place.hive), wide(path).as_ptr(), flag(place.view), 0);
                };
                for e in ENTRIES {
                    delete(&format!(r"{}\{e}", uninstall()));
                }
                delete(&uninstall());
                delete(&base());
            }
        }
    }

    fn create(place: Place, path: &str) -> HKEY {
        let mut h: HKEY = null_mut();
        let r = unsafe {
            RegCreateKeyExW(
                root(place.hive),
                wide(path).as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE | flag(place.view),
                null(),
                &mut h,
                null_mut(),
            )
        };
        assert_eq!(r, ERROR_SUCCESS, "create {path} (is the runner elevated?)");
        h
    }

    fn set_text(k: HKEY, name: &str, value: &str) {
        let data: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
        let r = unsafe { RegSetValueExW(k, wide(name).as_ptr(), 0, REG_SZ, data.as_ptr(), data.len() as u32) };
        assert_eq!(r, ERROR_SUCCESS, "set {name}");
    }

    fn set_number(k: HKEY, name: &str, value: u32) {
        let data = value.to_le_bytes();
        let r = unsafe { RegSetValueExW(k, wide(name).as_ptr(), 0, REG_DWORD, data.as_ptr(), 4) };
        assert_eq!(r, ERROR_SUCCESS, "set {name}");
    }

    fn entry(place: Place, name: &str, fill: impl FnOnce(HKEY)) {
        let k = create(place, &format!(r"{}\{name}", uninstall()));
        fill(k);
        unsafe { RegCloseKey(k) };
    }

    fn write_fixture() {
        for place in PLACES {
            let t = place.tag;
            unsafe { RegCloseKey(create(place, &uninstall())) };
            entry(place, "full", |k| {
                set_text(k, "DisplayName", &format!("WDC Test Full {t}"));
                set_text(k, "Publisher", &format!("WDC Test Publisher {t}"));
                set_text(k, "DisplayVersion", "1.2.3");
                set_text(k, "InstallDate", "20240131");
                set_number(k, "EstimatedSize", 2048);
                set_text(k, "InstallLocation", &format!(r"C:\Program Files\WDC Test {t}"));
                set_text(k, "UninstallString", &format!(r#""C:\Program Files\WDC Test {t}\uninstall.exe" /remove"#));
                set_text(k, "QuietUninstallString", &format!(r#""C:\Program Files\WDC Test {t}\uninstall.exe" /remove /quiet"#));
            });
            entry(place, "minimal", |k| set_text(k, "DisplayName", &format!("WDC Test Minimal {t}")));
            entry(place, "bad-date", |k| {
                set_text(k, "DisplayName", &format!("WDC Test Bad Date {t}"));
                set_text(k, "InstallDate", "2024-13-45");
                set_number(k, "EstimatedSize", 100);
                set_text(k, "Publisher", "");
            });
            entry(place, "system-component-0", |k| {
                set_text(k, "DisplayName", &format!("WDC Test Not A System Component {t}"));
                set_number(k, "SystemComponent", 0);
                set_number(k, "EstimatedSize", 5000);
            });
            // Skipped, one of each kind.
            entry(place, "no-name", |k| set_text(k, "Publisher", &format!("Nameless {t}")));
            entry(place, "system-component", |k| {
                set_text(k, "DisplayName", &format!("WDC Test System Component {t}"));
                set_number(k, "SystemComponent", 1);
            });
            entry(place, "parent", |k| {
                set_text(k, "DisplayName", &format!("WDC Test Patch With Parent {t}"));
                set_text(k, "ParentKeyName", "OperatingSystem");
            });
            for (name, kind) in [("update", "Update"), ("hotfix", "Hotfix"), ("security-update", "Security Update")] {
                entry(place, name, |k| {
                    set_text(k, "DisplayName", &format!("WDC Test {kind} {t}"));
                    set_text(k, "ReleaseType", kind);
                });
            }
        }
    }

    fn expected(place: Place) -> Vec<Program> {
        let t = place.tag;
        let scope = place.hive.scope();
        let p = |key: &str, name: String| Program {
            key_name: key.to_string(),
            scope,
            view: place.view,
            name,
            publisher: None,
            version: None,
            install_date: None,
            reported_size_bytes: None,
            install_location: None,
            uninstall_command: None,
            quiet_uninstall_command: None,
        };
        vec![
            Program {
                publisher: Some(format!("WDC Test Publisher {t}")),
                version: Some("1.2.3".into()),
                install_date: Some("2024-01-31".into()),
                reported_size_bytes: Some(2048 * 1024),
                install_location: Some(format!(r"C:\Program Files\WDC Test {t}")),
                uninstall_command: Some(format!(r#""C:\Program Files\WDC Test {t}\uninstall.exe" /remove"#)),
                quiet_uninstall_command: Some(format!(r#""C:\Program Files\WDC Test {t}\uninstall.exe" /remove /quiet"#)),
                ..p("full", format!("WDC Test Full {t}"))
            },
            p("minimal", format!("WDC Test Minimal {t}")),
            Program { reported_size_bytes: Some(100 * 1024), ..p("bad-date", format!("WDC Test Bad Date {t}")) },
            Program {
                reported_size_bytes: Some(5000 * 1024),
                ..p("system-component-0", format!("WDC Test Not A System Component {t}"))
            },
        ]
    }

    /// Every key and value below `path`, as sorted lines (type and bytes included).
    fn dump(place: Place, path: &str) -> Vec<String> {
        let mut lines = Vec::new();
        let mut h: HKEY = null_mut();
        let r = unsafe { RegOpenKeyExW(root(place.hive), wide(path).as_ptr(), 0, KEY_READ | flag(place.view), &mut h) };
        if r != ERROR_SUCCESS {
            return vec![format!("{path}: not there ({r})")];
        }
        for i in 0.. {
            let mut name = [0u16; 16384];
            let mut name_len = name.len() as u32;
            let mut ty = 0u32;
            let mut data = vec![0u8; 8192];
            let mut data_len = data.len() as u32;
            let r = unsafe {
                RegEnumValueW(h, i, name.as_mut_ptr(), &mut name_len, null(), &mut ty, data.as_mut_ptr(), &mut data_len)
            };
            if r == ERROR_NO_MORE_ITEMS {
                break;
            }
            assert_eq!(r, ERROR_SUCCESS, "enumerate values of {path}");
            let hex: String = data[..data_len as usize].iter().map(|b| format!("{b:02x}")).collect();
            lines.push(format!("{path} | {} | {ty} | {hex}", String::from_utf16_lossy(&name[..name_len as usize])));
        }
        let mut subs = Vec::new();
        for i in 0.. {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let r = unsafe { RegEnumKeyExW(h, i, name.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut()) };
            if r == ERROR_NO_MORE_ITEMS {
                break;
            }
            assert_eq!(r, ERROR_SUCCESS, "enumerate keys of {path}");
            subs.push(String::from_utf16_lossy(&name[..len as usize]));
        }
        unsafe { RegCloseKey(h) };
        for s in subs {
            lines.push(format!("{path} | key | {s}"));
            lines.extend(dump(place, &format!(r"{path}\{s}")));
        }
        lines.sort();
        lines
    }

    fn dump_all() -> Vec<Vec<String>> {
        PLACES.iter().map(|p| dump(*p, &base())).collect()
    }

    #[test]
    fn fixture_uninstall_keys_are_read_back_exactly_and_nothing_changes() {
        let _cleanup = Fixture;
        write_fixture();
        let before = dump_all();
        assert!(before.iter().all(|d| d.len() > 20), "the fixture was written: {before:?}");

        let reading = read_programs(&sources());
        let after = dump_all();

        assert!(reading.errors.is_empty(), "{:?}", reading.errors);
        // Every program of every place, read back exactly; the skipped ones are not there.
        let mut want: Vec<Program> = PLACES.iter().flat_map(|p| expected(*p)).collect();
        let mut got = reading.programs.clone();
        let key = |p: &Program| (p.name.clone(), p.key_name.clone());
        want.sort_by_key(key);
        got.sort_by_key(key);
        assert_eq!(got, want, "what was read is not what was written");
        for skipped in ["no-name", "system-component", "parent", "update", "hotfix", "security-update"] {
            assert!(!reading.programs.iter().any(|p| p.key_name == skipped), "{skipped} must be skipped");
        }
        // Largest reported size first.
        let sizes: Vec<u64> = reading.programs.iter().filter_map(|p| p.reported_size_bytes).collect();
        assert!(sizes.windows(2).all(|w| w[0] >= w[1]), "{sizes:?}");
        let first_unsized = reading.programs.iter().position(|p| p.reported_size_bytes.is_none()).unwrap();
        assert!(reading.programs[first_unsized..].iter().all(|p| p.reported_size_bytes.is_none()));
        // Reading wrote nothing.
        assert_eq!(before, after, "reading changed the fixture keys");
    }

    #[test]
    fn a_source_that_does_not_exist_gives_no_programs_and_no_error() {
        let missing = Source { hive: Hive::CurrentUser, path: format!(r"{}\Nothing\Here", base()), view: View::X64 };
        let reading = read_programs(&[missing]);
        assert!(reading.programs.is_empty());
        assert!(reading.errors.is_empty(), "{:?}", reading.errors);
    }

    #[test]
    fn the_default_sources_read_without_error_on_this_machine() {
        let reading = read_programs(&wdc_core::programs::default_sources());
        assert!(reading.errors.is_empty(), "{:?}", reading.errors);
        assert!(reading.programs.iter().all(|p| !p.name.trim().is_empty()));
        assert!(reading.programs.iter().all(|p| p.publisher.as_deref() != Some("")));
    }
}
