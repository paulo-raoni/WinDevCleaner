//! A minimal Cloud Files sync provider: registers a sync root, creates placeholders whose data
//! lives "in the cloud", and counts every data-fetch callback. A scan that read a placeholder's
//! data would trigger one (and, on OneDrive, a download).

use std::ffi::OsStr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use windows_sys::Win32::Storage::CloudFilters::*;
use windows_sys::Win32::Storage::FileSystem::FILE_BASIC_INFO;

pub static FETCH_DATA_CALLS: AtomicU64 = AtomicU64::new(0);

unsafe extern "system" fn on_fetch_data(_info: *const CF_CALLBACK_INFO, _params: *const CF_CALLBACK_PARAMETERS) {
    // Count and never answer: the requester would wait, then fail. The scanner must never
    // get here.
    FETCH_DATA_CALLS.fetch_add(1, Ordering::SeqCst);
}

fn wide(s: &OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn check(hr: i32, what: &str) -> Result<(), String> {
    if hr < 0 {
        Err(format!("{what} failed: HRESULT {hr:#010x}"))
    } else {
        Ok(())
    }
}

/// Registers `root` as a sync root (it must be an empty folder on NTFS).
pub fn register(root: &Path) -> Result<(), String> {
    let path = wide(root.as_os_str());
    let name = wide(OsStr::new("WinDevCleaner test provider"));
    let version = wide(OsStr::new("1.0"));
    let identity = b"wdc-bench";
    let registration = CF_SYNC_REGISTRATION {
        StructSize: std::mem::size_of::<CF_SYNC_REGISTRATION>() as u32,
        ProviderName: name.as_ptr(),
        ProviderVersion: version.as_ptr(),
        SyncRootIdentity: identity.as_ptr().cast(),
        SyncRootIdentityLength: identity.len() as u32,
        FileIdentity: std::ptr::null(),
        FileIdentityLength: 0,
        ProviderId: windows_sys::core::GUID::from_u128(0x5f0c_7a2e_93d4_4c6b_8a51_77e3_2b9d_c401),
    };
    let policies = CF_SYNC_POLICIES {
        StructSize: std::mem::size_of::<CF_SYNC_POLICIES>() as u32,
        Hydration: CF_HYDRATION_POLICY { Primary: CF_HYDRATION_POLICY_FULL, Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE },
        Population: CF_POPULATION_POLICY {
            Primary: CF_POPULATION_POLICY_ALWAYS_FULL,
            Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
        },
        InSync: CF_INSYNC_POLICY_NONE,
        HardLink: CF_HARDLINK_POLICY_NONE,
        PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
    };
    check(unsafe { CfRegisterSyncRoot(path.as_ptr(), &registration, &policies, CF_REGISTER_FLAG_NONE) }, "CfRegisterSyncRoot")
}

pub fn unregister(root: &Path) {
    let path = wide(root.as_os_str());
    unsafe {
        CfUnregisterSyncRoot(path.as_ptr());
    }
}

/// A live provider connection; data-fetch callbacks are counted while it exists.
pub struct Connection(CF_CONNECTION_KEY);

pub fn connect(root: &Path) -> Result<Connection, String> {
    let path = wide(root.as_os_str());
    let table = [
        CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_FETCH_DATA, Callback: Some(on_fetch_data) },
        CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_NONE, Callback: None },
    ];
    let mut key: CF_CONNECTION_KEY = unsafe { std::mem::zeroed() };
    check(
        unsafe { CfConnectSyncRoot(path.as_ptr(), table.as_ptr(), std::ptr::null(), CF_CONNECT_FLAG_NONE, &mut key) },
        "CfConnectSyncRoot",
    )?;
    Ok(Connection(key))
}

impl Drop for Connection {
    fn drop(&mut self) {
        unsafe {
            CfDisconnectSyncRoot(self.0);
        }
    }
}

/// Creates `count` dehydrated placeholders of `size` bytes each in `dir` (inside the root).
pub fn create_placeholders(dir: &Path, count: usize, size: u64) -> Result<(), String> {
    let base = wide(dir.as_os_str());
    let names: Vec<Vec<u16>> = (0..count).map(|i| wide(OsStr::new(&format!("cloud-{i:03}.bin")))).collect();
    let identity = b"id";
    let mut infos: Vec<CF_PLACEHOLDER_CREATE_INFO> = names
        .iter()
        .map(|n| {
            let mut info: CF_PLACEHOLDER_CREATE_INFO = unsafe { std::mem::zeroed() };
            info.RelativeFileName = n.as_ptr();
            info.FsMetadata.FileSize = size as i64;
            info.FsMetadata.BasicInfo = FILE_BASIC_INFO {
                CreationTime: 133_000_000_000_000_000,
                LastAccessTime: 133_000_000_000_000_000,
                LastWriteTime: 133_000_000_000_000_000,
                ChangeTime: 133_000_000_000_000_000,
                FileAttributes: 0x80, // FILE_ATTRIBUTE_NORMAL
            };
            info.FileIdentity = identity.as_ptr().cast();
            info.FileIdentityLength = identity.len() as u32;
            info.Flags = CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC;
            info
        })
        .collect();
    let mut processed = 0u32;
    check(
        unsafe { CfCreatePlaceholders(base.as_ptr(), infos.as_mut_ptr(), infos.len() as u32, CF_CREATE_FLAG_NONE, &mut processed) },
        "CfCreatePlaceholders",
    )?;
    if processed as usize != count {
        return Err(format!("CfCreatePlaceholders created {processed} of {count}"));
    }
    Ok(())
}

/// Turns a normal file with data into a placeholder and dehydrates its first half, so it is
/// partially on disk and partially "in the cloud" (RECALL_ON_DATA_ACCESS with real allocation).
pub fn partially_hydrated(path: &Path, size: u64) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    wdc_testkit::write_file(path, size);
    let f = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| e.to_string())?;
    let h = f.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    let identity = b"id";
    check(
        unsafe {
            CfConvertToPlaceholder(
                h,
                identity.as_ptr().cast(),
                identity.len() as u32,
                CF_CONVERT_FLAG_MARK_IN_SYNC,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        "CfConvertToPlaceholder",
    )?;
    check(
        unsafe { CfDehydratePlaceholder(h, 0, (size / 2) as i64, CF_DEHYDRATE_FLAG_NONE, std::ptr::null_mut()) },
        "CfDehydratePlaceholder",
    )
}
