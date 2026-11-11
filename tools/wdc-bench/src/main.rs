//! Budget check of the rebuild plan, section 6.4, run on the `windows-latest` runner (never on
//! the owner's laptop) against the real `wdc.exe` and `WinDevCleaner.exe` of slice 1.
//!
//! It creates a fresh 4 GiB VHDX, generates the fixture on it with the expected values read
//! from Windows through other APIs than the scanner's, runs 5 warm and 3 cold scans
//! (detaching and re-attaching the VHDX before each cold one), measures start-up, executable
//! size and the desktop app (idle CPU and memory), compares everything with the budgets and
//! writes `budget.json` and `budget.md`. It exits non-zero when a result or a budget fails.

#[cfg(windows)]
mod cloud;
#[cfg(windows)]
mod fixture;
#[cfg(windows)]
mod parallel;
#[cfg(windows)]
mod run;

fn main() {
    #[cfg(windows)]
    std::process::exit(run::main());
    #[cfg(not(windows))]
    {
        eprintln!("wdc-bench runs on Windows only");
        std::process::exit(2);
    }
}
