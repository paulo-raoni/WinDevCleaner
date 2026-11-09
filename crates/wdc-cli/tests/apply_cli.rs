//! `wdc apply`, `restore` and `purge` end to end through `wdc.exe` (plan, sections 5 and 7):
//! the presence code is read from the desktop dialog, as only a person (or an agent that can
//! read the screen) could; a pseudo-console alone is not proof; a program that starts after
//! planning keeps its item; early purge only for caches and install leftovers. Windows only,
//! needs a desktop session and administrator rights (the CI runner has both); run with
//! `--ignored`.
#![cfg(windows)]

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use wdc_core::presence::DIALOG_TITLE;
use wdc_testkit::{fresh_dir, guard_assert, has_window, read_presence_code, run_in_pseudo_console, write_file, Vhdx};

const MIB: u64 = 1024 * 1024;

/// A fake user (profile on `root`) and a data folder.
struct User {
    profile: PathBuf,
    data: PathBuf,
}

impl User {
    fn new(root: &Path, data: PathBuf) -> User {
        let profile = root.join("Users").join("me");
        std::fs::create_dir_all(profile.join("AppData").join("Local").join("Temp")).unwrap();
        std::fs::create_dir_all(profile.join("AppData").join("Roaming")).unwrap();
        User { profile, data }
    }

    fn roaming(&self) -> PathBuf {
        self.profile.join("AppData").join("Roaming")
    }

    fn env(&self) -> Vec<(&'static str, OsString)> {
        let local = self.profile.join("AppData").join("Local");
        vec![
            ("USERPROFILE", self.profile.clone().into_os_string()),
            ("APPDATA", self.roaming().into_os_string()),
            ("LOCALAPPDATA", local.clone().into_os_string()),
            ("TEMP", local.join("Temp").into_os_string()),
            ("WDC_DATA_DIR", self.data.clone().into_os_string()),
            ("WDC_LANG", "en".into()),
            // Windows Hello is not set up on the runner; say so, so the code path is certain.
            ("WDC_NO_HELLO", "1".into()),
        ]
    }

