#!/usr/bin/env python3
"""Run transport, capture-budget and config-monitor regressions on Linux.

Usage: python3 tools/test_transport.py [cargo test arguments, e.g. --offline]
This does not validate Windows clipboard integration or DPAPI.
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

repo = Path(__file__).resolve().parents[1]
manifest = (repo / "Cargo.toml").read_text()
dependencies = manifest.split("[dependencies]\n", 1)[1].split("[dev-dependencies]", 1)[0]
dependencies = "\n".join(
    line for line in dependencies.splitlines()
    if not line.startswith(("windows", "winreg"))
)
with tempfile.TemporaryDirectory(prefix="clipboard-transport-") as directory:
    crate = Path(directory)
    (crate / "src").mkdir()
    (crate / "Cargo.toml").write_text(
        '[package]\nname="clipboard-transport-tests"\nversion="0.0.0"\nedition="2024"\n'
        '[dependencies]\n' + dependencies + '\n[dev-dependencies]\n'
        'tokio = { version="1.53.1", features=["test-util"] }\n'
    )
    # Seed resolution with the application's exact dependency versions.
    (crate / "Cargo.lock").write_bytes((repo / "Cargo.lock").read_bytes())
    # Retain the repository's config types and defaults. Tests must not invoke DPAPI.
    config = (repo / "src/config.rs").read_text().split("fn dpapi_protect", 1)[0]
    config += '''fn dpapi_protect(_: &[u8]) -> Result<Vec<u8>> { bail!("DPAPI unavailable in transport harness") }
fn dpapi_unprotect(_: &[u8]) -> Result<Vec<u8>> { bail!("DPAPI unavailable in transport harness") }
'''
    (crate / "src/config.rs").write_text(config)
    source = '''#![allow(dead_code)]
#[path = "PROTOCOL_PATH"] mod protocol;
#[path = "NETWORK_PATH"] mod network;
mod config;
mod clipboard {
    use std::path::{Path, PathBuf};
    use crate::protocol::ClipboardItem;
    #[derive(Debug)]
    pub struct ClipboardCapture { pub item: ClipboardItem, pub source_files: Vec<SourceFile> }
    #[derive(Debug)]
    pub struct SourceFile { pub source: PathBuf, pub relative_path: PathBuf }
    pub async fn apply(_: &ClipboardItem, _: &Path) -> anyhow::Result<()> {
        panic!("OS clipboard must not be invoked in transport regressions")
    }
}
'''
    source = source.replace("PROTOCOL_PATH", (repo / "src/protocol.rs").as_posix())
    source = source.replace("NETWORK_PATH", (repo / "src/network.rs").as_posix())
    # Extract platform-neutral implementations verbatim; do not reimplement logic in tests.
    capture = (repo / "src/clipboard.rs").read_text()
    capture_module = '''
use std::{fs::File, io::{BufReader, Read}, path::{Path, PathBuf}};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;
use uuid::Uuid;
use crate::{clipboard::SourceFile, protocol::{FileEntry, MAX_ITEM_BYTES}};
'''
    capture_module += "#[derive(Debug)]\nstruct CaptureTooLarge" + capture.split("struct CaptureTooLarge", 1)[1].split("#[derive(Debug, Clone)]", 1)[0]
    capture_module += "fn should_capture_sequence" + capture.split("fn should_capture_sequence", 1)[1].split("pub async fn apply", 1)[0]
    capture_module += "fn collect_files" + capture.split("fn collect_files", 1)[1].split("async fn top_level_received_items", 1)[0]
    capture_module += "#[cfg(test)]" + capture.split("#[cfg(test)]", 1)[1]
    (crate / "src/capture_regressions.rs").write_text(capture_module)
    service = (repo / "src/service.rs").read_text()
    monitor_module = "use std::time::Duration; use anyhow::Result; use uuid::Uuid; use crate::config::AppConfig;\n"
    monitor_module += "async fn wait_for_config_change" + service.split("async fn wait_for_config_change", 1)[1].split("async fn wait_flag", 1)[0]
    monitor_module += "#[cfg(test)]" + service.split("#[cfg(test)]", 1)[1]
    (crate / "src/config_regressions.rs").write_text(monitor_module)
    source += "#[cfg(test)] mod capture_regressions;\n#[cfg(test)] mod config_regressions;\n"
    (crate / "src/lib.rs").write_text(source)
    environment = os.environ.copy()
    environment.setdefault("CARGO_TARGET_DIR", str(repo / "target/transport-tests"))
    result = subprocess.run(
        ["cargo", "test", "--manifest-path", str(crate / "Cargo.toml"), *sys.argv[1:]],
        env=environment,
    )
    sys.exit(result.returncode)
