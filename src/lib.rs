#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(not(windows))]
compile_error!("clipboard_share 仅支持 Windows 10 1809 及以上版本");

pub mod autostart;
pub mod clipboard;
pub mod config;
pub mod network;
pub mod process_control;
pub mod protocol;
pub mod service;
pub mod windows_runtime;

pub use config::{AppConfig, AutoPairMode};
pub use service::{AppStatus, PeerStatus, SyncService, validate_code};
