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
use tokio::net::{
    TcpStream,
    tcp::{OwnedReadHalf, OwnedWriteHalf},
};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_ITEM_BYTES: u64 = 50 * 1024 * 1024;
// Base64 payload plus bounded metadata; the logical item limit is checked separately.
pub const MAX_FRAME_SIZE: usize = (MAX_ITEM_BYTES as usize).div_ceil(3) * 4 + 1024 * 1024;
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

    /// Sum decoded clipboard formats and all declared files, without large decode allocations.
    pub fn payload_bytes(&self) -> Result<u64> {
        let mut total = self.files.iter().try_fold(0_u64, |sum, file| {
            sum.checked_add(file.size).context("剪贴板总大小溢出")
        })?;
        for format in &self.formats {
            let mut decoded = [0_u8; 3072];
            for chunk in format.data_base64.as_bytes().chunks(4096) {
                let size = BASE64
                    .decode_slice(chunk, &mut decoded)
                    .context("剪贴板格式 Base64 无效")?;
                total = total.checked_add(size as u64).context("剪贴板总大小溢出")?;
            }
        }
        Ok(total)
    }

    pub fn validate_size(&self, configured_limit: u64, direction: &str) -> Result<u64> {
        let limit = configured_limit.min(MAX_ITEM_BYTES);
        let actual_bytes = self.payload_bytes()?;
        if actual_bytes > limit {
            tracing::warn!(item_id = %self.id, direction, actual_bytes, limit_bytes = limit,
                "剪贴板项目超过大小限制，已拒绝");
            bail!("剪贴板项目 {actual_bytes} 字节超过限制 {limit} 字节");
        }
        Ok(actual_bytes)
    }

    pub fn calculate_hash(&self) -> Result<String> {
        #[derive(Serialize)]
        struct HashView<'a> {
            id: Uuid,
            origin: Uuid,
            created_unix_ms: u64,
            formats: &'a [ClipboardFormat],
            files: &'a [FileEntry],
            content_hash: &'a str,
        }
        let normalized = HashView {
            id: Uuid::nil(),
            origin: Uuid::nil(),
            created_unix_ms: 0,
            formats: &self.formats,
            files: &self.files,
            content_hash: "",
        };
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
    sender: SecureSender,
    receiver: SecureReceiver,
}

pub struct SecureSender {
    stream: OwnedWriteHalf,
    send_cipher: ChaCha20Poly1305,
    send_counter: u64,
}

pub struct SecureReceiver {
    stream: OwnedReadHalf,
    receive_cipher: ChaCha20Poly1305,
    receive_counter: u64,
}

impl SecureChannel {
    pub fn new(stream: TcpStream, send_key: &[u8; 32], receive_key: &[u8; 32]) -> Self {
        let (reader, writer) = stream.into_split();
        Self {
            sender: SecureSender {
                stream: writer,
                send_cipher: ChaCha20Poly1305::new(send_key.into()),
                send_counter: 0,
            },
            receiver: SecureReceiver {
                stream: reader,
                receive_cipher: ChaCha20Poly1305::new(receive_key.into()),
                receive_counter: 0,
            },
        }
    }

    pub async fn send(&mut self, message: &Message) -> Result<()> {
        self.sender.send(message).await
    }

    pub async fn receive(&mut self) -> Result<Message> {
        self.receiver.receive().await
    }

    pub fn into_split(self) -> (SecureSender, SecureReceiver) {
        (self.sender, self.receiver)
    }
}

impl SecureSender {
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
}

impl SecureReceiver {
    // This future must run to completion; cancelling it requires closing the connection.
    pub async fn receive(&mut self) -> Result<Message> {
        let length = self.stream.read_u32().await? as usize;
        if !(16..=MAX_FRAME_SIZE + 16).contains(&length) {
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
    // Enforce Windows separators even in cross-platform protocol tests.
    let text = path.to_str().context("文件路径不是有效 Unicode")?;
    if text
        .split(['/', '\\'])
        .any(|part| part.is_empty() || part == "." || part == ".." || part.contains(':'))
    {
        bail!("文件路径包含不安全组件: {}", path.display());
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
    fn item_limit_counts_all_formats_and_files_and_cannot_be_raised() {
        let mut item = ClipboardItem::new(
            Uuid::new_v4(),
            1,
            vec![
                ClipboardFormat::from_bytes("text/plain", b"abc"),
                ClipboardFormat::from_bytes("custom", b"de"),
            ],
            vec![FileEntry {
                relative_path: "file".into(),
                size: MAX_ITEM_BYTES - 5,
                sha256: String::new(),
                is_directory: false,
            }],
        )
        .unwrap();
        assert_eq!(
            item.validate_size(u64::MAX, "test").unwrap(),
            MAX_ITEM_BYTES
        );
        assert!(item.validate_size(MAX_ITEM_BYTES - 1, "test").is_err());
        item.files[0].size += 1;
        assert!(item.validate_size(u64::MAX, "test").is_err());
        item.files[0].size = u64::MAX;
        assert!(item.payload_bytes().is_err());
    }

    #[test]
    fn rejected_item_logs_size_limit_direction_and_id() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        #[derive(Clone)]
        struct LogWriter(Arc<Mutex<Vec<u8>>>);
        impl Write for LogWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let output = Arc::new(Mutex::new(Vec::new()));
        let writer = LogWriter(output.clone());
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let item = ClipboardItem::new(
            Uuid::new_v4(),
            1,
            vec![],
            vec![FileEntry {
                relative_path: "oversized.bin".into(),
                size: MAX_ITEM_BYTES + 1,
                sha256: String::new(),
                is_directory: false,
            }],
        )
        .unwrap();
        tracing::subscriber::with_default(subscriber, || {
            assert!(item.validate_size(u64::MAX, "receive").is_err());
        });
        let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        for field in [
            "WARN",
            "actual_bytes=52428801",
            "limit_bytes=52428800",
            "receive",
            "已拒绝",
            &item.id.to_string(),
        ] {
            assert!(log.contains(field), "missing {field} in {log}");
        }
    }

    #[test]
    fn decoded_sizes_include_padding_and_multiple_decode_chunks() {
        for size in [0, 1, 2, 3, 3071, 3072, 3073, 8192] {
            let item = ClipboardItem::new(
                Uuid::new_v4(),
                1,
                vec![ClipboardFormat::from_bytes("test", &vec![0; size])],
                vec![],
            )
            .unwrap();
            assert_eq!(item.payload_bytes().unwrap(), size as u64);
        }
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
