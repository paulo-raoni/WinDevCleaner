//! Windows walker: one `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)` call lists a
//! whole directory with file IDs, sizes on disk, attributes and reparse tags, without opening
//! any file (plan, section 6.1). Falls back to `FileIdBothDirectoryInfo` on file systems
//! without the extended class.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FileAttributeTagInfo, FileIdBothDirectoryInfo, FileIdExtdDirectoryInfo,
    FileIdInfo, FileStandardInfo, GetFileInformationByHandle,
    GetFileInformationByHandleEx, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
    FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_ID_BOTH_DIR_INFO, FILE_ID_EXTD_DIR_INFO, FILE_ID_INFO, FILE_LIST_DIRECTORY,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_STANDARD_INFO, OPEN_EXISTING, SYNCHRONIZE,
};

use super::{Collector, LinkKind, RootReport, TopFile, DEDUP_THRESHOLD, NO_PARENT, VERIFY_THRESHOLD};
use crate::win::{display_path, last_error, verbatim, OwnedHandle};

const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;

/// A reparse point that is a link to another name (junction, symbolic link, mount point).
/// Other reparse points (compressed files, OneDrive files, dedup, app aliases) are measured.
fn is_name_surrogate(tag: u32) -> bool {
    tag & 0x2000_0000 != 0
}

fn link_kind(tag: u32) -> LinkKind {
    match tag {
        IO_REPARSE_TAG_MOUNT_POINT => LinkKind::Junction,
        IO_REPARSE_TAG_SYMLINK => LinkKind::SymbolicLink,
        _ => LinkKind::OtherLink,
    }
}

