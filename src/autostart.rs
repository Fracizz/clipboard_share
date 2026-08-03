use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

use crate::config::{config_path, install_dir};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ClipboardShare";

pub fn install() -> Result<PathBuf> {
    let source = env::current_exe().context("无法定位当前可执行文件")?;
    let destination = install_dir()?.join("clipboard_share.exe");
    fs::create_dir_all(destination.parent().context("安装路径没有父目录")?)?;
    if source != destination {
        fs::copy(&source, &destination).with_context(|| {
            format!(
                "无法将 {} 复制到 {}",
                source.display(),
                destination.display()
            )
        })?;
    }
    let source_config = config_path()?;
    let installed_config = destination
        .parent()
        .context("安装路径没有父目录")?
        .join("config.json");
    if source_config.exists() && source_config != installed_config {
        fs::copy(&source_config, &installed_config)?;
    }

    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = current_user.create_subkey(RUN_KEY)?;
    run.set_value(
        VALUE_NAME,
        &format!("\"{}\" daemon --background", destination.display()),
    )?;
    Ok(destination)
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
