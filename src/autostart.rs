use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

use crate::config::{config_path, install_dir};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ClipboardShare";

pub fn install() -> Result<PathBuf> {
    let source = env::current_exe().context("无法定位当前可执行文件")?;
    let destination_dir = install_dir()?;
    fs::create_dir_all(&destination_dir)?;

    let source_dir = source
        .parent()
        .context("可执行文件路径没有父目录")?
        .to_path_buf();
    let source_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    let cli_source = if source_name.eq_ignore_ascii_case("clipboard_share_ui.exe") {
        source_dir.join("clipboard_share.exe")
    } else {
        source.clone()
    };
    let ui_source = if source_name.eq_ignore_ascii_case("clipboard_share_ui.exe") {
        source.clone()
    } else {
        source_dir.join("clipboard_share_ui.exe")
    };

    let cli_destination = destination_dir.join("clipboard_share.exe");
    if cli_source.exists() {
        copy_file(&cli_source, &cli_destination)?;
    }

    let ui_destination = destination_dir.join("clipboard_share_ui.exe");
    let has_ui = ui_source.exists();
    if has_ui {
        copy_file(&ui_source, &ui_destination)?;
    }

    let source_config = config_path()?;
    let installed_config = destination_dir.join("config.json");
    if source_config.exists() && source_config != installed_config {
        fs::copy(&source_config, &installed_config)?;
    }

    let launch = if has_ui {
        format!("\"{}\"", ui_destination.display())
    } else {
        format!("\"{}\" daemon --background", cli_destination.display())
    };

    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = current_user.create_subkey(RUN_KEY)?;
    run.set_value(VALUE_NAME, &launch)?;

    Ok(if has_ui {
        ui_destination
    } else {
        cli_destination
    })
}

pub fn uninstall() -> Result<()> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = current_user.create_subkey(RUN_KEY)?;
    match run.delete_value(VALUE_NAME) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn copy_file(source: &std::path::Path, destination: &std::path::Path) -> Result<()> {
    if source == destination {
        return Ok(());
    }
    fs::copy(source, destination).with_context(|| {
        format!(
            "无法将 {} 复制到 {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}
