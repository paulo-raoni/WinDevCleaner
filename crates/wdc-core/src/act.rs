//! Allowed and forbidden roots (plan, section 5.3). `explain` reports this verdict in slice 3;
//! `apply` (slice 4) checks it on the open handle of every item before moving anything.
//!
//! The verdict is computed on the item's real final path, as Windows resolves it from an
//! open handle (`GetFinalPathNameByHandleW`), never on the text the caller typed.

use serde::Serialize;

use crate::i18n::{Lang, Msg};
use crate::rules::{canon, Context};

/// Why a path can never be acted on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Forbid {
    VolumeRoot,
    Windows,
    ProgramFiles,
    UserProfile,
    PersonalFolder,
    OneDrive,
    Secrets,
    VirtualDiskOrMailbox,
    MessengerData,
    BrowserProfile,
    Quarantine,
    GitRepository,
}

impl Forbid {
    pub fn text(&self, lang: Lang) -> &'static str {
        lang.t(match self {
            Forbid::VolumeRoot => Msg::ForbidVolumeRoot,
            Forbid::Windows => Msg::ForbidWindows,
            Forbid::ProgramFiles => Msg::ForbidProgramFiles,
            Forbid::UserProfile => Msg::ForbidUserProfile,
            Forbid::PersonalFolder => Msg::ForbidPersonalFolder,
            Forbid::OneDrive => Msg::ForbidOneDrive,
            Forbid::Secrets => Msg::ForbidSecrets,
            Forbid::VirtualDiskOrMailbox => Msg::ForbidVirtualDisk,
            Forbid::MessengerData => Msg::ForbidMessenger,
            Forbid::BrowserProfile => Msg::ForbidBrowserProfile,
            Forbid::Quarantine => Msg::ForbidQuarantine,
            Forbid::GitRepository => Msg::ForbidGit,
        })
    }
}

/// Whether the tool may act on a path at all, whatever a rule says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Verdict {
    /// The allowed root the path is inside.
    pub allowed_root: Option<String>,
    /// The path is an allowed root itself: only things inside a root can be items.
    pub is_allowed_root: bool,
    /// Why the path is forbidden, if it is.
    pub forbidden: Option<Forbid>,
}

impl Verdict {
    pub fn may_act(&self) -> bool {
        self.allowed_root.is_some() && !self.is_allowed_root && self.forbidden.is_none()
    }
}

