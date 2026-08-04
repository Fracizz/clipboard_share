use std::env;

use anyhow::Result;
use clap::{Parser, Subcommand};
use clipboard_share::{
    SyncService, autostart,
    service::{init_logging, run_daemon, spawn_background_daemon},
};
use tracing::error;
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
    let background = env::var_os("CLIPBOARD_SHARE_BACKGROUND").is_some();
    let _log_guard = init_logging(!background)?;

    match cli
        .command
        .unwrap_or(Commands::Daemon { background: false })
    {
        Commands::Start => {
            spawn_background_daemon()?;
            println!("ClipboardShare 已在后台启动");
            Ok(())
        }
        Commands::Stop => {
            if SyncService::request_stop_external()? {
                println!("已发送停止请求");
            } else {
                println!("ClipboardShare 当前未运行");
            }
            Ok(())
        }
        Commands::Daemon { background: true }
            if env::var_os("CLIPBOARD_SHARE_BACKGROUND").is_none() =>
        {
            spawn_background_daemon()?;
            Ok(())
        }
        Commands::Daemon { .. } => {
            run_daemon(std::sync::Arc::new(
                std::sync::atomic::AtomicBool::new(false),
            ))
            .await
        }
        Commands::PairListen { code } => {
            SyncService::pair_listen(code).await?;
            Ok(())
        }
        Commands::Pair { address, code } => SyncService::pair_connect(address, code).await,
        Commands::Status => {
            let status = SyncService::status()?;
            println!("本机：{} ({})", status.device_name, status.device_id);
            println!("监听端口：{}", status.listen_port);
            println!("配对端口：{}", status.pairing_port);
            println!(
                "运行状态：{}",
                if status.running {
                    "后台运行中"
                } else {
                    "未运行"
                }
            );
            if status.peers.is_empty() {
                println!("尚未配对设备");
            } else {
                println!("已配对设备：");
                for peer in status.peers {
                    println!(
                        "  {}  {}  {}",
                        peer.device_id, peer.device_name, peer.address
                    );
                }
            }
            Ok(())
        }
        Commands::Unpair { device_id } => {
            if SyncService::unpair(device_id)? {
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