/// Opens a directory for listing. Links are never followed: the handle is to the name
/// itself (`FILE_FLAG_OPEN_REPARSE_POINT`), so a folder swapped for a junction after it was
/// listed is not entered either.
fn open_dir(path: &[u16]) -> Result<OwnedHandle, u32> {
    let mut z = path.to_vec();
    z.push(0);
    let h = unsafe {
        CreateFileW(
            z.as_ptr(),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    OwnedHandle::new(h).ok_or_else(last_error)
}

/// Opens a file with no data access (attributes only), never following links. Opening for
/// attributes does not hydrate cloud files and is not blocked by other programs' sharing.
fn open_attributes_only(path: &[u16]) -> Option<OwnedHandle> {
    let mut z = path.to_vec();
    z.push(0);
    let h = unsafe {
        CreateFileW(
            z.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        None
    } else {
        OwnedHandle::new(h)
    }
}

/// Size and link count read from the file itself.
struct Recheck {
    allocation: u64,
    length: u64,
    links: u32,
}

fn recheck(path: &[u16]) -> Option<Recheck> {
    let h = open_attributes_only(path)?;
    let mut info: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            h.raw(),
            FileStandardInfo,
            &mut info as *mut FILE_STANDARD_INFO as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    Some(Recheck {
        allocation: info.AllocationSize as u64,
        length: info.EndOfFile as u64,
        links: info.NumberOfLinks,
    })
}

/// One directory entry, from either information class.
struct Entry<'a> {
    name: &'a [u16],
    attributes: u32,
    length: u64,
    allocation: u64,
    reparse_tag: u32,
    file_id: u128,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InfoClass {
    Extended,
    Both,
}

/// Lists a directory handle, calling `f` for every entry except `.` and `..`.
fn list_dir(
    h: &OwnedHandle,
    class: &mut InfoClass,
    buf: &mut [u64],
    mut f: impl FnMut(&Entry),
) -> Result<(), u32> {
    let bytes = std::mem::size_of_val(buf) as u32;
    loop {
        let info_class = match class {
            InfoClass::Extended => FileIdExtdDirectoryInfo,
            InfoClass::Both => FileIdBothDirectoryInfo,
        };
        let ok = unsafe {
            GetFileInformationByHandleEx(h.raw(), info_class, buf.as_mut_ptr().cast(), bytes)
        };
        if ok == 0 {
            let e = last_error();
            if e == ERROR_NO_MORE_FILES {
                return Ok(());
            }
            if *class == InfoClass::Extended
                && (e == ERROR_INVALID_PARAMETER || e == ERROR_NOT_SUPPORTED)
            {
                // FAT, exFAT and some network file systems lack the extended class.
                *class = InfoClass::Both;
                continue;
            }
            return Err(e);
        }
        let base = buf.as_ptr() as *const u8;
        let mut offset = 0usize;
        loop {
            // SAFETY: Windows filled `buf` with a chain of 8-byte-aligned records; each
            // record's name length is bounded by the buffer.
            let next = unsafe {
                match class {
                    InfoClass::Extended => {
                        let r = &*(base.add(offset) as *const FILE_ID_EXTD_DIR_INFO);
                        let name = std::slice::from_raw_parts(
                            r.FileName.as_ptr(),
                            r.FileNameLength as usize / 2,
                        );
                        let e = Entry {
                            name,
                            attributes: r.FileAttributes,
                            length: r.EndOfFile as u64,
                            allocation: r.AllocationSize as u64,
                            reparse_tag: r.ReparsePointTag,
                            file_id: u128::from_le_bytes(r.FileId.Identifier),
                        };
                        if !is_dot(e.name) {
                            f(&e);
                        }
                        r.NextEntryOffset
                    }
                    InfoClass::Both => {
                        let r = &*(base.add(offset) as *const FILE_ID_BOTH_DIR_INFO);
                        let name = std::slice::from_raw_parts(
                            r.FileName.as_ptr(),
                            r.FileNameLength as usize / 2,
                        );
                        let reparse = r.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
                        let e = Entry {
                            name,
                            attributes: r.FileAttributes,
                            length: r.EndOfFile as u64,
                            allocation: r.AllocationSize as u64,
                            // For reparse points this class reports the tag in EaSize.
                            reparse_tag: if reparse { r.EaSize } else { 0 },
                            file_id: r.FileId as u64 as u128,
                        };
                        if !is_dot(e.name) {
                            f(&e);
                        }
                        r.NextEntryOffset
                    }
                }
            };
            if next == 0 {
                break;
            }
            offset += next as usize;
        }
    }
}

fn is_dot(name: &[u16]) -> bool {
    name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16]
}

fn upper(w: &[u16]) -> Vec<u16> {
    String::from_utf16_lossy(w).to_uppercase().encode_utf16().collect()
}

pub(crate) struct Walker {
    excluded: Vec<Vec<u16>>,
    buf: Vec<u64>,
    class: InfoClass,
    /// Large files already counted, by (volume serial, file ID), across every root of the
    /// scan: a file with one name in each of two roots is still counted once.
    seen: HashSet<(u64, u128)>,
}

impl Walker {
    pub fn new(excluded: &[PathBuf]) -> Walker {
        let excluded = excluded
            .iter()
            .filter_map(|p| verbatim(p).ok())
            .map(|w| upper(&w))
            .collect();
        Walker { excluded, buf: vec![0u64; 64 * 1024 / 8], class: InfoClass::Extended, seen: HashSet::new() }
    }

    fn is_excluded(&self, path: &[u16]) -> bool {
        if self.excluded.is_empty() {
            return false;
        }
        let mut candidates = self.excluded.iter().filter(|e| e.len() == path.len()).peekable();
        if candidates.peek().is_none() {
            return false;
        }
        let up = upper(path);
        candidates.any(|e| *e == up)
    }

    pub fn walk_root(&mut self, root: &Path, c: &mut Collector) -> RootReport {
        let display = super::root_display(root);
        let mut report =
            RootReport { path: display.clone(), node: None, bytes_on_disk: 0, files: 0, is_link: false, error: None };
        let root_w = match verbatim(root) {
            Ok(w) => w,
            Err(e) => {
                report.error = Some(e.to_string());
                c.add_error(display, e.raw_os_error().unwrap_or(0) as u32);
                return report;
            }
        };
        let h = match open_dir(&root_w) {
            Ok(h) => h,
            Err(code) => {
                report.error = Some(std::io::Error::from_raw_os_error(code as i32).to_string());
                c.add_error(display, code);
                return report;
            }
        };

        // The root itself may be a link: record it, do not walk into it.
        let mut tag: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                h.raw(),
                FileAttributeTagInfo,
                &mut tag as *mut FILE_ATTRIBUTE_TAG_INFO as *mut core::ffi::c_void,
                std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        let root_is_link = ok != 0
            && tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            && is_name_surrogate(tag.ReparseTag);
        // GUARD-BEGIN(root-link)
        if root_is_link {
            report.is_link = true;
            c.add_link(display, link_kind(tag.ReparseTag), true);
            return report;
        }
        // GUARD-END(root-link)
        if ok != 0 && tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            report.error = Some("not a folder".into());
            c.add_error(display, 267); // ERROR_DIRECTORY
            return report;
        }

        let volume = volume_serial(&h);
        let mut seen = std::mem::take(&mut self.seen);
        let root_idx = c.add_dir(display, NO_PARENT);
        report.node = Some(root_idx);

        // Depth-first walk with an explicit stack: (node, verbatim path, handle if open).
        let mut stack: Vec<(u32, Vec<u16>, Option<OwnedHandle>)> = vec![(root_idx, root_w, Some(h))];
        while let Some((dir, path, handle)) = stack.pop() {
            if c.cancelled() {
                break;
            }
            let h = match handle {
                Some(h) => h,
                None => match open_dir(&path) {
                    Ok(h) => h,
                    Err(code) => {
                        c.add_error(display_path(&path), code);
                        continue;
                    }
                },
            };
            c.between_dirs(|| display_path(&path));

            let mut subdirs: Vec<(Vec<u16>, Vec<u16>)> = Vec::new();
            let mut class = self.class;
            let mut buf = std::mem::take(&mut self.buf);
            let excluded_check = |p: &[u16]| self.is_excluded(p);
            let result = list_dir(&h, &mut class, &mut buf, |e| {
                let mut full = Vec::with_capacity(path.len() + 1 + e.name.len());
                full.extend_from_slice(&path);
                if full.last() != Some(&(b'\\' as u16)) {
                    full.push(b'\\' as u16);
                }
                full.extend_from_slice(e.name);

                // GUARD-BEGIN(no-follow-links)
                if e.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && is_name_surrogate(e.reparse_tag) {
                    let is_dir = e.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
                    c.add_link(display_path(&full), link_kind(e.reparse_tag), is_dir);
                    return;
                }
                // GUARD-END(no-follow-links)

                if e.attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                    if excluded_check(&full) {
                        c.add_excluded(display_path(&full));
                    } else {
                        subdirs.push((e.name.to_vec(), full));
                    }
                    return;
                }

                measure_file(e, &full, dir, volume, &mut seen, c);
            });
            self.buf = buf;
            self.class = class;
            drop(h);
            if let Err(code) = result {
                c.add_error(display_path(&path), code);
            }
            // Push in reverse so the walk visits names in listing order.
            for (name, full) in subdirs.into_iter().rev() {
                let idx = c.add_dir(crate::win::from_wide(&name), dir);
                stack.push((idx, full, None));
            }
        }
        self.seen = seen;
        report
    }
}

