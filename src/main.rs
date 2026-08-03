#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(not(windows))]
compile_error!("clipboard_share 仅支持 Windows 10 1809 及以上版本");

mod autostart;
mod clipboard;
mod config;
mod network;
mod process_control;
mod protocol;
mod windows_runtime;

use std::{
    env,
    process::{Command, Stdio},
    sync::Arc,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::{AppConfig, AutoPairMode, log_dir};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "clipboard_share",
    version,
    about = "局域网 Windows Win+V 剪贴板双向同步"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// 纯后台启动，不显示控制台窗口
    Start,
    /// 通知后台进程安全停止
    Stop,
    /// 启动同步进程
    Daemon {
        /// 创建无控制台窗口的后台子进程
        #[arg(long)]
        background: bool,
    },
    /// 生成一次性配对码并等待另一台电脑
    PairListen {
        /// 可选的六位配对码；省略则自动生成
        code: Option<String>,
    },
    /// 连接另一台电脑完成配对
    Pair {
        /// 对方 IP 地址，可附带配对端口
        address: String,
        /// 对方显示的六位一次性配对码
        code: String,
    },
    /// 显示本机和已配对设备
    Status,
    /// 删除一个已配对设备
    Unpair {
        /// status 中显示的设备 UUID
        device_id: Uuid,
    },
    /// 复制程序到用户目录并设置登录自启
    Install,
    /// 取消登录自启（保留配置和缓存）
    Uninstall,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        error!(%error, "程序退出");
        eprintln!("错误：{error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let _log_guard = init_logging()?;

    match cli
        .command
        .unwrap_or(Commands::Daemon { background: false })
    {
        Commands::Start => {
            spawn_background()?;
            println!("ClipboardShare 已在后台启动");
            Ok(())
        }
        Commands::Stop => {
            let config = AppConfig::load_or_create()?;
            if process_control::request_stop(config.device_id)? {
                println!("已发送停止请求");
            } else {
                println!("ClipboardShare 当前未运行");
            }
            Ok(())
        }
        Commands::Daemon { background: true }
            if env::var_os("CLIPBOARD_SHARE_BACKGROUND").is_none() =>
        {
            spawn_background()?;
            Ok(())
        }
        Commands::Daemon { .. } => run_daemon().await,
        Commands::PairListen { code } => {
            let mut config = AppConfig::load_or_create()?;
            let code = code.unwrap_or_else(config::random_pairing_code);
            validate_code(&code)?;
            network::pair_listen(&mut config, &code).await
        }
        Commands::Pair { address, code } => {
            validate_code(&code)?;
            let mut config = AppConfig::load_or_create()?;
            network::pair_connect(&mut config, &address, &code).await
        }
        Commands::Status => {
            let config = AppConfig::load_or_create()?;
            println!("本机：{} ({})", config.device_name, config.device_id);
            println!("监听端口：{}", config.listen_port);
            println!("配对端口：{}", network::default_pairing_port());
            println!(
                "运行状态：{}",
                if process_control::is_running(config.device_id) {
                    "后台运行中"
                } else {
                    "未运行"
                }
            );
            if config.peers.is_empty() {
                println!("尚未配对设备");
            } else {
                println!("已配对设备：");
                for peer in config.peers {
                    println!(
                        "  {}  {}  {}",
                        peer.device_id, peer.device_name, peer.address
                    );
                }
            }
            Ok(())
        }
        Commands::Unpair { device_id } => {
            let mut config = AppConfig::load_or_create()?;
            if config.remove_peer(device_id)? {
                println!("已解除配对：{device_id}");
            } else {
                println!("未找到设备：{device_id}");
            }
            Ok(())
        }
        Commands::Install => {
            let path = autostart::install()?;
            println!("安装完成：{}", path.display());
            println!("已设置当前用户登录后自动启动");
            Ok(())
        }
        Commands::Uninstall => {
            autostart::uninstall()?;
            println!("已取消登录自启，配置和缓存未删除");
            Ok(())
        }
    }
}

async fn run_daemon() -> Result<()> {
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
                }
                // 仅同步连接后的实时剪贴板变更，避免重连时回灌整段历史导致断连。
                let state = network::NetworkState::new(config.clone());
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

fn spawn_background() -> Result<()> {
    use std::os::windows::process::CommandExt;

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

fn init_logging() -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let directory = log_dir()?;
    std::fs::create_dir_all(&directory)?;
    let file_appender = tracing_appender::rolling::daily(directory, "clipboard-share.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("clipboard_share=info"));
    let background = env::var_os("CLIPBOARD_SHARE_BACKGROUND").is_some();
    if background {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_writer(file_writer))
            .try_init()
            .ok();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_writer(file_writer))
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .try_init()
            .ok();
    }
    Ok(guard)
}

fn validate_code(code: &str) -> Result<()> {
    if code.len() != 6 || !code.chars().all(|character| character.is_ascii_digit()) {
        anyhow::bail!("配对码必须是六位数字");
    }
    Ok(())
}

async fn auto_pair_from_config(config: &mut AppConfig) -> Result<()> {
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
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
            loop {
                match network::pair_connect(config, &address, &code).await {
                    Ok(()) => return Ok(()),
                    Err(error) if tokio::time::Instant::now() < deadline => {
                        warn!(%error, "自动配对暂时失败，3 秒后重试");
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    }
                    Err(error) => return Err(error.context("自动配对在 5 分钟内未完成")),
                }
            }
        }
    }
}
