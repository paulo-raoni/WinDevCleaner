//! WinDevCleaner engine.
//!
//! Every face of the tool (`wdc.exe`, `WinDevCleaner.exe` and `wdc mcp`) calls this library;
//! none has cleaning logic of its own. Slice 1 is read-only: nothing here changes the disk
//! outside the tool's own data folder (snapshots and the activity log).

pub mod activity;
pub mod background;
pub mod fmt;
pub mod i18n;
pub mod link;
pub mod scan;
pub mod scope;
pub mod snapshot;
pub mod time;
pub mod tools;

#[cfg(windows)]
pub mod win;

/// Version of the tool, shared by every face.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
