//! Orchestrates the budget check and writes the report.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use wdc_testkit::{measure, Measurement};

use crate::fixture::{self, Expected, Fixture};

const MIB: u64 = 1024 * 1024;

/// Budgets of plan section 6.4.
const WARM_MS: f64 = 2_000.0;
const COLD_MS: f64 = 15_000.0;
const PRIVATE_BYTES: u64 = 40 * MIB;
const STARTUP_MS: f64 = 50.0;
const WDC_EXE_BYTES: u64 = 15 * MIB;
const APP_MEMORY_BYTES: u64 = 80 * MIB;
/// "About 0%": at most 0.5% of one core while idle.
const APP_IDLE_CPU_PERCENT: f64 = 0.5;

struct Args {
    wdc: PathBuf,
    app: PathBuf,
    vhd: PathBuf,
    letter: char,
    out: PathBuf,
    build_seconds: Option<f64>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = std::env::args().skip(1);
    let (mut wdc, mut app, mut vhd, mut letter, mut out, mut build) = (None, None, None, 'W', None, None);
    while let Some(k) = a.next() {
        let v = a.next().ok_or(format!("{k} needs a value"))?;
        match k.as_str() {
            "--wdc" => wdc = Some(PathBuf::from(v)),
            "--app" => app = Some(PathBuf::from(v)),
            "--vhd" => vhd = Some(PathBuf::from(v)),
            "--letter" => letter = v.chars().next().ok_or("bad letter")?,
            "--out" => out = Some(PathBuf::from(v)),
            "--build-seconds" => build = v.parse().ok(),
            _ => return Err(format!("unknown option {k}")),
        }
    }
    Ok(Args {
        wdc: wdc.ok_or("--wdc is required")?,
        app: app.ok_or("--app is required")?,
        vhd: vhd.ok_or("--vhd is required")?,
        letter,
        out: out.ok_or("--out is required")?,
        build_seconds: build,
    })
}

