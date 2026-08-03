use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DEFAULT_PORT: u16 = 24817;
pub const DEFAULT_HISTORY_LIMIT: usize = 20;
pub const DEFAULT_MAX_ITEM_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const DEFAULT_CACHE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub device_id: Uuid,
    pub device_name: String,
    pub listen_port: u16,
    pub history_limit: usize,
    pub max_item_bytes: u64,
    pub cache_bytes: u64,
    #[serde(default)]
    pub pairing_code: Option<String>,
    #[serde(default)]
    pub auto_pair: Option<AutoPairConfig>,
    pub peers: Vec<PeerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoPairConfig {
    #[serde(default)]
    pub enabled: bool,
    pub mode: AutoPairMode,
    #[serde(default)]
    pub peer_address: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AutoPairMode {
    Listen,
    Connect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerConfig {
    pub device_id: Uuid,
    pub device_name: String,
    pub address: String,
    pub protected_key: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            device_id: Uuid::new_v4(),
            device_name: env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows-PC".to_owned()),
            listen_port: DEFAULT_PORT,
            history_limit: DEFAULT_HISTORY_LIMIT,
            max_item_bytes: DEFAULT_MAX_ITEM_BYTES,
            cache_bytes: DEFAULT_CACHE_BYTES,
            pairing_code: None,
            auto_pair: None,
            peers: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn load_or_create() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            let config = Self::default();
            config.save()?;
            return Ok(config);
        }
        let bytes =
            fs::read(&path).with_context(|| format!("无法读取配置文件 {}", path.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("配置文件格式错误 {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        let parent = path.parent().context("配置文件路径没有父目录")?;
        fs::create_dir_all(parent)?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        if path.exists() {
            fs::remove_file(&path)?;
        }
        fs::rename(&temporary, &path)?;
        Ok(())
    }

    pub fn peer_key(&self, id: Uuid) -> Result<[u8; 32]> {
        let peer = self
            .peers
            .iter()
            .find(|peer| peer.device_id == id)
            .with_context(|| format!("设备 {id} 尚未配对"))?;
        unprotect_key(&peer.protected_key)
    }

    pub fn upsert_peer(
        &mut self,
        device_id: Uuid,
        device_name: String,
        address: String,
        key: &[u8; 32],
    ) -> Result<()> {
        let peer = PeerConfig {
            device_id,
            device_name,
            address,
            protected_key: protect_key(key)?,
        };
        if let Some(existing) = self
            .peers
            .iter_mut()
            .find(|existing| existing.device_id == device_id)
        {
            *existing = peer;
        } else {
            self.peers.push(peer);
        }
        self.save()
    }

    pub fn remove_peer(&mut self, id: Uuid) -> Result<bool> {
        let old_len = self.peers.len();
        self.peers.retain(|peer| peer.device_id != id);
        let removed = old_len != self.peers.len();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }
}

pub fn app_dir() -> Result<PathBuf> {
    let parent = config_path()?
        .parent()
        .context("配置文件路径没有父目录")?
        .to_owned();
    Ok(parent.join("data"))
}

pub fn install_dir() -> Result<PathBuf> {
    let local = env::var_os("LOCALAPPDATA").context("环境变量 LOCALAPPDATA 不存在")?;
    Ok(PathBuf::from(local).join("ClipboardShare"))
}

pub fn config_path() -> Result<PathBuf> {
    if let Some(explicit) = env::var_os("CLIPBOARD_SHARE_CONFIG") {
        return Ok(PathBuf::from(explicit));
    }

    let current = env::current_dir()?.join("config.json");
    if current.exists() {
        return Ok(current);
    }

    if let Ok(executable) = env::current_exe()
        && let Some(parent) = executable.parent()
    {
        let beside_executable = parent.join("config.json");
        if beside_executable.exists() {
            return Ok(beside_executable);
        }
    }
    Ok(current)
}

pub fn cache_dir() -> Result<PathBuf> {
    Ok(app_dir()?.join("cache"))
}

pub fn log_dir() -> Result<PathBuf> {
    Ok(app_dir()?.join("logs"))
}

pub fn random_pairing_code() -> String {
    let value: u32 = rand::rng().random_range(0..1_000_000);
    format!("{value:06}")
}

fn protect_key(key: &[u8; 32]) -> Result<String> {
    Ok(BASE64.encode(dpapi_protect(key)?))
}

fn unprotect_key(encoded: &str) -> Result<[u8; 32]> {
    let encrypted = BASE64.decode(encoded).context("配对密钥 Base64 损坏")?;
    let plaintext = dpapi_unprotect(&encrypted)?;
    if plaintext.len() != 32 {
        bail!("配对密钥长度错误");
    }
    let mut key = [0_u8; 32];
    key.copy_from_slice(&plaintext);
    Ok(key)
}

fn dpapi_protect(input: &[u8]) -> Result<Vec<u8>> {
    use windows::{
        Win32::{
            Foundation::LocalFree,
            Security::Cryptography::{
                CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
            },
        },
        core::w,
    };

    let input_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len().try_into()?,
        pbData: input.as_ptr().cast_mut(),
    };
    let mut output_blob = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input_blob,
            w!("ClipboardShare peer key"),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output_blob,
        )?;
        let output =
            std::slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            output_blob.pbData.cast(),
        )));
        Ok(output)
    }
}

fn dpapi_unprotect(input: &[u8]) -> Result<Vec<u8>> {
    use windows::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
        },
    };

    let input_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len().try_into()?,
        pbData: input.as_ptr().cast_mut(),
    };
    let mut output_blob = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output_blob,
        )?;
        let output =
            std::slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            output_blob.pbData.cast(),
        )));
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_code_has_six_digits() {
        let code = random_pairing_code();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|character| character.is_ascii_digit()));
    }
}
