use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use rand::RngExt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, RwLock, broadcast},
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    clipboard::{self, ClipboardCapture},
    config::{AppConfig, DEFAULT_PORT, cache_dir},
    protocol::{
        ClipboardItem, FILE_CHUNK_SIZE, MAX_FRAME_SIZE, Message, PROTOCOL_VERSION, SecureChannel,
        derive_transport_key, proof, validate_relative_path, verify_proof,
    },
};

const PAIRING_PORT_OFFSET: u16 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct PairIntro {
    device_id: Uuid,
    device_name: String,
    spake_message: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PairReply {
    device_id: Uuid,
    device_name: String,
    spake_message: String,
    confirmation: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PairFinish {
    confirmation: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ConnectionHello {
    version: u16,
    device_id: Uuid,
    nonce: String,
    proof: String,
}

#[derive(Clone)]
pub struct NetworkState {
    pub config: Arc<RwLock<AppConfig>>,
    pub outbound: broadcast::Sender<Arc<ClipboardCapture>>,
    pub suppressed_hashes: Arc<Mutex<HashSet<String>>>,
    /// 应用远端剪贴板后的短暂静默，避免回环重发。
    pub suppress_capture_until: Arc<Mutex<tokio::time::Instant>>,
}

impl NetworkState {
    pub fn new(config: AppConfig) -> Self {
        let (outbound, _) = broadcast::channel(128);
        Self {
            config: Arc::new(RwLock::new(config)),
            outbound,
            suppressed_hashes: Arc::new(Mutex::new(HashSet::new())),
            suppress_capture_until: Arc::new(Mutex::new(tokio::time::Instant::now())),
        }
    }
}

pub async fn pair_listen(config: &mut AppConfig, code: &str) -> Result<()> {
    let port = config
        .listen_port
        .checked_add(PAIRING_PORT_OFFSET)
        .context("配对端口溢出")?;
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    println!("配对码：{code}");
    println!("请在另一台电脑执行：clipboard_share pair <本机IP> {code}");
    println!("等待连接，端口 {port}（配对码仅本次有效）...");

    // 短超时循环，便于后台 stop 取消自动配对。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    let (mut stream, address) = loop {
        if tokio::time::Instant::now() >= deadline {
            bail!("等待配对超时");
        }
        match tokio::time::timeout(Duration::from_secs(1), listener.accept()).await {
            Ok(Ok(connection)) => break connection,
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => continue,
        }
    };
    let intro: PairIntro = read_plain(&mut stream).await?;
    let client_id = intro.device_id.to_string();
    let (state, outbound) = Spake2::<Ed25519Group>::start_b(
        &Password::new(code.as_bytes()),
        &Identity::new(client_id.as_bytes()),
        &Identity::new(b"clipboard-share-pairing-server"),
    );
    let peer_message = BASE64
        .decode(intro.spake_message)
        .context("对端 SPAKE2 消息损坏")?;
    let shared = state
        .finish(&peer_message)
        .map_err(|_| anyhow::anyhow!("SPAKE2 配对失败"))?;
    let key = derive_transport_key(&shared, b"paired-device-key")?;
    let confirmation = proof(&key, config.device_id, b"pair-server");
    write_plain(
        &mut stream,
        &PairReply {
            device_id: config.device_id,
            device_name: config.device_name.clone(),
            spake_message: BASE64.encode(outbound),
            confirmation,
        },
    )
    .await?;
    let finish: PairFinish = read_plain(&mut stream).await?;
    if !verify_proof(&key, intro.device_id, b"pair-client", &finish.confirmation) {
        bail!("配对确认失败，配对码可能不一致");
    }

    config.upsert_peer(
        intro.device_id,
        intro.device_name,
        format!("{}:{}", address.ip(), config.listen_port),
        &key,
    )?;
    println!("配对成功：{}", address.ip());
    Ok(())
}

pub async fn pair_connect(config: &mut AppConfig, address: &str, code: &str) -> Result<()> {
    let host = if address.contains(':') {
        address.to_owned()
    } else {
        format!("{}:{}", address, config.listen_port + PAIRING_PORT_OFFSET)
    };
    let mut stream = TcpStream::connect(&host)
        .await
        .with_context(|| format!("无法连接配对地址 {host}"))?;
    let client_id = config.device_id.to_string();
    // 服务端 UUID 在收到回复前未知，服务身份固定为协议名；服务端使用同一值。
    let service_id = b"clipboard-share-pairing-server";
    let (state, outbound) = Spake2::<Ed25519Group>::start_a(
        &Password::new(code.as_bytes()),
        &Identity::new(client_id.as_bytes()),
        &Identity::new(service_id),
    );
    write_plain(
        &mut stream,
        &PairIntro {
            device_id: config.device_id,
            device_name: config.device_name.clone(),
            spake_message: BASE64.encode(outbound),
        },
    )
    .await?;
    let reply: PairReply = read_plain(&mut stream).await?;

    // 重新以服务端真实身份启动会产生不同随机消息，不能继续；协议约定服务端固定身份。
    let peer_message = BASE64
        .decode(reply.spake_message)
        .context("服务端 SPAKE2 消息损坏")?;
    let shared = state
        .finish(&peer_message)
        .map_err(|_| anyhow::anyhow!("SPAKE2 配对失败，配对码可能错误"))?;
    let key = derive_transport_key(&shared, b"paired-device-key")?;
    if !verify_proof(&key, reply.device_id, b"pair-server", &reply.confirmation) {
        bail!("服务端配对确认失败");
    }
    write_plain(
        &mut stream,
        &PairFinish {
            confirmation: proof(&key, config.device_id, b"pair-client"),
        },
    )
    .await?;

    let normal_address = if address.contains(':') {
        let host_only = address
            .rsplit_once(':')
            .map(|(host, _)| host)
            .unwrap_or(address);
        format!("{}:{}", host_only, config.listen_port)
    } else {
        format!("{}:{}", address, config.listen_port)
    };
    config.upsert_peer(
        reply.device_id,
        reply.device_name.clone(),
        normal_address,
        &key,
    )?;
    println!("配对成功：{} ({})", reply.device_name, reply.device_id);
    Ok(())
}

pub async fn run(state: NetworkState) -> Result<()> {
    let cache = cache_dir()?;
    fs::create_dir_all(&cache).await?;
    let config = state.config.read().await;
    let listen_port = config.listen_port;
    let cache_bytes = config.cache_bytes;
    drop(config);
    tokio::task::spawn_blocking(move || prune_cache(&cache, cache_bytes)).await??;
    let listener = TcpListener::bind(("0.0.0.0", listen_port)).await?;
    info!(port = listen_port, "剪贴板同步服务已监听");

    let peers = state.config.read().await.peers.clone();
    let local_id = state.config.read().await.device_id;
    for peer in peers {
        // 每对设备只由 UUID 较小的一端主动连接，防止产生双连接。
        if local_id.as_bytes() < peer.device_id.as_bytes() {
            let state = state.clone();
            tokio::task::spawn_local(async move {
                connector_loop(state, peer.device_id, peer.address).await;
            });
        }
    }

    loop {
        let (stream, address) = listener.accept().await?;
        let state = state.clone();
        tokio::task::spawn_local(async move {
            if let Err(error) = accept_connection(state, stream).await {
                warn!(%address, %error, "入站连接结束");
            }
        });
    }
}

async fn connector_loop(state: NetworkState, peer_id: Uuid, address: String) {
    let mut delay = Duration::from_secs(1);
    loop {
        match TcpStream::connect(&address).await {
            Ok(stream) => {
                info!(%peer_id, %address, "已连接配对设备");
                if let Err(error) = connect_connection(state.clone(), peer_id, stream).await {
                    warn!(%peer_id, %error, "设备连接断开");
                }
                delay = Duration::from_secs(1);
            }
            Err(error) => warn!(%peer_id, %error, "暂时无法连接设备"),
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

async fn connect_connection(
    state: NetworkState,
    peer_id: Uuid,
    mut stream: TcpStream,
) -> Result<()> {
    let config = state.config.read().await;
    let local_id = config.device_id;
    let key = config.peer_key(peer_id)?;
    drop(config);

    let client_nonce = random_nonce();
    write_plain(
        &mut stream,
        &ConnectionHello {
            version: PROTOCOL_VERSION,
            device_id: local_id,
            nonce: BASE64.encode(client_nonce),
            proof: proof(&key, local_id, &client_nonce),
        },
    )
    .await?;
    let reply: ConnectionHello = read_plain(&mut stream).await?;
    if reply.version != PROTOCOL_VERSION || reply.device_id != peer_id {
        bail!("对端协议版本或设备身份不匹配");
    }
    let server_nonce = BASE64.decode(reply.nonce)?;
    if !verify_proof(&key, peer_id, &server_nonce, &reply.proof) {
        bail!("对端身份校验失败");
    }
    let (send, receive) = session_keys(&key, local_id, peer_id, &client_nonce, &server_nonce)?;
    run_channel(state, SecureChannel::new(stream, &send, &receive)).await
}

async fn accept_connection(state: NetworkState, mut stream: TcpStream) -> Result<()> {
    let hello: ConnectionHello = read_plain(&mut stream).await?;
    if hello.version != PROTOCOL_VERSION {
        bail!("不支持的协议版本 {}", hello.version);
    }
    let config = state.config.read().await;
    let local_id = config.device_id;
    let key = config.peer_key(hello.device_id)?;
    drop(config);
    let client_nonce = BASE64.decode(hello.nonce)?;
    if !verify_proof(&key, hello.device_id, &client_nonce, &hello.proof) {
        bail!("入站设备身份校验失败");
    }
    let server_nonce = random_nonce();
    write_plain(
        &mut stream,
        &ConnectionHello {
            version: PROTOCOL_VERSION,
            device_id: local_id,
            nonce: BASE64.encode(server_nonce),
            proof: proof(&key, local_id, &server_nonce),
        },
    )
    .await?;
    let (client_send, server_send) = session_keys(
        &key,
        hello.device_id,
        local_id,
        &client_nonce,
        &server_nonce,
    )?;
    run_channel(
        state,
        SecureChannel::new(stream, &server_send, &client_send),
    )
    .await
}

async fn run_channel(state: NetworkState, mut channel: SecureChannel) -> Result<()> {
    let mut receiver = state.outbound.subscribe();
    let mut incoming: HashMap<Uuid, IncomingItem> = HashMap::new();

    loop {
        tokio::select! {
            outbound = receiver.recv() => {
                match outbound {
                    Ok(capture) => {
                        if let Err(error) = send_capture(&mut channel, &capture).await {
                            warn!(%error, "发送剪贴板失败，跳过本项并保持连接");
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(count)) => {
                        warn!(count, "发送队列拥塞，跳过旧项目");
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
            inbound = channel.receive() => {
                match inbound {
                    Ok(message) => {
                        if let Err(error) =
                            handle_message(&state, &mut channel, &mut incoming, message).await
                        {
                            warn!(%error, "处理远端消息失败，保持连接");
                        }
                    }
                    Err(error) => {
                        // 超大帧等协议错误仍需断开，由上层重连。
                        return Err(error);
                    }
                }
            }
        }
    }
}

async fn send_capture(channel: &mut SecureChannel, capture: &ClipboardCapture) -> Result<()> {
    let estimated = estimate_item_wire_size(&capture.item);
    if estimated > crate::protocol::MAX_FRAME_SIZE {
        bail!(
            "剪贴板项过大（约 {} 字节），已跳过",
            estimated
        );
    }
    channel
        .send(&Message::Clipboard(capture.item.clone()))
        .await?;
    for source in &capture.source_files {
        validate_relative_path(&source.relative_path)?;
        let mut file = fs::File::open(&source.source).await?;
        let mut offset = 0_u64;
        let mut buffer = vec![0_u8; FILE_CHUNK_SIZE];
        loop {
            let count = file.read(&mut buffer).await?;
            let eof = count == 0;
            channel
                .send(&Message::FileChunk {
                    item_id: capture.item.id,
                    relative_path: source.relative_path.clone(),
                    offset,
                    data_base64: BASE64.encode(&buffer[..count]),
                    eof,
                })
                .await?;
            if eof {
                break;
            }
            offset += count as u64;
        }
    }
    Ok(())
}

async fn handle_message(
    state: &NetworkState,
    channel: &mut SecureChannel,
    incoming: &mut HashMap<Uuid, IncomingItem>,
    message: Message,
) -> Result<()> {
    match message {
        Message::Clipboard(item) => {
            item.verify_hash()?;
            let max_bytes = state.config.read().await.max_item_bytes;
            let total = item
                .files
                .iter()
                .try_fold(0_u64, |sum, file| sum.checked_add(file.size))
                .context("文件总大小溢出")?;
            if total > max_bytes {
                bail!("收到的文件总大小 {total} 超过限制 {max_bytes}");
            }
            if item.files.is_empty() {
                if let Err(error) = apply_received(state, &item).await {
                    warn!(%error, item_id = %item.id, "应用远端剪贴板失败，保持连接继续同步");
                }
                channel.send(&Message::Ack { item_id: item.id }).await?;
            } else {
                let pending = IncomingItem::create(item).await?;
                if pending.item.files.iter().all(|entry| entry.is_directory) {
                    if let Err(error) = apply_received(state, &pending.item).await {
                        warn!(%error, item_id = %pending.item.id, "应用远端目录剪贴板失败");
                    }
                    channel
                        .send(&Message::Ack {
                            item_id: pending.item.id,
                        })
                        .await?;
                } else {
                    incoming.insert(pending.item.id, pending);
                }
            }
        }
        Message::FileChunk {
            item_id,
            relative_path,
            offset,
            data_base64,
            eof,
        } => {
            let pending = incoming
                .get_mut(&item_id)
                .context("收到未知剪贴板项目的文件块")?;
            if pending
                .write_chunk(&relative_path, offset, &BASE64.decode(data_base64)?, eof)
                .await?
            {
                let pending = incoming.remove(&item_id).expect("项目应仍然存在");
                pending.verify().await?;
                if let Err(error) = apply_received(state, &pending.item).await {
                    warn!(%error, %item_id, "应用远端文件剪贴板失败，保持连接继续同步");
                }
                channel.send(&Message::Ack { item_id }).await?;
            }
        }
        Message::Ping => channel.send(&Message::Pong).await?,
        Message::Pong | Message::Ack { .. } => {}
        Message::Error { message } => bail!("对端错误：{message}"),
        Message::Hello { .. } => bail!("连接建立后不应再次收到 Hello"),
    }
    Ok(())
}

async fn apply_received(state: &NetworkState, item: &ClipboardItem) -> Result<()> {
    // 先抑制，避免写入本机剪贴板后被 watch 立刻回传。
    state
        .suppressed_hashes
        .lock()
        .await
        .insert(item.content_hash.clone());
    *state.suppress_capture_until.lock().await =
        tokio::time::Instant::now() + Duration::from_millis(1500);
    clipboard::apply(item, &cache_dir()?).await?;
    Ok(())
}

fn estimate_item_wire_size(item: &ClipboardItem) -> usize {
    let formats = item
        .formats
        .iter()
        .map(|format| format.data_base64.len() + format.name.len() + 64)
        .sum::<usize>();
    let files = item
        .files
        .iter()
        .map(|file| file.relative_path.as_os_str().len() + file.sha256.len() + 64)
        .sum::<usize>();
    formats + files + 512
}

struct IncomingItem {
    item: ClipboardItem,
    root: PathBuf,
    completed: HashSet<PathBuf>,
}

impl IncomingItem {
    async fn create(item: ClipboardItem) -> Result<Self> {
        let root = cache_dir()?.join(item.id.to_string());
        if root.exists() {
            fs::remove_dir_all(&root).await?;
        }
        fs::create_dir_all(&root).await?;
        for entry in &item.files {
            entry.validate_path()?;
            if entry.is_directory {
                fs::create_dir_all(root.join(&entry.relative_path)).await?;
            } else if let Some(parent) = root.join(&entry.relative_path).parent() {
                fs::create_dir_all(parent).await?;
            }
        }
        Ok(Self {
            item,
            root,
            completed: HashSet::new(),
        })
    }

    async fn write_chunk(
        &mut self,
        relative_path: &Path,
        offset: u64,
        data: &[u8],
        eof: bool,
    ) -> Result<bool> {
        validate_relative_path(relative_path)?;
        let expected = self
            .item
            .files
            .iter()
            .find(|entry| !entry.is_directory && entry.relative_path == relative_path)
            .context("文件块不在项目清单中")?;
        if offset + data.len() as u64 > expected.size {
            bail!("文件块超过清单声明大小");
        }
        let path = self.root.join(relative_path);
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        file.write_all(data).await?;
        file.flush().await?;
        if eof {
            if offset != expected.size {
                bail!("文件结束位置与清单大小不一致");
            }
            self.completed.insert(relative_path.to_owned());
        }
        let file_count = self
            .item
            .files
            .iter()
            .filter(|entry| !entry.is_directory)
            .count();
        Ok(self.completed.len() == file_count)
    }

    async fn verify(&self) -> Result<()> {
        for entry in self.item.files.iter().filter(|entry| !entry.is_directory) {
            let path = self.root.join(&entry.relative_path);
            let expected = entry.sha256.clone();
            let actual = tokio::task::spawn_blocking(move || hash_file(&path)).await??;
            if actual != expected {
                bail!("文件 {} 哈希校验失败", entry.relative_path.display());
            }
        }
        Ok(())
    }
}

fn hash_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn prune_cache(root: &Path, quota: u64) -> Result<()> {
    let mut entries = Vec::new();
    let mut total = 0_u64;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let mut size = 0_u64;
        for child in walkdir::WalkDir::new(entry.path()).follow_links(false) {
            let child = child?;
            if child.file_type().is_file() {
                size = size.saturating_add(child.metadata()?.len());
            }
        }
        let modified = entry
            .metadata()?
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        total = total.saturating_add(size);
        entries.push((modified, entry.path(), size));
    }
    entries.sort_by_key(|(modified, _, _)| *modified);
    for (_, path, size) in entries {
        if total <= quota {
            break;
        }
        std::fs::remove_dir_all(&path)?;
        total = total.saturating_sub(size);
    }
    Ok(())
}

fn random_nonce() -> [u8; 32] {
    let mut nonce = [0_u8; 32];
    rand::rng().fill(&mut nonce);
    nonce
}

fn session_keys(
    peer_key: &[u8; 32],
    client_id: Uuid,
    server_id: Uuid,
    client_nonce: &[u8],
    server_nonce: &[u8],
) -> Result<([u8; 32], [u8; 32])> {
    let mut context = Vec::new();
    context.extend_from_slice(client_id.as_bytes());
    context.extend_from_slice(server_id.as_bytes());
    context.extend_from_slice(client_nonce);
    context.extend_from_slice(server_nonce);
    let mut c2s_context = context.clone();
    c2s_context.extend_from_slice(b"client-to-server");
    let mut s2c_context = context;
    s2c_context.extend_from_slice(b"server-to-client");
    Ok((
        derive_transport_key(peer_key, &c2s_context)?,
        derive_transport_key(peer_key, &s2c_context)?,
    ))
}

async fn write_plain<T: Serialize>(stream: &mut TcpStream, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME_SIZE {
        bail!("握手消息过大");
    }
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_plain<T: DeserializeOwned>(stream: &mut TcpStream) -> Result<T> {
    let length = stream.read_u32().await? as usize;
    if length > MAX_FRAME_SIZE {
        bail!("握手消息过大");
    }
    let mut bytes = vec![0_u8; length];
    stream.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn default_pairing_port() -> u16 {
    DEFAULT_PORT + PAIRING_PORT_OFFSET
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_directions_have_different_keys() {
        let (c2s, s2c) = session_keys(
            &[1_u8; 32],
            Uuid::new_v4(),
            Uuid::new_v4(),
            &[2_u8; 32],
            &[3_u8; 32],
        )
        .unwrap();
        assert_ne!(c2s, s2c);
    }
}
