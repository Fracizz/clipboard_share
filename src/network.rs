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
    sync::{Mutex, RwLock, Semaphore, broadcast, mpsc},
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    clipboard::{self, ClipboardCapture},
    config::{AppConfig, DEFAULT_PORT, cache_dir},
    protocol::{
        ClipboardItem, FILE_CHUNK_SIZE, Message, PROTOCOL_VERSION, SecureChannel, SecureReceiver,
        SecureSender, derive_transport_key, proof, validate_relative_path, verify_proof,
    },
};

const MAX_HANDSHAKE_SIZE: usize = 16 * 1024;
const MAX_INBOUND_CONNECTIONS: usize = 16;
const MAX_PENDING_ITEMS: usize = 4;

const PAIRING_PORT_OFFSET: u16 = 1;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(90);

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
        let (outbound, _) = broadcast::channel(2);
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

    let permits = Arc::new(Semaphore::new(MAX_INBOUND_CONNECTIONS));
    loop {
        let (stream, address) = listener.accept().await?;
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            warn!(%address, "入站连接数量已达上限，拒绝连接");
            continue;
        };
        let state = state.clone();
        tokio::task::spawn_local(async move {
            let _permit = permit;
            if let Err(error) = accept_connection(state, stream).await {
                warn!(%address, %error, "入站连接结束");
            }
        });
    }
}

async fn connector_loop(state: NetworkState, peer_id: Uuid, address: String) {
    let mut delay = Duration::from_secs(1);
    loop {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&address)).await {
            Ok(Ok(stream)) => {
                info!(%peer_id, %address, "已连接配对设备");
                if let Err(error) = connect_connection(state.clone(), peer_id, stream).await {
                    warn!(%peer_id, %error, "设备连接断开");
                }
                delay = Duration::from_secs(1);
            }
            Ok(Err(error)) => warn!(%peer_id, %error, "暂时无法连接设备"),
            Err(_) => warn!(%peer_id, %address, "连接设备超时，稍后重试"),
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

async fn run_channel(state: NetworkState, channel: SecureChannel) -> Result<()> {
    let receiver = state.outbound.subscribe();
    let (writer, reader) = channel.into_split();
    // Bound large payload buffering, but never block reads while queuing small replies.
    let (data_sender, data_receiver) = mpsc::channel(1);
    let (reply_sender, reply_receiver) = mpsc::channel(32);
    // Each direction owns its I/O future. A local capture cannot cancel a partial read,
    // and simultaneous large transfers keep draining both sockets.
    tokio::try_join!(
        queue_captures(&state, receiver, data_sender),
        write_messages(writer, data_receiver, reply_receiver),
        read_messages(&state, reader, reply_sender),
    )?;
    Ok(())
}

async fn queue_captures(
    state: &NetworkState,
    mut receiver: broadcast::Receiver<Arc<ClipboardCapture>>,
    sender: mpsc::Sender<Message>,
) -> Result<()> {
    loop {
        match receiver.recv().await {
            Ok(capture) => {
                let limit = state.config.read().await.max_item_bytes;
                if let Err(error) = capture.item.validate_size(limit, "send") {
                    warn!(%error, "拒绝发送无效剪贴板项目");
                    continue;
                }
                // A source/read failure may leave a partial manifest on the peer.
                // Close this session so its pending transfer is discarded.
                send_capture(&sender, &capture, limit).await?;
            }
            Err(broadcast::error::RecvError::Lagged(count)) => {
                warn!(count, "发送队列拥塞，跳过旧项目");
            }
            Err(broadcast::error::RecvError::Closed) => return Ok(()),
        }
    }
}

async fn write_messages(
    mut writer: SecureSender,
    mut data: mpsc::Receiver<Message>,
    mut replies: mpsc::Receiver<Message>,
) -> Result<()> {
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let message = tokio::select! {
            reply = replies.recv() => reply.context("回复队列已关闭")?,
            message = data.recv() => message.context("发送队列已关闭")?,
            _ = heartbeat.tick() => Message::Ping,
        };
        // A failed/partial write invalidates framing and nonce state: close and reconnect.
        tokio::time::timeout(IO_TIMEOUT, writer.send(&message))
            .await
            .context("发送消息超时，关闭连接以便重连")??;
    }
}