    fn wdc(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_wdc"));
        c.args(args);
        for (k, v) in self.env() {
            c.env(k, v);
        }
        c
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.wdc(args).output().unwrap();
        assert!(out.status.success(), "wdc {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
    }

    /// Drafts a plan of `paths`; returns the plan file.
    fn plan(&self, paths: &[&Path]) -> PathBuf {
        let mut args = vec!["plan", "--json"];
        let p: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        args.extend(p.iter().map(String::as_str));
        let v = self.json(&args);
        assert_eq!(v["items"].as_array().unwrap().len(), paths.len(), "{v}");
        PathBuf::from(v["file"].as_str().unwrap())
    }
}

enum Answer<'a> {
    /// Read the code from the dialog and type it.
    TheCode,
    /// Type this instead, once the dialog is up.
    Typed(&'a str),
    /// Close the input without typing.
    Nothing,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    json: Value,
}

/// Runs a `wdc` command that asks for presence, answering through its standard input.
fn run(user: &User, args: &[&str], answer: Answer) -> Run {
    let mut child = user.wdc(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let pid = child.id();
    let mut stdin = child.stdin.take().unwrap();
    match answer {
        Answer::TheCode => {
            let code = read_presence_code(pid, DIALOG_TITLE, Duration::from_secs(60)).expect("the presence dialog shows a code");
            writeln!(stdin, "{code}").unwrap();
        }
        Answer::Typed(t) => {
            let deadline = Instant::now() + Duration::from_secs(60);
            while !has_window(pid, DIALOG_TITLE) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
            writeln!(stdin, "{t}").unwrap();
        }
        Answer::Nothing => {}
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let json = serde_json::from_str(&stdout).unwrap_or(Value::Null);
    Run { code: out.status.code(), stdout, stderr: String::from_utf8_lossy(&out.stderr).into_owned(), json }
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

fn cache_in(user: &User, name: &str) -> PathBuf {
    let c = user.roaming().join("Code").join(name);
    std::fs::create_dir_all(c.join("v1").join("deep")).unwrap();
    write_file(&c.join("v1").join("deep").join("data.bin"), 3 * MIB);
    write_file(&c.join("v1").join("index.txt"), 1234);
    std::fs::create_dir_all(c.join("empty")).unwrap();
    c
}

#[test]
#[ignore]
fn apply_with_the_desktop_code_then_restore_gives_the_identical_tree() {
    let vol = Vhdx::create("cli-apply", 128);
    let user = User::new(&vol.root, fresh_dir("cli-apply-data"));
    let cache = cache_in(&user, "CachedData");
    let before = tree(&cache);
    let file = user.plan(&[&cache]);

    let r = run(&user, &["apply", "--plan", &file.to_string_lossy(), "--json"], Answer::TheCode);
    println!("{}\n{}", r.stdout, r.stderr);
    guard_assert!("presence-code", r.code == Some(0) && r.json["moved"] == 1, "the right code applies the plan: {} {}", r.stdout, r.stderr);
    assert!(!cache.exists(), "the item moved");
    let q = user.json(&["quarantine", "--json"]);
    let id = q["items"][0]["id"].as_str().unwrap().to_string();
    let stored = PathBuf::from(q["items"][0]["stored"].as_str().unwrap());
    assert_eq!(tree(&stored), before, "the quarantine holds the same tree");
    assert!(q["held_bytes"].as_u64().unwrap() >= 3 * MIB);

    let r = run(&user, &["restore", &id, "--json"], Answer::TheCode);
    println!("{}\n{}", r.stdout, r.stderr);
    assert_eq!(r.code, Some(0), "{} {}", r.stdout, r.stderr);
    assert_eq!(tree(&cache), before, "restore gives the identical tree");
    assert_eq!(user.json(&["quarantine", "--json"])["held_bytes"], 0);
    drop(vol);
    wdc_testkit::remove_tree(&user.data);
}

#[test]
#[ignore]
fn apply_restore_and_purge_without_presence_proof_refuse() {
    let base = fresh_dir("cli-refuse");
    let user = User::new(&base, base.join("data"));
    let cache = cache_in(&user, "CachedData");
    let before = tree(&cache);
    let file = user.plan(&[&cache]);
    let file_s = file.to_string_lossy().into_owned();

    // A wrong code.
    let r = run(&user, &["apply", "--plan", &file_s, "--json"], Answer::Typed("12345678"));
    assert_eq!(r.code, Some(4), "{} {}", r.stdout, r.stderr);
    assert_eq!(r.json["detail"]["reason"], "wrong-code", "{}", r.stdout);
    // The plan's own hash (known to whoever wrote the plan) is not proof either.
    let hash = serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap()["hash"].as_str().unwrap().to_string();
    let r = run(&user, &["apply", "--plan", &file_s, "--json"], Answer::Typed(&hash));
    assert_eq!(r.code, Some(4), "{} {}", r.stdout, r.stderr);
    // No input at all.
    let r = run(&user, &["apply", "--plan", &file_s, "--json"], Answer::Nothing);
    assert_eq!(r.code, Some(4), "{} {}", r.stdout, r.stderr);
    assert_eq!(tree(&cache), before, "nothing moved");

    // A real interactive console (ConPTY) is not proof: typing yes there changes nothing.
    let env: Vec<(&str, OsString)> = user.env();
    let env_ref: Vec<(&str, &OsStr)> = env.iter().map(|(k, v)| (*k, v.as_os_str())).collect();
    let c = run_in_pseudo_console(
        Path::new(env!("CARGO_BIN_EXE_wdc")),
        &["apply", "--plan", &file_s],
        &env_ref,
        "Type it and press Enter",
        "yes",
        Duration::from_secs(120),
    );
    println!("ConPTY output:\n{}", c.output);
    assert!(c.output.contains("Type it and press Enter"), "the console asked for the code (exit {:?})", c.exit_code);
    assert_eq!(c.exit_code, Some(4), "refused in a pseudo-console");
    assert_eq!(tree(&cache), before, "nothing moved");
    assert_eq!(user.json(&["quarantine", "--json"])["held_bytes"], 0);

    // Restore and purge ask too: apply for real, then try both with a wrong code.
    let r = run(&user, &["apply", "--plan", &file_s, "--json"], Answer::TheCode);
    assert_eq!(r.json["moved"], 1, "{} {}", r.stdout, r.stderr);
    let id = user.json(&["quarantine", "--json"])["items"][0]["id"].as_str().unwrap().to_string();
    let r = run(&user, &["restore", &id, "--json"], Answer::Typed("00000000"));
    assert_eq!(r.code, Some(4), "{} {}", r.stdout, r.stderr);
    let r = run(&user, &["purge", "--now", "--json"], Answer::Typed("00000000"));
    assert_eq!(r.code, Some(4), "{} {}", r.stdout, r.stderr);
    assert_eq!(user.json(&["quarantine", "--json"])["items"].as_array().unwrap().len(), 1, "still held");
    wdc_testkit::remove_tree(&base);
}

#[test]
#[ignore]
fn a_program_started_after_planning_keeps_its_item() {
    let base = fresh_dir("cli-fake-code");
    let user = User::new(&base, base.join("data"));
    let cache = cache_in(&user, "CachedData");
    let file = user.plan(&[&cache]);

    // A program named Code.exe starts after the plan was drafted.
    let fake = base.join("fake").join("Code.exe");
    std::fs::create_dir_all(fake.parent().unwrap()).unwrap();
    std::fs::copy(PathBuf::from(std::env::var("WINDIR").unwrap()).join("System32").join("PING.EXE"), &fake).unwrap();
    let mut code = Command::new(&fake).args(["-n", "300", "127.0.0.1"]).stdout(Stdio::null()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !wdc_core::facts::running_programs().unwrap_or_default().contains("code.exe") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }

    let r = run(&user, &["apply", "--plan", &file.to_string_lossy(), "--json"], Answer::TheCode);
    let _ = code.kill();
    let _ = code.wait();
    println!("{}\n{}", r.stdout, r.stderr);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    guard_assert!("apply-conditions", r.json["moved"] == 0, "the item of a running program stays: {}", r.stdout);
    assert_eq!(r.json["items"][0]["skipped"]["reason"], "condition-failed", "{}", r.stdout);
    assert!(cache.join("v1").join("deep").join("data.bin").exists());
    wdc_testkit::remove_tree(&base);
}

#[test]
#[ignore]
fn purge_before_seven_days_is_only_for_caches_and_leftovers_on_both_paths() {
    let base = fresh_dir("cli-purge");
    let user = User::new(&base, base.join("data"));

    // `apply --purge-now`: a cache goes at once, and the space is measured.
    let first = cache_in(&user, "GPUCache");
    let file = user.plan(&[&first]);
    let r = run(&user, &["apply", "--plan", &file.to_string_lossy(), "--purge-now", "--json"], Answer::TheCode);
    println!("{}", r.stdout);
    assert_eq!(r.json["moved"], 1, "{} {}", r.stdout, r.stderr);
    assert_eq!(r.json["purge"]["items"][0]["purged"], true, "{}", r.stdout);
    assert!(r.json["purge"]["volumes"][0]["free_after"].as_u64().is_some(), "free space is measured");

    // `purge` without --now keeps a fresh item; with --now the built-in rule decides, not the index.
    let second = cache_in(&user, "CachedData");
    let file = user.plan(&[&second]);
    let r = run(&user, &["apply", "--plan", &file.to_string_lossy(), "--json"], Answer::TheCode);
    assert_eq!(r.json["moved"], 1, "{} {}", r.stdout, r.stderr);
    let r = run(&user, &["purge", "--json"], Answer::TheCode);
    assert_eq!(r.json["items"][0]["purged"], false, "{}", r.stdout);
    assert_eq!(r.json["items"][0]["refused"]["reason"], "too-recent", "{}", r.stdout);

    let index = user.data.join("quarantine").join("index.json");
    let original = std::fs::read_to_string(&index).unwrap();
    let mut v: Value = serde_json::from_str(&original).unwrap();
    v[0]["rule_id"] = "vscode-workspace-storage".into();
    std::fs::write(&index, serde_json::to_vec(&v).unwrap()).unwrap();
    let r = run(&user, &["purge", "--now", "--json"], Answer::TheCode);
    println!("{}", r.stdout);
    guard_assert!("early-purge-class", r.json["items"][0]["purged"] == false, "an app-managed item waits 7 days: {}", r.stdout);
    assert_eq!(r.json["items"][0]["refused"]["reason"], "not-cache-or-leftover", "{}", r.stdout);

    std::fs::write(&index, original).unwrap();
    let r = run(&user, &["purge", "--now", "--json"], Answer::TheCode);
    assert_eq!(r.json["items"][0]["purged"], true, "{}", r.stdout);
    assert_eq!(r.json["quarantine_held_bytes"], 0);
    wdc_testkit::remove_tree(&base);
}
