//! System items (plan, sections 4.1 and 8, slice 2): sizes read from Windows' own APIs, each
//! with the tool that manages it. Windows only.
#![cfg(windows)]

use wdc_core::system::{recycle_bin, system_items, Kind, Status};
use wdc_testkit::{fresh_dir, guard_assert, guard_assert_eq, powershell_root_file_length, write_file};

const MIB: u64 = 1024 * 1024;

#[test]
fn recycle_bin_size_comes_from_the_shell() {
    let before = recycle_bin();
    assert_eq!(before.status, Status::Measured);
    let base = fresh_dir("recycle");
    let f = base.join("deleted-by-the-user.bin");
    write_file(&f, 3 * MIB);
    wdc_testkit::send_to_recycle_bin(&f);
    assert!(!f.exists());

    let after = recycle_bin();
    let grew = after.bytes.unwrap_or(0) as i64 - before.bytes.unwrap_or(0) as i64;
    println!("Recycle Bin: {:?} -> {:?} bytes, {:?} -> {:?} items", before.bytes, after.bytes, before.items, after.items);
    guard_assert!("recycle-bin", grew >= 3 * MIB as i64, "the Recycle Bin grew by {grew} bytes, expected at least 3 MiB");
    assert!(after.items.unwrap() > before.items.unwrap_or(0));
    assert!(after.managed_by.contains("Storage Sense"));
}

#[test]
fn paging_swap_and_hibernation_files_match_the_volume_listing() {
    let items = system_items();
    let mut found = 0;
    for drive in 'C'..='Z' {
        for (kind, name) in
            [(Kind::PageFile, "pagefile.sys"), (Kind::SwapFile, "swapfile.sys"), (Kind::HibernationFile, "hiberfil.sys")]
        {
            let path = format!("{drive}:\\{name}");
            let ours = items.iter().find(|i| i.kind == kind && i.path.as_deref() == Some(path.as_str()));
            let theirs = powershell_root_file_length(drive, name);
            match (ours, theirs) {
                (Some(i), Some(len)) => {
                    found += 1;
                    println!("{path}: {len} bytes");
                    guard_assert_eq!("system-file-size", i.bytes, Some(len), "{path}");
                }
                (None, None) => {}
                (o, t) => panic!("{path}: engine {o:?}, PowerShell {t:?}"),
            }
        }
    }
    assert!(found > 0, "the runner has a paging file somewhere");
}

#[test]
fn crash_dumps_are_measured_with_the_tool_that_manages_them() {
    let local = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap();
    let dumps = local.join("CrashDumps");
    std::fs::create_dir_all(&dumps).unwrap();
    let f = dumps.join(format!("wdc-test-{}.dmp", std::process::id()));
    write_file(&f, 2 * MIB);
    let item = system_items().into_iter().find(|i| i.kind == Kind::CrashDumps).expect("crash dumps item");
    let _ = std::fs::remove_file(&f);
    assert_eq!(item.status, Status::Measured);
    assert!(item.bytes.unwrap() >= 2 * MIB);
    assert!(item.managed_by.contains("Disk Cleanup"));
}

#[test]
fn system_restore_is_reported_as_needing_an_administrator() {
    let item = system_items().into_iter().find(|i| i.kind == Kind::SystemRestore).unwrap();
    assert_eq!(item.status, Status::NeedsAdmin);
    assert!(item.managed_by.contains("System Protection"));
}
