use std::{
    env,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

use crate::{
    clipboard,
    config::{AppConfig, AutoPairMode, log_dir},
    network::{self, NetworkState},
    process_control,
    windows_runtime,
};

#[derive(Debug, Clone, Serialize)]
pub struct PeerStatus {
    pub device_id: Uuid,
    pub device_name: String,
    pub address: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppStatus {
    pub device_id: Uuid,
    pub device_name: String,
    pub listen_port: u16,
    pub pairing_port: u16,
    pub running: bool,
    pub peers: Vec<PeerStatus>,
    pub pairing_code: Option<String>,
}

pub struct SyncService {
    join: Mutex<Option<JoinHandle<()>>>,
    stopping: Arc<AtomicBool>,
}

impl Default for SyncService {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncService {
    pub fn new() -> Self {
        Self {
            join: Mutex::new(None),
            stopping: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn status() -> Result<AppStatus> {
        let config = AppConfig::load_or_create()?;
        Ok(AppStatus {
            device_id: config.device_id,
            device_name: config.device_name,
            listen_port: config.listen_port,
            pairing_port: network::default_pairing_port(),
            running: process_control::is_running(config.device_id),
            peers: config
                .peers
                .into_iter()
                .map(|peer| PeerStatus {
                    device_id: peer.device_id,
                    device_name: peer.device_name,
                    address: peer.address,
                })
                .collect(),
            pairing_code: config.pairing_code,
        })
    }

    pub fn is_running() -> Result<bool> {
        let config = AppConfig::load_or_create()?;
        Ok(process_control::is_running(config.device_id))
    }

    pub fn start(&self) -> Result<()> {
        let config = AppConfig::load_or_create()?;
        if process_control::is_running(config.device_id) {
            bail!("ClipboardShare 已在运行");
        }

        let mut join = self.join.lock().expect("sync join mutex poisoned");
        if let Some(handle) = join.as_ref()
            && !handle.is_finished()
        {
            bail!("ClipboardShare 同步线程已在运行");
        }

        self.stopping.store(false, Ordering::SeqCst);
        let stopping = self.stopping.clone();
        let handle = thread::Builder::new()
            .name("clipboard-share-sync".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        error!(%error, "无法创建同步运行时");
                        return;
                    }
                };
                if let Err(error) = runtime.block_on(run_daemon(stopping)) {
                    error!(%error, "同步任务退出");
                }
            })
            .context("无法启动同步线程")?;
        *join = Some(handle);
        // 等待实例锁建立，便于 UI 立刻读到 running。
        for _ in 0..50 {
            if process_control::is_running(config.device_id) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<bool> {
        let config = AppConfig::load_or_create()?;
        self.stopping.store(true, Ordering::SeqCst);
        let stopped = process_control::request_stop(config.device_id)?;
        if let Some(handle) = self.join.lock().expect("sync join mutex poisoned").take() {
            let _ = handle.join();
        }
        Ok(stopped)
    }

    pub fn request_stop_external() -> Result<bool> {
        let config = AppConfig::load_or_create()?;
        process_control::request_stop(config.device_id)
    }

    pub async fn pair_listen(code: Option<String>) -> Result<String> {
        let mut config = AppConfig::load_or_create()?;
        let code = code.unwrap_or_else(crate::config::random_pairing_code);
        validate_code(&code)?;
        network::pair_listen(&mut config, &code).await?;
        Ok(code)
    }

    pub async fn pair_connect(address: String, code: String) -> Result<()> {
        validate_code(&code)?;
        let mut config = AppConfig::load_or_create()?;
        network::pair_connect(&mut config, &address, &code).await
    }

    pub fn unpair(device_id: Uuid) -> Result<bool> {
        let mut config = AppConfig::load_or_create()?;
        config.remove_peer(device_id)
    }
}

pub fn validate_code(code: &str) -> Result<()> {
    if code.len() != 6 || !code.chars().all(|character| character.is_ascii_digit()) {
        bail!("配对码必须是六位数字");
    }
    Ok(())
}

static LOG_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

pub fn init_logging(
    also_stderr: bool,
) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let directory = log_dir()?;
    std::fs::create_dir_all(&directory)?;
    let file_appender = tracing_appender::rolling::daily(directory, "clipboard-share.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("clipboard_share=info"));
    if also_stderr {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_writer(file_writer))
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .try_init()
            .ok();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_writer(file_writer))
            .try_init()
            .ok();
    }
    Ok(guard)
}

/// Initialize logging and keep the non-blocking writer alive for the process lifetime.
pub fn init_logging_global(also_stderr: bool) -> Result<()> {
    let guard = init_logging(also_stderr)?;
    let _ = LOG_GUARD.set(guard);
    Ok(())
}

pub async fn run_daemon(stopping: Arc<AtomicBool>) -> Result<()> {
    let mut config = AppConfig::load_or_create()?;
    let instance = process_control::InstanceControl::create(config.device_id)?;
    let apartment = windows_runtime::ClipboardApartment::initialize()?;
    let local = tokio::task::LocalSet::new();

    local
        .run_until(async move {
            let message_pump =
                tokio::task::spawn_local(async { windows_runtime::pump_messages().await });

            let operation = async {
                tokio::select! {
                    result = auto_pair_from_config(&mut config) => result?,
                    _ = instance.wait_for_stop() => {
                        info!("自动配对期间收到后台停止请求");
                        return Ok(());
                    }
                    _ = wait_flag(&stopping) => {
                        info!("自动配对期间收到停止标志");
                        return Ok(());
                    }
                }
                // 仅同步连接后的实时剪贴板变更，避免重连时回灌整段历史导致断连。
                let state = NetworkState::new(config.clone());
                let (capture_sender, mut capture_receiver) = mpsc::channel(64);
                let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);

                let watcher_state = state.clone();
                let watcher = tokio::task::spawn_local(async move {
                    clipboard::watch(
                        config,
                        capture_sender,
                        watcher_state.suppressed_hashes,
                        watcher_state.suppress_capture_until,
                        shutdown_receiver,
                    )
                    .await
                });

                let network_state = state.clone();
                let network =
                    tokio::task::spawn_local(async move { network::run(network_state).await });

                let outbound = state.outbound.clone();
                let dispatcher = tokio::task::spawn_local(async move {
                    while let Some(capture) = capture_receiver.recv().await {
                        let _ = outbound.send(Arc::new(capture));
                    }
                });

                info!("ClipboardShare 后台同步已启动");
                let outcome = tokio::select! {
                    result = watcher => result.context("剪贴板监听任务异常退出")?,
                    result = network => result.context("网络任务异常退出")?,
                    _ = dispatcher => Err(anyhow::anyhow!("剪贴板分发任务异常退出")),
                    result = tokio::signal::ctrl_c() => {
                        result?;
                        info!("收到退出信号");
                        Ok(())
                    }
                    _ = instance.wait_for_stop() => {
                        info!("收到后台停止请求");
                        Ok(())
                    }
                    _ = wait_flag(&stopping) => {
                        info!("收到停止标志");
                        Ok(())
                    }
                };
                let _ = shutdown_sender.send(true);
                outcome
            }
            .await;

            let shutdown = apartment.shutdown().await;
            message_pump.abort();
            operation?;
            shutdown
        })
        .await
}

async fn wait_flag(flag: &AtomicBool) {
    while !flag.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

pub async fn auto_pair_from_config(config: &mut AppConfig) -> Result<()> {
    if !config.peers.is_empty() {
        return Ok(());
    }
    let Some(auto_pair) = config.auto_pair.clone().filter(|value| value.enabled) else {
        return Ok(());
    };
    let code = config
        .pairing_code
        .clone()
        .context("auto_pair 已启用，但 JSON 中缺少 pairing_code")?;
    validate_code(&code)?;

    match auto_pair.mode {
        AutoPairMode::Listen => {
            info!("根据 JSON 配置等待自动配对");
            network::pair_listen(config, &code).await
        }
        AutoPairMode::Connect => {
            let address = auto_pair
                .peer_address
                .context("connect 模式缺少 peer_address")?;
            info!(%address, "根据 JSON 配置连接自动配对");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            loop {
                match network::pair_connect(config, &address, &code).await {
                    Ok(()) => return Ok(()),
                    Err(error) if tokio::time::Instant::now() < deadline => {
                        warn!(%error, "自动配对暂时失败，3 秒后重试");
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                    Err(error) => return Err(error.context("自动配对在 5 分钟内未完成")),
                }
            }
        }
    }
}

pub fn spawn_background_daemon() -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let executable = env::current_exe().context("无法定位当前可执行文件")?;
    Command::new(executable)
        .arg("daemon")
        .env("CLIPBOARD_SHARE_BACKGROUND", "1")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("无法创建后台同步进程")?;
    Ok(())
}
