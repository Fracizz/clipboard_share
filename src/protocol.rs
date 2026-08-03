use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chacha20poly1305::{
    ChaCha20Poly1305, Nonce,
    aead::{Aead, KeyInit},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_SIZE: usize = 64 * 1024 * 1024;
pub const FILE_CHUNK_SIZE: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardItem {
    pub id: Uuid,
    pub origin: Uuid,
    pub created_unix_ms: u64,
    pub formats: Vec<ClipboardFormat>,
    pub files: Vec<FileEntry>,
    pub content_hash: String,
}

impl ClipboardItem {
    pub fn new(
        origin: Uuid,
        created_unix_ms: u64,
        formats: Vec<ClipboardFormat>,
        files: Vec<FileEntry>,
    ) -> Result<Self> {
        let mut item = Self {
            id: Uuid::new_v4(),
            origin,
            created_unix_ms,
            formats,
            files,
            content_hash: String::new(),
        };
        item.content_hash = item.calculate_hash()?;
        Ok(item)
    }

    pub fn calculate_hash(&self) -> Result<String> {
        let mut normalized = self.clone();
        normalized.id = Uuid::nil();
        normalized.origin = Uuid::nil();
        normalized.created_unix_ms = 0;
        normalized.content_hash.clear();
        let bytes = serde_json::to_vec(&normalized)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }

    pub fn verify_hash(&self) -> Result<()> {
        if self.calculate_hash()? != self.content_hash {
            bail!("剪贴板内容哈希校验失败");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardFormat {
    pub name: String,
    pub data_base64: String,
}

impl ClipboardFormat {
    pub fn from_bytes(name: impl Into<String>, data: &[u8]) -> Self {
        Self {
            name: name.into(),
            data_base64: BASE64.encode(data),
        }
    }

    pub fn bytes(&self) -> Result<Vec<u8>> {
        BASE64
            .decode(&self.data_base64)
            .with_context(|| format!("无法解码剪贴板格式 {}", self.name))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub relative_path: PathBuf,
    pub size: u64,
    pub sha256: String,
    pub is_directory: bool,
}

impl FileEntry {
    pub fn validate_path(&self) -> Result<()> {
        validate_relative_path(&self.relative_path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "data")]
pub enum Message {
    Hello {
        version: u16,
        device_id: Uuid,
        device_name: String,
        proof: String,
    },
    Clipboard(ClipboardItem),
    FileChunk {
        item_id: Uuid,
        relative_path: PathBuf,
        offset: u64,
        data_base64: String,
        eof: bool,
    },
    Ack {
        item_id: Uuid,
    },
    Ping,
    Pong,
    Error {
        message: String,
    },
}

pub struct SecureChannel {
    stream: TcpStream,
    send_cipher: ChaCha20Poly1305,
    receive_cipher: ChaCha20Poly1305,
    send_counter: u64,
    receive_counter: u64,
}

impl SecureChannel {
    pub fn new(stream: TcpStream, send_key: &[u8; 32], receive_key: &[u8; 32]) -> Self {
        Self {
            stream,
            send_cipher: ChaCha20Poly1305::new(send_key.into()),
            receive_cipher: ChaCha20Poly1305::new(receive_key.into()),
            send_counter: 0,
            receive_counter: 0,
        }
    }

    pub async fn send(&mut self, message: &Message) -> Result<()> {
        let plaintext = serde_json::to_vec(message)?;
        if plaintext.len() > MAX_FRAME_SIZE {
            bail!("消息超过最大限制 {} 字节", MAX_FRAME_SIZE);
        }
        let nonce = nonce_from_counter(self.send_counter);
        self.send_counter = self.send_counter.checked_add(1).context("发送计数器溢出")?;
        let ciphertext = self
            .send_cipher
            .encrypt(&nonce, plaintext.as_ref())
            .map_err(|_| anyhow::anyhow!("消息加密失败"))?;
        self.stream.write_u32(ciphertext.len() as u32).await?;
        self.stream.write_all(&ciphertext).await?;
        self.stream.flush().await?;
        Ok(())
    }

    pub async fn receive(&mut self) -> Result<Message> {
        let length = self.stream.read_u32().await? as usize;
        if length > MAX_FRAME_SIZE + 32 {
            bail!("收到的消息超过最大限制");
        }
        let mut ciphertext = vec![0_u8; length];
        self.stream.read_exact(&mut ciphertext).await?;
        let nonce = nonce_from_counter(self.receive_counter);
        self.receive_counter = self
            .receive_counter
            .checked_add(1)
            .context("接收计数器溢出")?;
        let plaintext = self
            .receive_cipher
            .decrypt(&nonce, ciphertext.as_ref())
            .map_err(|_| anyhow::anyhow!("消息认证失败"))?;
        Ok(serde_json::from_slice(&plaintext)?)
    }
}

pub fn derive_transport_key(shared_secret: &[u8], context: &[u8]) -> Result<[u8; 32]> {
    let hkdf = hkdf::Hkdf::<Sha256>::new(Some(b"clipboard-share-v1"), shared_secret);
    let mut key = [0_u8; 32];
    hkdf.expand(context, &mut key)
        .map_err(|_| anyhow::anyhow!("无法派生传输密钥"))?;
    Ok(key)
}

pub fn proof(key: &[u8; 32], device_id: Uuid, nonce: &[u8]) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(device_id.as_bytes());
    mac.update(nonce);
    hex::encode(mac.finalize().into_bytes())
}

pub fn verify_proof(key: &[u8; 32], device_id: Uuid, nonce: &[u8], value: &str) -> bool {
    use hmac::{Hmac, KeyInit, Mac};
    let Ok(decoded) = hex::decode(value) else {
        return false;
    };
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(device_id.as_bytes());
    mac.update(nonce);
    mac.verify_slice(&decoded).is_ok()
}

pub fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        bail!("文件路径必须是非空相对路径");
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            bail!("文件路径包含不安全组件: {}", path.display());
        }
    }
    Ok(())
}

fn nonce_from_counter(counter: u64) -> Nonce {
    let mut bytes = [0_u8; 12];
    bytes[4..].copy_from_slice(&counter.to_be_bytes());
    Nonce::from(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_relative_paths() {
        assert!(validate_relative_path(Path::new(r"..\secret.txt")).is_err());
        assert!(validate_relative_path(Path::new(r"C:\secret.txt")).is_err());
        assert!(validate_relative_path(Path::new(r"safe\file.txt")).is_ok());
    }

    #[test]
    fn clipboard_hash_detects_changes() {
        let item = ClipboardItem::new(
            Uuid::new_v4(),
            1,
            vec![ClipboardFormat::from_bytes("text/plain", b"hello")],
            vec![],
        )
        .unwrap();
        item.verify_hash().unwrap();
        let mut changed = item;
        changed.formats[0] = ClipboardFormat::from_bytes("text/plain", b"changed");
        assert!(changed.verify_hash().is_err());
    }

    #[test]
    fn proof_is_bound_to_device() {
        let key = [7_u8; 32];
        let id = Uuid::new_v4();
        let nonce = [3_u8; 32];
        let value = proof(&key, id, &nonce);
        assert!(verify_proof(&key, id, &nonce, &value));
        assert!(!verify_proof(&key, Uuid::new_v4(), &nonce, &value));
    }
}