fn diskpart(script: &str) -> Result<String, String> {
    let file = std::env::temp_dir().join(format!("wdc-diskpart-{}.txt", std::process::id()));
    std::fs::write(&file, script).map_err(|e| e.to_string())?;
    let out = Command::new("diskpart").arg("/s").arg(&file).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

fn create_vhd(vhd: &Path, letter: char) -> Result<(), String> {
    if vhd.exists() {
        let _ = diskpart(&format!("select vdisk file=\"{}\"\ndetach vdisk noerr\n", vhd.display()));
        std::fs::remove_file(vhd).map_err(|e| e.to_string())?;
    }
    diskpart(&format!(
        "create vdisk file=\"{}\" maximum=4096 type=expandable\nselect vdisk file=\"{}\"\nattach vdisk\n\
         create partition primary\nformat fs=ntfs quick label=WDCBENCH\nassign letter={letter}\n",
        vhd.display(),
        vhd.display()
    ))?;
    wait_for_volume(letter, true)
}

fn wait_for_volume(letter: char, present: bool) -> Result<(), String> {
    let root = PathBuf::from(format!("{letter}:\\"));
    for _ in 0..100 {
        if root.exists() == present {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!("volume {letter}: did not {}", if present { "appear" } else { "go away" }))
}

/// Detach and re-attach: the volume is dismounted, so its file system cache is gone.
fn cold_cycle(vhd: &Path, letter: char) -> Result<(), String> {
    diskpart(&format!("select vdisk file=\"{}\"\ndetach vdisk\n", vhd.display()))?;
    wait_for_volume(letter, false)?;
    diskpart(&format!("select vdisk file=\"{}\"\nattach vdisk\n", vhd.display()))?;
    if wait_for_volume(letter, true).is_ok() {
        return Ok(());
    }
    // The mount manager did not give the letter back: assign it to the VHDX's partition.
    let script = format!(
        "$p = Get-DiskImage -ImagePath '{}' | Get-Disk | Get-Partition | Where-Object {{ $_.Type -ne 'Reserved' }} | Select-Object -First 1; \
         Set-Partition -InputObject $p -NewDriveLetter {letter}",
        vhd.display()
    );
    let out = Command::new("powershell").args(["-NoProfile", "-Command", &script]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    wait_for_volume(letter, true)
}

#[derive(Serialize)]
struct Run {
    kind: &'static str,
    measurement: Measurement,
    totals: Value,
    self_reported: Value,
    matches_expected: bool,
    mismatches: Vec<String>,
}

fn compare(totals: &Value, e: &Expected) -> Vec<String> {
    let want = serde_json::to_value(e).unwrap();
    let mut out = Vec::new();
    for (k, v) in want.as_object().unwrap() {
        if totals.get(k) != Some(v) {
            out.push(format!("{k}: scanner {} expected {v}", totals.get(k).cloned().unwrap_or(Value::Null)));
        }
    }
    out
}

fn scan_once(args: &Args, root: &Path, data: &Path, kind: &'static str, e: &Expected) -> Run {
    let mut cmd = Command::new(&args.wdc);
    cmd.arg("scan").arg(root).args(["--json", "--top", "50"]).env("WDC_DATA_DIR", data);
    let m = measure(&mut cmd);
    let v: Value = serde_json::from_str(&m.stdout).unwrap_or(Value::Null);
    let totals = v["totals"].clone();
    let mismatches = compare(&totals, e);
    Run { kind, matches_expected: mismatches.is_empty() && m.exit_code == Some(0), mismatches, totals, self_reported: v["process"].clone(), measurement: m }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        return f64::NAN;
    }
    v[v.len() / 2]
}

#[derive(Serialize, Default)]
struct AppBudget {
    started: bool,
    error: Option<String>,
    seconds_to_listen: f64,
    idle_cpu_percent_of_one_core: f64,
    idle_working_set_bytes: u64,
    idle_private_bytes: u64,
    after_scan_working_set_bytes: u64,
    after_scan_private_bytes: u64,
    peak_working_set_bytes: u64,
    peak_private_bytes: u64,
    after_scan_idle_cpu_percent_of_one_core: f64,
    scan_bytes_on_disk: Option<u64>,
}

fn idle_cpu(h: windows_sys::Win32::Foundation::HANDLE, seconds: u64) -> f64 {
    let c0 = wdc_testkit::cpu_ms(h);
    std::thread::sleep(Duration::from_secs(seconds));
    let c1 = wdc_testkit::cpu_ms(h);
    (c1 - c0) / (seconds as f64 * 1000.0) * 100.0
}

fn app_budget(args: &Args, root: &Path) -> AppBudget {
    use std::os::windows::io::AsRawHandle;
    use wdc_core::link::{pipe, CallOutcome, Client};
    let mut b = AppBudget::default();
    let data = wdc_testkit::fresh_dir("bench-app-data");
    let suffix = format!("-bench-{}", std::process::id());
    let name = pipe::pipe_name_with_suffix(&suffix).unwrap();
    let started = Instant::now();
    let mut child = match Command::new(&args.app)
        .env("WDC_DATA_DIR", &data)
        .env("WDC_PIPE_SUFFIX", &suffix)
        .env("WDC_LANG", "en")
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            b.error = Some(e.to_string());
            return b;
        }
    };
    if !wdc_testkit::wait_for_pipe(&name, Duration::from_secs(60)) {
        b.error = Some(format!("no agent pipe; exit status {:?}", child.try_wait()));
        let _ = child.kill();
        return b;
    }
    b.started = true;
    b.seconds_to_listen = started.elapsed().as_secs_f64();
    let h = child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    std::thread::sleep(Duration::from_secs(5));
    b.idle_cpu_percent_of_one_core = idle_cpu(h, 10);
    let (ws, _, private, _) = wdc_testkit::memory(h);
    b.idle_working_set_bytes = ws;
    b.idle_private_bytes = private;

    // Load the 200k-file tree into the window through the agent link, as an agent would.
    match pipe::connect(&name) {
        Ok(pipe::Connect::Connected(conn)) => match Client::handshake(conn, "wdc-bench") {
            Ok(mut c) => {
                if let CallOutcome::Ok(v) = c.call("scan", json!({ "roots": [root.to_string_lossy()] })) {
                    b.scan_bytes_on_disk = v["totals"]["bytes_on_disk"].as_u64();
                }
                c.bye();
            }
            Err(e) => b.error = Some(e),
        },
        Ok(pipe::Connect::NotRunning) => b.error = Some("pipe vanished".into()),
        Err(e) => b.error = Some(e.to_string()),
    }
    std::thread::sleep(Duration::from_secs(3));
    let (ws, _, private, _) = wdc_testkit::memory(h);
    b.after_scan_working_set_bytes = ws;
    b.after_scan_private_bytes = private;
    b.after_scan_idle_cpu_percent_of_one_core = idle_cpu(h, 10);
    let (_, peak_ws, _, peak_private) = wdc_testkit::memory(h);
    b.peak_working_set_bytes = peak_ws;
    b.peak_private_bytes = peak_private;
    let _ = child.kill();
    let _ = child.wait();
    b
}

