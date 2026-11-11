//! Slice 5 (plan, section 8): `wdc explain` and the manual steps of NVIDIA update artifacts.
//! Runs `wdc.exe` itself against a fake `%ProgramData%`; platform-independent, and run on the
//! Windows runner.
//!
//! The older-artifact case (the exact `Remove-Item` line) is covered in `wdc-core`'s
//! `rules.rs`: the installed driver is read from the registry, and the CLI has no setting to
//! fake it, so an older artifact cannot be built here without new product code.

use std::path::Path;
use std::process::Command;

use wdc_testkit::fresh_dir;

fn write(path: &Path, len: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, vec![7u8; len]).unwrap();
}

fn explain(base: &Path, path: &Path, json: bool, lang: &str) -> String {
    let mut args = vec!["explain", path.to_str().unwrap()];
    if json {
        args.push("--json");
    }
    let out = Command::new(env!("CARGO_BIN_EXE_wdc"))
        .args(args)
        .env("WDC_DATA_DIR", base.join("data"))
        .env("WDC_LANG", lang)
        .env("ProgramData", base.join("ProgramData"))
        .env("APPDATA", base.join("u").join("AppData").join("Roaming"))
        .env("LOCALAPPDATA", base.join("u").join("AppData").join("Local"))
        .output()
        .unwrap();
    assert!(out.status.success(), "wdc explain: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn explain_json_gives_manual_steps_for_nvidia_artifacts_and_for_no_other_rule() {
    let base = fresh_dir("cli-explain-steps");
    let ota = base.join("ProgramData").join("NVIDIA Corporation").join("NVIDIA app").join("UpdateFramework").join("ota-artifacts");
    // No version in the name: it can never be compared, so the steps say not to delete it.
    let artifact = ota.join("no-version-here");
    write(&artifact.join("blob.bin"), 1024 * 1024);
    let cache = base.join("u").join("AppData").join("Roaming").join("Code").join("Code Cache");
    write(&cache.join("index"), 1024 * 1024);

    let e: serde_json::Value = serde_json::from_str(&explain(&base, &artifact, true, "en")).expect("JSON output");
    assert_eq!(e["rule"]["id"], "nvidia-ota-artifacts", "{e}");
    assert_eq!(e["actionable"], false, "report-only stays");
    let steps = e["manual_steps"].as_array().expect("manual_steps is an array of strings");
    assert_eq!(steps.len(), 1, "{steps:?}");
    let text = steps[0].as_str().unwrap();
    assert!(text.starts_with("Do not delete") && !text.contains("Remove-Item"), "{text}");

    // Human output, numbered, in the chosen language.
    let human = explain(&base, &artifact, false, "pt-BR");
    assert!(human.contains("Passos manuais") && human.contains("1. Não apague"), "{human}");

    // Another rule: the field is absent.
    let c: serde_json::Value = serde_json::from_str(&explain(&base, &cache, true, "en")).expect("JSON output");
    assert_eq!(c["rule"]["id"], "vscode-caches", "{c}");
    assert!(c.get("manual_steps").is_none(), "{c}");
    wdc_testkit::remove_tree(&base);
}
