//! Embeds the WinDevCleaner icon (assets/windevcleaner.ico) as the executable's Windows
//! resource, the icon Explorer, the Start menu and the taskbar show.
//!
//! The resource compiler is only used on a Windows host. Anywhere else (the Linux cloud
//! session, including a `--target x86_64-pc-windows-msvc` check) this does nothing but say so;
//! it never fails the build.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/windevcleaner.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    embed();
}

#[cfg(windows)]
fn embed() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/windevcleaner.ico");
    if let Err(e) = res.compile() {
        // Never fail the build over an icon.
        println!("cargo:warning=the icon was not embedded: {e}");
    }
}

#[cfg(not(windows))]
fn embed() {
    println!("cargo:warning=the icon is not embedded: the Windows resource compiler is only used on a Windows host");
}