#[allow(unused_assignments, unused_mut)]
fn measure_file(
    e: &Entry,
    full: &[u16],
    dir: u32,
    volume: u64,
    seen: &mut HashSet<(u64, u128)>,
    c: &mut Collector,
) {
    let placeholder = e.attributes
        & (FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_OFFLINE)
        != 0;
    let mut length = e.length;
    let mut on_disk = e.length;
    let mut links = 1u32;

    // Size on disk is the allocation, not the length. For compressed and sparse files NTFS
    // reports the bytes really allocated here (measured on the CI runner: equal to
    // GetCompressedFileSizeW), so no per-file call is needed.
    // GUARD-BEGIN(size-on-disk)
    on_disk = e.allocation;
    // GUARD-END(size-on-disk)

    // Large files: size and link count from the file itself (another name's directory
    // entry can hold a stale size). Placeholders are never opened.
    if !placeholder && on_disk.max(length) >= VERIFY_THRESHOLD {
        // GUARD-BEGIN(large-file-recheck)
        if let Some(r) = recheck(full) {
            on_disk = r.allocation;
            length = r.length;
            links = r.links;
        }
        // GUARD-END(large-file-recheck)
    }

    // Cloud placeholders and offline files count 0 and are never read.
    if placeholder {
        c.totals.placeholders += 1;
        // GUARD-BEGIN(placeholder-zero)
        on_disk = 0;
        // GUARD-END(placeholder-zero)
    }

    // One count per file: a second name of a remembered file counts 0 bytes.
    // GUARD-BEGIN(hardlink-dedup)
    if on_disk >= DEDUP_THRESHOLD && !seen.insert((volume, e.file_id)) {
        c.totals.hardlink_duplicates += 1;
        on_disk = 0;
    }
    // GUARD-END(hardlink-dedup)

    c.add_file(dir, on_disk, length);
    if c.wants_top(on_disk) {
        c.add_top(TopFile { path: display_path(full), bytes_on_disk: on_disk, logical_bytes: length, links });
    }
}

/// The serial number of the volume a handle is on, so file IDs from different volumes never
/// collide.
fn volume_serial(h: &OwnedHandle) -> u64 {
    let mut id: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            h.raw(),
            FileIdInfo,
            &mut id as *mut FILE_ID_INFO as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if ok != 0 {
        return id.VolumeSerialNumber;
    }
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(h.raw(), &mut info) } != 0 {
        info.dwVolumeSerialNumber as u64
    } else {
        0
    }
}
