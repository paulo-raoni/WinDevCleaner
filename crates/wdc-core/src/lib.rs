//! WinDevCleaner engine.
//!
//! Every face of the tool (`wdc.exe`, `WinDevCleaner.exe` and `wdc mcp`) calls this library;
//! none has cleaning logic of its own. Scanning, explaining and planning are read-only; only
//! `apply`, `restore` and `purge` change the disk, after proof that a person is present.

pub mod act;
pub mod activity;
pub mod apply;
pub mod background;
pub mod diff;
pub mod explain;
pub mod facts;
pub mod fmt;
pub mod fsops;
pub mod i18n;
pub mod link;
pub mod plan;
pub mod presence;
pub mod programs;
pub mod quarantine;
pub mod rules;
pub mod scan;
pub mod scope;
pub mod snapshot;
pub mod system;
pub mod time;
pub mod tools;

#[cfg(windows)]
pub mod win;

/// Version of the tool, shared by every face.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
