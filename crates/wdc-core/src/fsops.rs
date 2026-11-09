//! The only file operations that change the disk (plan, sections 5.1 and 5.2): an item is
//! opened once without following links, checked on that handle, and renamed through the same
//! handle, so nothing can be swapped in between the check and the move. Nothing here deletes
//! except `delete_tree`, which purge calls on paths inside the quarantine only.

use std::path::Path;

/// An item opened for moving.
pub struct OpenItem {
    #[cfg(windows)]
    handle: crate::win::OwnedHandle,
    #[cfg(not(windows))]
    path: std::path::PathBuf,
    /// `<volume serial>:<file id>`, hex.
    pub file_id: String,
    /// The real path Windows resolves from the handle (long names, `..` and dots resolved).
    pub final_path: String,
    pub is_dir: bool,
    /// The item itself is a link (junction, symbolic link, mount point).
    pub is_link: bool,
}

/// Why an operation failed, with the Windows error code.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct OpError {
    pub code: u32,
    pub kind: &'static str,
}

impl OpError {
    pub fn from_code(code: u32) -> OpError {
        let kind = match code {
            2 | 3 => "not-found",
            5 => "access-denied",
            32 | 33 => "in-use",
            17 => "other-volume",
            80 | 183 => "exists",
            _ => "other",
        };
        OpError { code, kind }
    }
}

#[cfg(windows)]
mod imp {
    use super::{OpError, OpenItem};
    use std::path::Path;

    use windows_sys::Win32::Foundation::{GENERIC_READ, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileAttributeTagInfo, FileIdInfo, FileRenameInfo, FileRenameInfoEx, GetDiskFreeSpaceExW,
        GetFileInformationByHandleEx, GetFinalPathNameByHandleW, GetVolumePathNameW, SetFileInformationByHandle,
        DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_NAME_NORMALIZED,
        FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        SYNCHRONIZE, VOLUME_NAME_DOS,
    };

    use crate::win::{display_path, last_error, verbatim, OwnedHandle};

    fn open(path: &Path, access: u32) -> Result<OwnedHandle, OpError> {
        let mut w = verbatim(path).map_err(|e| OpError::from_code(e.raw_os_error().unwrap_or(0) as u32))?;
        w.push(0);
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                // Never follow a link: the handle is to the name itself.
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        OwnedHandle::new(h).ok_or_else(|| OpError::from_code(last_error()))
    }