/// The real path of an existing folder (long names, links resolved), else the text itself.
/// Environment variables may hold short names (`C:\Users\RUNNER~1\...`), while the paths
/// checked come from Windows' final path, in long form.
fn real(p: &str) -> String {
    match std::fs::canonicalize(crate::rules::native(&canon(p))) {
        Ok(r) => {
            let t = r.to_string_lossy().into_owned();
            canon(t.strip_prefix(r"\\?\").unwrap_or(&t))
        }
        Err(_) => canon(p),
    }
}

fn lower(p: &str) -> String {
    canon(p).to_lowercase()
}

fn is_under(path: &str, base: &str) -> bool {
    let (p, b) = (lower(path), lower(base));
    p == b || (p.starts_with(&b) && p.as_bytes().get(b.len()) == Some(&b'\\'))
}

/// The allowed roots (act scope) on this machine.
pub fn allowed_roots(ctx: &Context) -> Vec<String> {
    let mut out = Vec::new();
    for v in ["%LOCALAPPDATA%", "%APPDATA%", "%TEMP%"] {
        if let Some(p) = ctx.expand(v) {
            out.push(real(&p));
        }
    }
    for sub in [
        r"%USERPROFILE%\.vscode",
        r"%USERPROFILE%\.vscode-insiders",
        r"%USERPROFILE%\.cursor",
        r"%USERPROFILE%\.cache",
        r"%USERPROFILE%\.nuget\packages",
        r"%USERPROFILE%\.gradle\caches",
    ] {
        if let Some(p) = ctx.expand(sub) {
            out.push(real(&p));
        }
    }
    out
}

/// Why `path` is forbidden, if it is (plan 5.3, "Forbidden, always").
pub fn forbidden_reason(path: &str, is_dir: bool, ctx: &Context, data_dir: Option<&str>) -> Option<Forbid> {
    let p = canon(path);
    let pl = p.to_lowercase();
    let segs: Vec<&str> = pl.split('\\').filter(|s| !s.is_empty()).collect();
    if segs.len() <= 1 {
        return Some(Forbid::VolumeRoot);
    }
    for (var, why) in [
        ("%WINDIR%", Forbid::Windows),
        ("%ProgramFiles%", Forbid::ProgramFiles),
        ("%ProgramFiles(x86)%", Forbid::ProgramFiles),
        ("%ProgramW6432%", Forbid::ProgramFiles),
    ] {
        if let Some(base) = ctx.expand(var) {
            if is_under(&p, &real(&base)) {
                return Some(why);
            }
        }
    }
    if let Some(profile) = ctx.expand("%USERPROFILE%").map(|p| real(&p)) {
        if lower(&profile) == pl {
            return Some(Forbid::UserProfile);
        }
        for f in ["Documents", "Desktop", "Pictures", "Videos", "Music", "Downloads", ".ssh", ".gnupg"] {
            if is_under(&p, &format!("{profile}\\{f}")) {
                return Some(if f.starts_with('.') { Forbid::Secrets } else { Forbid::PersonalFolder });
            }
        }
        if segs.len() > lower(&profile).split('\\').filter(|s| !s.is_empty()).count() {
            let first = &segs[lower(&profile).split('\\').filter(|s| !s.is_empty()).count()];
            if is_under(&p, &profile) && first.starts_with("onedrive") {
                return Some(Forbid::OneDrive);
            }
        }
    }
    for var in ["%OneDrive%", "%OneDriveConsumer%", "%OneDriveCommercial%"] {
        if let Some(base) = ctx.expand(var) {
            if is_under(&p, &base) {
                return Some(Forbid::OneDrive);
            }
        }
    }
    for sub in [r"Microsoft\Protect", r"Microsoft\Credentials", r"Microsoft\Vault"] {
        for root in ["%APPDATA%", "%LOCALAPPDATA%"] {
            if let Some(base) = ctx.expand(&format!("{root}\\{sub}")) {
                if is_under(&p, &base) {
                    return Some(Forbid::Secrets);
                }
            }
        }
    }
    let name = segs.last().copied().unwrap_or("");
    for ext in [".vhd", ".vhdx", ".pst", ".ost"] {
        if name.ends_with(ext) {
            return Some(Forbid::VirtualDiskOrMailbox);
        }
    }
    if segs.iter().any(|s| s.contains("whatsapp") || s.contains("telegram") || *s == "signal") {
        return Some(Forbid::MessengerData);
    }
    // Browser profiles: only named cache subfolders may be touched.
    if let Some(i) = segs.iter().position(|s| *s == "user data") {
        let rest = &segs[i + 1..];
        let cache = rest.iter().any(|s| {
            ["cache", "code cache", "gpucache", "shadercache", "grshadercache", "dawncache"].contains(s)
        });
        if !cache {
            return Some(Forbid::BrowserProfile);
        }
    }
    if let Some(d) = data_dir {
        if is_under(&p, &format!("{d}\\quarantine")) || is_under(&p, &format!("{}\\quarantine", real(d))) {
            return Some(Forbid::Quarantine);
        }
    }
    if segs.get(1) == Some(&crate::quarantine::VOLUME_FOLDER) {
        return Some(Forbid::Quarantine);
    }
    if is_dir && crate::rules::native(&format!("{p}\\.git")).exists() {
        return Some(Forbid::GitRepository);
    }
    None
}

/// The verdict for a final path.
pub fn verdict(final_path: &str, is_dir: bool, ctx: &Context, data_dir: Option<&str>) -> Verdict {
    let roots = allowed_roots(ctx);
    let p = lower(final_path);
    let is_allowed_root = roots.iter().any(|r| r.to_lowercase() == p);
    // The deepest allowed root that contains the path.
    let allowed_root = roots
        .iter()
        .filter(|r| is_under(final_path, r))
        .max_by_key(|r| r.len())
        .cloned();
    Verdict { allowed_root, is_allowed_root, forbidden: forbidden_reason(final_path, is_dir, ctx, data_dir) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ctx() -> Context {
        let env: HashMap<String, String> = [
            ("USERPROFILE", r"C:\Users\me"),
            ("LOCALAPPDATA", r"C:\Users\me\AppData\Local"),
            ("APPDATA", r"C:\Users\me\AppData\Roaming"),
            ("TEMP", r"C:\Users\me\AppData\Local\Temp"),
            ("WINDIR", r"C:\Windows"),
            ("ProgramFiles", r"C:\Program Files"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        Context::new(env, None, None)
    }

    #[test]
    fn allowed_roots_are_never_items_and_forbidden_places_win() {
        let c = ctx();
        let v = verdict(r"C:\Users\me\AppData\Local\Temp", true, &c, None);
        assert!(v.is_allowed_root && !v.may_act());
        let v = verdict(r"C:\Users\me\AppData\Local\Temp\x", true, &c, None);
        assert!(v.may_act(), "{v:?}");
        assert_eq!(v.allowed_root.as_deref(), Some(r"C:\Users\me\AppData\Local\Temp"));
        for (path, why) in [
            (r"C:\Windows\WinSxS", Forbid::Windows),
            (r"C:\Program Files\App", Forbid::ProgramFiles),
            (r"C:\Users\me\Documents\x", Forbid::PersonalFolder),
            (r"C:\Users\me\.ssh\id_ed25519", Forbid::Secrets),
            (r"C:\Users\me\AppData\Roaming\Microsoft\Protect\S-1", Forbid::Secrets),
            (r"C:\Users\me\AppData\Local\Docker\wsl\disk.vhdx", Forbid::VirtualDiskOrMailbox),
            (r"C:\Users\me\AppData\Local\Google\Chrome\User Data\Default\History", Forbid::BrowserProfile),
            (r"C:\", Forbid::VolumeRoot),
            (r"C:\Users\me", Forbid::UserProfile),
        ] {
            let v = verdict(path, false, &c, None);
            assert_eq!(v.forbidden.as_ref(), Some(&why), "{path}: {v:?}");
            assert!(!v.may_act());
        }
        let v = verdict(r"C:\Users\me\AppData\Local\Google\Chrome\User Data\Default\Cache", true, &c, None);
        assert!(v.forbidden.is_none(), "{v:?}");
        let v = verdict(r"D:\games\x", true, &c, None);
        assert!(v.allowed_root.is_none() && !v.may_act());
    }
}