async fn read_messages(
    state: &NetworkState,
    mut reader: SecureReceiver,
    replies: mpsc::Sender<Message>,
) -> Result<()> {
    let mut incoming: HashMap<Uuid, IncomingItem> = HashMap::new();
    loop {
        let message = tokio::time::timeout(RECEIVE_TIMEOUT, reader.receive())
            .await
            .context("接收消息超时，对端可能已离线，关闭连接以便重连")??;
        incoming.retain(|_, pending| pending.last_activity.elapsed() < RECEIVE_TIMEOUT);
        match handle_message(state, &mut incoming, message).await {
            Ok(Some(reply)) => replies.try_send(reply).context("回复队列已关闭")?,
            Ok(None) => {}
            Err(error) => warn!(%error, "处理远端消息失败，保持连接"),
        }
    }
}

async fn send_capture(
    sender: &mpsc::Sender<Message>,
    capture: &ClipboardCapture,
    limit: u64,
) -> Result<()> {
    capture.item.validate_size(limit, "send")?;
    // Check all sources before publishing a manifest; then cap reads to declared sizes.
    for source in &capture.source_files {
        let declared = capture
            .item
            .files
            .iter()
            .find(|entry| !entry.is_directory && entry.relative_path == source.relative_path)
            .context("源文件不在项目清单中")?;
        let actual_bytes = fs::metadata(&source.source).await?.len();
        if actual_bytes != declared.size {
            warn!(item_id = %capture.item.id, direction = "send", actual_bytes,
                limit_bytes = limit.min(crate::protocol::MAX_ITEM_BYTES), "源文件大小已变化，拒绝发送");
            bail!("源文件大小已变化");
        }
    }
    let estimated = estimate_item_wire_size(&capture.item);
    if estimated > crate::protocol::MAX_FRAME_SIZE {
        bail!("剪贴板项过大（约 {} 字节），已跳过", estimated);
    }
    sender
        .send(Message::Clipboard(capture.item.clone()))
        .await?;
    for source in &capture.source_files {
        validate_relative_path(&source.relative_path)?;
        let mut file = fs::File::open(&source.source).await?;
        let declared_size = capture
            .item
            .files
            .iter()
            .find(|entry| entry.relative_path == source.relative_path)
            .context("源文件不在项目清单中")?
            .size;
        let mut offset = 0_u64;
        let mut buffer = vec![0_u8; FILE_CHUNK_SIZE];
        loop {
            let count = file.read(&mut buffer).await?;
            let eof = count == 0;
            if offset + count as u64 > declared_size || (eof && offset != declared_size) {
                bail!("发送期间源文件大小发生变化");
            }
            sender
                .send(Message::FileChunk {
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
    incoming: &mut HashMap<Uuid, IncomingItem>,
    message: Message,
) -> Result<Option<Message>> {
    match message {
        Message::Clipboard(item) => {
            let max_bytes = state.config.read().await.max_item_bytes;
            item.validate_size(max_bytes, "receive")?;
            item.verify_hash()?;
            if item.files.is_empty() {
                if let Err(error) = apply_received(state, &item).await {
                    warn!(%error, item_id = %item.id, "应用远端剪贴板失败，保持连接继续同步");
                }
                return Ok(Some(Message::Ack { item_id: item.id }));
            } else {
                if incoming.len() >= MAX_PENDING_ITEMS || incoming.contains_key(&item.id) {
                    bail!("未完成接收项目数量已达上限或项目重复");
                }
                let mut pending = IncomingItem::create(item).await?;
                if pending.item.files.iter().all(|entry| entry.is_directory) {
                    pending.keep_cache = true;
                    if let Err(error) = apply_received(state, &pending.item).await {
                        warn!(%error, item_id = %pending.item.id, "应用远端目录剪贴板失败");
                    }
                    return Ok(Some(Message::Ack {
                        item_id: pending.item.id,
                    }));
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
            if data_base64.len() > FILE_CHUNK_SIZE.div_ceil(3) * 4 {
                bail!("文件块超过大小限制");
            }
            let data = BASE64.decode(data_base64)?;
            let result = incoming
                .get_mut(&item_id)
                .context("收到未知剪贴板项目的文件块")?
                .write_chunk(&relative_path, offset, &data, eof)
                .await;
            let completed = match result {
                Ok(completed) => completed,
                Err(error) => {
                    incoming.remove(&item_id);
                    return Err(error);
                }
            };
            if completed {
                let mut pending = incoming.remove(&item_id).expect("项目应仍然存在");
                pending.verify().await?;
                pending.keep_cache = true;
                if let Err(error) = apply_received(state, &pending.item).await {
                    warn!(%error, %item_id, "应用远端文件剪贴板失败，保持连接继续同步");
                }
                return Ok(Some(Message::Ack { item_id }));
            }
        }
        Message::Ping => return Ok(Some(Message::Pong)),
        Message::Pong | Message::Ack { .. } => {}
        Message::Error { message } => bail!("对端错误：{message}"),
        Message::Hello { .. } => bail!("连接建立后不应再次收到 Hello"),
    }
    Ok(None)
}

async fn apply_received(_state: &NetworkState, item: &ClipboardItem) -> Result<()> {
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
    received: HashMap<PathBuf, u64>,
    last_activity: tokio::time::Instant,
    keep_cache: bool,
}

impl IncomingItem {
    async fn create(item: ClipboardItem) -> Result<Self> {
        let root = cache_dir()?.join(item.id.to_string());
        // Never replace an active/history cache directory sharing a remote ID.
        for entry in &item.files {
            entry.validate_path()?;
        }
        let mut paths = HashSet::new();
        for entry in &item.files {
            if !paths.insert(entry.relative_path.clone()) {
                bail!("文件清单含重复路径");
            }
        }
        fs::create_dir(&root).await?;
        let pending = Self {
            item,
            root,
            completed: HashSet::new(),
            received: HashMap::new(),
            last_activity: tokio::time::Instant::now(),
            keep_cache: false,
        };
        for entry in &pending.item.files {
            if entry.is_directory {
                fs::create_dir_all(pending.root.join(&entry.relative_path)).await?;
            } else if let Some(parent) = pending.root.join(&entry.relative_path).parent() {
                fs::create_dir_all(parent).await?;
            }
        }
        Ok(pending)
    }

    async fn write_chunk(
        &mut self,
        relative_path: &Path,
        offset: u64,
        data: &[u8],
        eof: bool,
    ) -> Result<bool> {
        self.last_activity = tokio::time::Instant::now();
        validate_relative_path(relative_path)?;
        let expected = self
            .item
            .files
            .iter()
            .find(|entry| !entry.is_directory && entry.relative_path == relative_path)
            .context("文件块不在项目清单中")?;
        if self.completed.contains(relative_path)
            || offset != *self.received.get(relative_path).unwrap_or(&0)
        {
            bail!("文件块偏移不连续或文件已完成");
        }
        let end = offset
            .checked_add(data.len() as u64)
            .context("文件块偏移溢出")?;
        if end > expected.size {
            bail!("文件块超过清单声明大小");
        }
        if eof && (offset != expected.size || !data.is_empty()) {
            bail!("文件结束位置与清单大小不一致");
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
        self.received.insert(relative_path.to_owned(), end);
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

impl Drop for IncomingItem {
    fn drop(&mut self) {
        if !self.keep_cache {
            let root = self.root.clone();
            // Use the runtime (not a LocalSet), so connection cancellation also cleans up.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if let Err(error) = fs::remove_dir_all(&root).await {
                        warn!(%error, path = %root.display(), "清理未完成的接收缓存失败");
                    }
                });
            }
        }
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
    if bytes.len() > MAX_HANDSHAKE_SIZE {
        bail!("握手消息过大");
    }
    tokio::time::timeout(IO_TIMEOUT, async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;
        stream.flush().await
    })
    .await
    .context("发送握手消息超时")??;
    Ok(())
}

async fn read_plain<T: DeserializeOwned>(stream: &mut TcpStream) -> Result<T> {
    tokio::time::timeout(IO_TIMEOUT, async {
        let length = stream.read_u32().await? as usize;
        if length > MAX_HANDSHAKE_SIZE {
            bail!("握手消息过大");
        }
        let mut bytes = vec![0_u8; length];
        stream.read_exact(&mut bytes).await?;
        Ok(serde_json::from_slice(&bytes)?)
    })
    .await
    .context("接收握手消息超时")?
}

pub fn default_pairing_port() -> u16 {
    DEFAULT_PORT + PAIRING_PORT_OFFSET
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::protocol::ClipboardFormat;
    use chacha20poly1305::{
        ChaCha20Poly1305, Nonce,
        aead::{Aead, KeyInit},
    };

    const TEST_KEY: [u8; 32] = [7; 32];

    async fn socket_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (client, server) = tokio::join!(
            TcpStream::connect(listener.local_addr().unwrap()),
            listener.accept(),
        );
        (client.unwrap(), server.unwrap().0)
    }

    fn encrypted_frame(message: &Message, counter: u64) -> Vec<u8> {
        let mut nonce = [0; 12];
        nonce[4..].copy_from_slice(&counter.to_be_bytes());
        let encrypted = ChaCha20Poly1305::new((&TEST_KEY).into())
            .encrypt(
                &Nonce::from(nonce),
                serde_json::to_vec(message).unwrap().as_slice(),
            )
            .unwrap();
        let mut frame = (encrypted.len() as u32).to_be_bytes().to_vec();
        frame.extend(encrypted);
        frame
    }

    async fn read_test_message(stream: &mut TcpStream, counter: &mut u64) -> Message {
        let length = stream.read_u32().await.unwrap() as usize;
        let mut ciphertext = vec![0; length];
        stream.read_exact(&mut ciphertext).await.unwrap();
        let mut nonce = [0; 12];
        nonce[4..].copy_from_slice(&counter.to_be_bytes());
        *counter += 1;
        let plaintext = ChaCha20Poly1305::new((&TEST_KEY).into())
            .decrypt(&Nonce::from(nonce), ciphertext.as_slice())
            .unwrap();
        serde_json::from_slice(&plaintext).unwrap()
    }

    fn test_capture(size: usize) -> Arc<ClipboardCapture> {
        Arc::new(ClipboardCapture {
            item: ClipboardItem::new(
                Uuid::new_v4(),
                1,
                vec![ClipboardFormat::from_bytes("text/plain", &vec![b'x'; size])],
                vec![],
            )
            .unwrap(),
            source_files: vec![],
        })
    }

    #[tokio::test]
    async fn local_capture_does_not_cancel_partial_inbound_frame() {
        let (local, mut peer) = socket_pair().await;
        let state = NetworkState::new(AppConfig::default());
        let outbound = state.outbound.clone();
        let channel = SecureChannel::new(local, &TEST_KEY, &TEST_KEY);
        let scenario = async {
            let mut receive_counter = 0;
            // Interrupt both a partial length prefix and a partial encrypted body.
            for (counter, split) in [2, 9].into_iter().enumerate() {
                let frame = encrypted_frame(&Message::Ping, counter as u64);
                peer.write_all(&frame[..split]).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                let capture = test_capture(32);
                let expected = capture.item.id;
                outbound.send(capture).unwrap();
                loop {
                    match read_test_message(&mut peer, &mut receive_counter).await {
                        Message::Clipboard(item) => {
                            assert_eq!(item.id, expected);
                            break;
                        }
                        Message::Ping => {}
                        other => panic!("unexpected message: {other:?}"),
                    }
                }
                peer.write_all(&frame[split..]).await.unwrap();
                loop {
                    match read_test_message(&mut peer, &mut receive_counter).await {
                        Message::Pong => break,
                        Message::Ping => {}
                        other => panic!("unexpected message: {other:?}"),
                    }
                }
            }
        };
        tokio::select! {
            result = run_channel(state, channel) => panic!("channel ended: {result:?}"),
            result = tokio::time::timeout(Duration::from_secs(10), scenario) => result.unwrap(),
        }
    }

    #[tokio::test]
    async fn simultaneous_large_transfers_keep_receiving() {
        let (local, mut peer) = socket_pair().await;
        let state = NetworkState::new(AppConfig::default());
        let outbound = state.outbound.clone();
        let channel = SecureChannel::new(local, &TEST_KEY, &TEST_KEY);
        let capture = test_capture(8 * 1024 * 1024);
        let expected = capture.item.id;
        let scenario = async {
            // Wait for subscription, then stop reading so the local send hits TCP backpressure.
            tokio::time::sleep(Duration::from_millis(20)).await;
            outbound.send(capture).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
            // The unknown item is deliberately rejected without accessing the OS clipboard.
            let chunk = Message::FileChunk {
                item_id: Uuid::new_v4(),
                relative_path: PathBuf::from("test.bin"),
                offset: 0,
                data_base64: BASE64.encode(vec![0; FILE_CHUNK_SIZE]),
                eof: false,
            };
            for counter in 0..16 {
                peer.write_all(&encrypted_frame(&chunk, counter))
                    .await
                    .unwrap();
            }
            peer.write_all(&encrypted_frame(&Message::Ping, 16))
                .await
                .unwrap();
            let mut receive_counter = 0;
            let mut got_clipboard = false;
            let mut got_pong = false;
            while !got_clipboard || !got_pong {
                match read_test_message(&mut peer, &mut receive_counter).await {
                    Message::Clipboard(item) => {
                        assert_eq!(item.id, expected);
                        got_clipboard = true;
                    }
                    Message::Pong => got_pong = true,
                    Message::Ping => {}
                    other => panic!("unexpected message: {other:?}"),
                }
            }
        };
        tokio::select! {
            result = run_channel(state, channel) => panic!("channel ended: {result:?}"),
            result = tokio::time::timeout(Duration::from_secs(20), scenario) => result.unwrap(),
        }
    }

    #[tokio::test]
    async fn unresponsive_peer_times_out() {
        let (local, _peer) = socket_pair().await;
        let state = NetworkState::new(AppConfig::default());
        let (_writer, reader) = SecureChannel::new(local, &TEST_KEY, &TEST_KEY).into_split();
        let (replies, _receiver) = mpsc::channel(32);
        tokio::time::pause();
        let (result, ()) = tokio::join!(
            read_messages(&state, reader, replies),
            tokio::time::advance(RECEIVE_TIMEOUT + Duration::from_secs(1)),
        );
        assert!(result.unwrap_err().to_string().contains("接收消息超时"));
    }

    #[tokio::test]
    async fn blocked_write_closes_connection_on_timeout() {
        let (local, _peer) = socket_pair().await;
        let (writer, _reader) = SecureChannel::new(local, &TEST_KEY, &TEST_KEY).into_split();
        let (data, receiver) = mpsc::channel(1);
        let (_replies, reply_receiver) = mpsc::channel(32);
        data.send(Message::Clipboard(
            test_capture(8 * 1024 * 1024).item.clone(),
        ))
        .await
        .unwrap();
        tokio::time::pause();
        let result = write_messages(writer, receiver, reply_receiver).await;
        assert!(result.unwrap_err().to_string().contains("发送消息超时"));
    }

    #[tokio::test]
    async fn incomplete_handshake_times_out() {
        let (mut local, mut peer) = socket_pair().await;
        peer.write_all(&[0, 0]).await.unwrap();
        tokio::time::pause();
        let (result, ()) = tokio::join!(
            read_plain::<ConnectionHello>(&mut local),
            tokio::time::advance(IO_TIMEOUT + Duration::from_secs(1)),
        );
        assert!(result.unwrap_err().to_string().contains("接收握手消息超时"));
    }

    #[tokio::test]
    async fn oversized_inbound_manifest_is_rejected_before_cache_or_clipboard() {
        let state = NetworkState::new(AppConfig::default());
        let item = ClipboardItem::new(
            Uuid::new_v4(),
            1,
            vec![ClipboardFormat::from_bytes("text/plain", b"x")],
            vec![crate::protocol::FileEntry {
                relative_path: "big.bin".into(),
                size: crate::protocol::MAX_ITEM_BYTES,
                sha256: String::new(),
                is_directory: false,
            }],
        )
        .unwrap();
        let mut pending = HashMap::new();
        assert!(
            handle_message(&state, &mut pending, Message::Clipboard(item))
                .await
                .is_err()
        );
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn oversized_handshake_is_rejected_from_header() {
        let (mut local, mut peer) = socket_pair().await;
        peer.write_u32((MAX_HANDSHAKE_SIZE + 1) as u32)
            .await
            .unwrap();
        let error = read_plain::<ConnectionHello>(&mut local).await.unwrap_err();
        assert!(error.to_string().contains("握手消息过大"));
    }

    #[tokio::test]
    async fn chunks_must_be_contiguous_and_partial_cache_is_cleaned() {
        let root = std::env::temp_dir().join(format!("clipboard-regression-{}", Uuid::new_v4()));
        fs::create_dir(&root).await.unwrap();
        let item = ClipboardItem::new(
            Uuid::new_v4(),
            1,
            vec![],
            vec![crate::protocol::FileEntry {
                relative_path: "test.bin".into(),
                size: 3,
                sha256: hex::encode(Sha256::digest(b"abc")),
                is_directory: false,
            }],
        )
        .unwrap();
        let mut pending = IncomingItem {
            item,
            root: root.clone(),
            completed: HashSet::new(),
            received: HashMap::new(),
            last_activity: tokio::time::Instant::now(),
            keep_cache: false,
        };
        let path = Path::new("test.bin");
        assert!(pending.write_chunk(path, 1, b"a", false).await.is_err());
        assert!(!pending.write_chunk(path, 0, b"ab", false).await.unwrap());
        assert!(pending.write_chunk(path, 0, b"a", false).await.is_err());
        assert!(
            pending
                .write_chunk(path, u64::MAX, b"a", false)
                .await
                .is_err()
        );
        assert!(pending.write_chunk(path, 2, b"cd", false).await.is_err());
        assert!(!pending.write_chunk(path, 2, b"c", false).await.unwrap());
        assert!(pending.write_chunk(path, 3, b"", true).await.unwrap());
        pending.verify().await.unwrap();
        drop(pending);
        tokio::time::timeout(Duration::from_secs(5), async {
            while root.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

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