    fn id_of(h: HANDLE) -> Option<String> {
        let mut id: FILE_ID_INFO = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            GetFileInformationByHandleEx(h, FileIdInfo, &mut id as *mut FILE_ID_INFO as *mut _, std::mem::size_of::<FILE_ID_INFO>() as u32)
        };
        (ok != 0).then(|| format!("{:016x}:{:032x}", id.VolumeSerialNumber, u128::from_le_bytes(id.FileId.Identifier)))
    }

    fn final_path_of(h: HANDLE) -> Option<String> {
        let mut buf = vec![0u16; 32_768];
        let n = unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS) };
        (n > 0 && (n as usize) < buf.len()).then(|| display_path(&buf[..n as usize]))
    }

    pub fn open_item(path: &Path) -> Result<OpenItem, OpError> {
        let handle = open(path, DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)?;
        let mut tag: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle.raw(),
                FileAttributeTagInfo,
                &mut tag as *mut FILE_ATTRIBUTE_TAG_INFO as *mut _,
                std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        if ok == 0 {
            return Err(OpError::from_code(last_error()));
        }
        let is_link = tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && tag.ReparseTag & 0x2000_0000 != 0;
        let file_id = id_of(handle.raw()).ok_or_else(|| OpError::from_code(last_error()))?;
        let final_path = final_path_of(handle.raw()).ok_or_else(|| OpError::from_code(last_error()))?;
        Ok(OpenItem { file_id, final_path, is_dir: tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0, is_link, handle })
    }

    /// File ID of a path (attributes only, links not followed).
    pub fn file_id(path: &Path) -> Option<String> {
        let h = open(path, FILE_READ_ATTRIBUTES).ok()?;
        id_of(h.raw())
    }

    /// Final path of an existing path (links not followed).
    pub fn final_path(path: &Path) -> Option<String> {
        let h = open(path, FILE_READ_ATTRIBUTES).ok()?;
        final_path_of(h.raw())
    }

    /// Renames the opened item to `target` through its handle (never replaces anything).
    pub fn rename(item: &OpenItem, target: &Path) -> Result<(), OpError> {
        // The NT form of the target: `\??\C:\...` (SetFileInformationByHandle takes NT paths).
        let v = verbatim(target).map_err(|e| OpError::from_code(e.raw_os_error().unwrap_or(0) as u32))?;
        let s = String::from_utf16_lossy(&v);
        let nt: Vec<u16> = format!(r"\??\{}", s.strip_prefix(r"\\?\").unwrap_or(&s)).encode_utf16().collect();
        let header = std::mem::size_of::<FILE_RENAME_INFO>();
        let bytes = header + nt.len() * 2;
        let mut buf = vec![0u64; bytes.div_ceil(8) + 1];
        let info = buf.as_mut_ptr() as *mut FILE_RENAME_INFO;
        unsafe {
            // Flags 0: do not replace an existing target.
            (*info).Anonymous.Flags = 0;
            (*info).RootDirectory = std::ptr::null_mut();
            (*info).FileNameLength = (nt.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(nt.as_ptr(), std::ptr::addr_of_mut!((*info).FileName) as *mut u16, nt.len());
        }
        let ok = unsafe { SetFileInformationByHandle(item.handle.raw(), FileRenameInfoEx, info as *const _, bytes as u32) };
        if ok != 0 {
            return Ok(());
        }
        let e = last_error();
        if e != 87 {
            return Err(OpError::from_code(e));
        }
        // File systems without the extended class: the classic one, ReplaceIfExists = false.
        unsafe { (*info).Anonymous.ReplaceIfExists = false };
        let ok = unsafe { SetFileInformationByHandle(item.handle.raw(), FileRenameInfo, info as *const _, bytes as u32) };
        if ok != 0 {
            Ok(())
        } else {
            Err(OpError::from_code(last_error()))
        }
    }

    /// Bytes free on the volume holding `path`.
    pub fn free_space(path: &Path) -> Option<u64> {
        let mut w: Vec<u16> = crate::win::wide_path(path);
        let mut free = 0u64;
        let ok = unsafe { GetDiskFreeSpaceExW(w.as_mut_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(free)
    }

    /// The root of the volume holding `path` (`C:\`).
    pub fn volume_root(path: &Path) -> Option<String> {
        let w = crate::win::wide_path(path);
        let mut buf = vec![0u16; 1024];
        let ok = unsafe { GetVolumePathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if ok == 0 {
            return None;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        Some(String::from_utf16_lossy(&buf[..n]))
    }

    #[allow(dead_code)]
    const _: u32 = GENERIC_READ;
}

#[cfg(not(windows))]
mod imp {
    //! Development fallback for non-Windows hosts: the same contract with POSIX calls.
    use super::{OpError, OpenItem};
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    fn err(e: std::io::Error) -> OpError {
        OpError::from_code(match e.kind() {
            std::io::ErrorKind::NotFound => 2,
            std::io::ErrorKind::PermissionDenied => 5,
            std::io::ErrorKind::AlreadyExists => 183,
            _ => e.raw_os_error().unwrap_or(0) as u32,
        })
    }

    pub fn open_item(path: &Path) -> Result<OpenItem, OpError> {
        let m = std::fs::symlink_metadata(path).map_err(err)?;
        let parent = path.parent().map(|p| std::fs::canonicalize(p).unwrap_or(p.to_path_buf())).unwrap_or_default();
        let final_path = parent.join(path.file_name().unwrap_or_default()).to_string_lossy().into_owned();
        Ok(OpenItem {
            path: path.to_path_buf(),
            file_id: format!("{:016x}:{:032x}", m.dev(), m.ino()),
            final_path,
            is_dir: m.is_dir(),
            is_link: m.file_type().is_symlink(),
        })
    }

    pub fn file_id(path: &Path) -> Option<String> {
        let m = std::fs::symlink_metadata(path).ok()?;
        Some(format!("{:016x}:{:032x}", m.dev(), m.ino()))
    }

    pub fn final_path(path: &Path) -> Option<String> {
        open_item(path).ok().map(|i| i.final_path)
    }

    pub fn rename(item: &OpenItem, target: &Path) -> Result<(), OpError> {
        if std::fs::symlink_metadata(target).is_ok() {
            return Err(OpError::from_code(183));
        }
        std::fs::rename(&item.path, target).map_err(err)
    }

    pub fn free_space(_path: &Path) -> Option<u64> {
        None
    }

    pub fn volume_root(_path: &Path) -> Option<String> {
        Some("/".into())
    }
}

pub use imp::{file_id, final_path, free_space, open_item, rename, volume_root};

/// Deletes a quarantined item for good. Links inside are removed, never followed.
pub fn delete_tree(path: &Path) -> std::io::Result<()> {
    let m = std::fs::symlink_metadata(path)?;
    if m.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}