#[derive(Serialize)]
struct Check {
    name: String,
    budget: String,
    measured: String,
    pass: bool,
}

fn check(checks: &mut Vec<Check>, name: &str, budget: String, measured: String, pass: bool) {
    checks.push(Check { name: name.into(), budget, measured, pass });
}

pub fn main() -> i32 {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("wdc-bench: {e}");
            return 2;
        }
    };
    std::fs::create_dir_all(&args.out).unwrap();
    let letter = args.letter;
    if let Err(e) = create_vhd(&args.vhd, letter) {
        eprintln!("could not create the VHDX: {e}");
        return 2;
    }
    let base = PathBuf::from(format!("{letter}:\\"));
    eprintln!("generating the fixture on {}", base.display());
    let (fixture, locked_path, cloud_root): (Fixture, PathBuf, PathBuf) = fixture::generate(&base);
    eprintln!("fixture: {:?}", fixture.expected);
    let root = PathBuf::from(&fixture.root);
    let data = wdc_testkit::fresh_dir("bench-data");

    let hold = |cloud_ok: bool| {
        let lock = wdc_testkit::hold_exclusive(&locked_path);
        let conn = if cloud_ok { crate::cloud::connect(&cloud_root).ok() } else { None };
        (lock, conn)
    };
    let cloud_ok = fixture.cloud.is_ok();

    let mut runs: Vec<Run> = Vec::new();
    {
        let _held = hold(cloud_ok);
        // One scan to warm the cache, then 5 warm runs.
        let _ = scan_once(&args, &root, &data, "warm-up", &fixture.expected);
        for _ in 0..5 {
            runs.push(scan_once(&args, &root, &data, "warm", &fixture.expected));
        }
    }
    let mut cold_error = None;
    for _ in 0..3 {
        if let Err(e) = cold_cycle(&args.vhd, letter) {
            cold_error = Some(e);
            break;
        }
        let _held = hold(cloud_ok);
        runs.push(scan_once(&args, &root, &data, "cold", &fixture.expected));
    }
    let fetches = crate::cloud::FETCH_DATA_CALLS.load(std::sync::atomic::Ordering::SeqCst);

    // Start-up: `wdc --version`, 21 runs.
    let startup: Vec<f64> = (0..21).map(|_| measure(Command::new(&args.wdc).arg("--version")).wall_ms).collect();
    let startup_ms = median(startup.clone());
    let wdc_size = std::fs::metadata(&args.wdc).map(|m| m.len()).unwrap_or(0);
    let app_size = std::fs::metadata(&args.app).map(|m| m.len()).unwrap_or(0);

    let app = app_budget(&args, &root);

    // Evaluate.
    let warm: Vec<&Run> = runs.iter().filter(|r| r.kind == "warm").collect();
    let cold: Vec<&Run> = runs.iter().filter(|r| r.kind == "cold").collect();
    let warm_ms = median(warm.iter().map(|r| r.measurement.wall_ms).collect());
    let cold_ms = median(cold.iter().map(|r| r.measurement.wall_ms).collect());
    let peak_private = runs.iter().map(|r| r.measurement.peak_private_bytes).max().unwrap_or(0);
    let mut checks = Vec::new();
    let all_match = runs.iter().all(|r| r.matches_expected);
    check(
        &mut checks,
        "results equal the generator's expected values (every run)",
        serde_json::to_string(&fixture.expected).unwrap(),
        if all_match {
            "all runs equal".into()
        } else {
            runs.iter().flat_map(|r| r.mismatches.clone()).collect::<Vec<_>>().join("; ")
        },
        all_match,
    );
    check(&mut checks, "warm scan, median of 5", format!("<= {WARM_MS} ms"), format!("{warm_ms:.0} ms"), warm_ms <= WARM_MS);
    check(
        &mut checks,
        "cold scan, median of 3 (VHDX detached and re-attached)",
        format!("<= {COLD_MS} ms"),
        match &cold_error {
            Some(e) => format!("not measured: {e}"),
            None => format!("{cold_ms:.0} ms"),
        },
        cold_error.is_none() && cold_ms <= COLD_MS,
    );
    check(
        &mut checks,
        "peak private memory, any run",
        format!("<= {} MiB", PRIVATE_BYTES / MIB),
        format!("{:.1} MiB", peak_private as f64 / MIB as f64),
        peak_private <= PRIVATE_BYTES,
    );
    let bg_ok = runs.iter().all(|r| !r.measurement.io_priority_samples.is_empty() && r.measurement.io_priority_samples.iter().all(|&p| p == 0));
    check(&mut checks, "background mode (I/O priority sampled from outside)", "very low (0) in every sample".into(), if bg_ok { "0 in every sample".into() } else { "not always 0".into() }, bg_ok);
    check(&mut checks, "start-up (`wdc --version`, median of 21)", format!("<= {STARTUP_MS} ms"), format!("{startup_ms:.1} ms"), startup_ms <= STARTUP_MS);
    check(&mut checks, "wdc.exe size", format!("<= {} MiB", WDC_EXE_BYTES / MIB), format!("{:.2} MiB", wdc_size as f64 / MIB as f64), wdc_size <= WDC_EXE_BYTES);
    match &fixture.cloud {
        Ok(_) => check(&mut checks, "cloud placeholders: data-fetch callbacks", "0".into(), fetches.to_string(), fetches == 0),
        Err(e) => check(&mut checks, "cloud placeholders (untested: no sync root)", "n/a".into(), e.clone(), true),
    }
    if app.started {
        let mem = app.idle_private_bytes.max(app.after_scan_private_bytes);
        check(
            &mut checks,
            "app idle CPU (10 s, before and after loading the 200k tree)",
            format!("<= {APP_IDLE_CPU_PERCENT}% of one core"),
            format!("{:.2}% / {:.2}%", app.idle_cpu_percent_of_one_core, app.after_scan_idle_cpu_percent_of_one_core),
            app.idle_cpu_percent_of_one_core <= APP_IDLE_CPU_PERCENT && app.after_scan_idle_cpu_percent_of_one_core <= APP_IDLE_CPU_PERCENT,
        );
        check(
            &mut checks,
            "app memory while open (private bytes, idle and with the 200k tree)",
            format!("<= {} MiB", APP_MEMORY_BYTES / MIB),
            format!(
                "{:.1} MiB idle, {:.1} MiB with the tree (working set {:.1} / {:.1} MiB)",
                app.idle_private_bytes as f64 / MIB as f64,
                app.after_scan_private_bytes as f64 / MIB as f64,
                app.idle_working_set_bytes as f64 / MIB as f64,
                app.after_scan_working_set_bytes as f64 / MIB as f64
            ),
            mem <= APP_MEMORY_BYTES,
        );
        check(
            &mut checks,
            "app scan through the agent link equals the expected size",
            fixture.expected.bytes_on_disk.to_string(),
            format!("{:?}", app.scan_bytes_on_disk),
            app.scan_bytes_on_disk == Some(fixture.expected.bytes_on_disk),
        );
    } else {
        check(&mut checks, "app started", "yes".into(), app.error.clone().unwrap_or_default(), false);
    }
    let pass = checks.iter().all(|c| c.pass);

    let report = json!({
        "pass": pass,
        "checks": checks,
        "fixture": fixture,
        "cloud_fetch_data_callbacks": fetches,
        "runs": runs,
        "startup_ms_all": startup,
        "wdc_exe_bytes": wdc_size,
        "app_exe_bytes": app_size,
        "build_seconds": args.build_seconds,
        "app": app,
        "not_measured": [
            "--jobs 1, 2 and 4: the option does not exist in slice 1 (plan 6.3: it ships in slice 6 only if a cold-cache gain above 2x is measured); every run is --jobs 1",
            "behaviour without admin rights: the runner is elevated (plan 6.4)"
        ],
    });
    std::fs::write(args.out.join("budget.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();

    let mut md = String::from("## Budget check (plan, section 6.4)\n\n| Check | Budget | Measured | Result |\n|---|---|---|---|\n");
    for c in &report["checks"].as_array().unwrap().clone() {
        md.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            c["name"].as_str().unwrap(),
            c["budget"].as_str().unwrap().replace('|', "\\|"),
            c["measured"].as_str().unwrap().replace('|', "\\|"),
            if c["pass"].as_bool().unwrap() { "pass" } else { "**FAIL**" }
        ));
    }
    md.push_str("\n| Run | Wall ms | CPU ms (user+kernel) | Peak private MiB | Peak working set MiB | Read ops | Read MiB |\n|---|---|---|---|---|---|---|\n");
    for r in &runs {
        let m = &r.measurement;
        md.push_str(&format!(
            "| {} | {:.0} | {:.0} | {:.1} | {:.1} | {} | {:.1} |\n",
            r.kind,
            m.wall_ms,
            m.user_ms + m.kernel_ms,
            m.peak_private_bytes as f64 / MIB as f64,
            m.peak_working_set_bytes as f64 / MIB as f64,
            m.read_operations,
            m.read_bytes as f64 / MIB as f64
        ));
    }
    md.push_str(&format!(
        "\nFixture: {} files, {} folders, {:.1} MiB on disk, generated in {:.0} s. Stale link: directory entry {} bytes, file {} bytes. Longest path {} characters.\n",
        fixture.expected.files,
        fixture.expected.dirs,
        fixture.expected.bytes_on_disk as f64 / MIB as f64,
        fixture.generation_seconds,
        fixture.stale_entry_bytes,
        fixture.stale_true_bytes,
        fixture.long_path_chars
    ));
    md.push_str(&format!(
        "\nExecutables: wdc.exe {:.2} MiB, WinDevCleaner.exe {:.2} MiB. Release build: {}.\n",
        wdc_size as f64 / MIB as f64,
        app_size as f64 / MIB as f64,
        args.build_seconds.map(|s| format!("{s:.0} s")).unwrap_or_else(|| "not given".into())
    ));
    md.push_str("\nNot measured: `--jobs` 2 and 4 (no such option in slice 1; plan 6.3 defers it to slice 6); behaviour without admin (the runner is elevated).\n");
    std::fs::write(args.out.join("budget.md"), &md).unwrap();
    println!("{md}");

    // Leave the runner clean: the sync root and the VHDX.
    if cloud_ok {
        crate::cloud::unregister(&cloud_root);
    }
    let _ = diskpart(&format!("select vdisk file=\"{}\"\ndetach vdisk noerr\n", args.vhd.display()));
    if pass {
        0
    } else {
        1
    }
}
